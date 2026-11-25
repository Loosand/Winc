//! Git 的表单对话框：「新建工作树」「新建分支」。
//!
//! 窗口正中的功能对话框，填名称、选起点，按「创建」执行；和设置浮层同属模态卡片，各种表单共用一套
//! 状态、几何和绘制，只是行和选项不同（[`FormKind`]）。`git`、`ssh` 的输入请求同样使用该对话框。
//! 按「创建」执行的操作随输入的名称变化（[`FormPlan`]）：新建工作树时可能是新建分支、签出已有的、
//! 跟踪远程的，或者已经在别的工作树里签出着（主按钮换成「打开」）；不能做的原因写在框里。

use std::sync::{Arc, PoisonError};

use winc_editor::git::{RebaseAction, unique_dir};
use winc_i18n::t;

use super::dialog::QuestionView;
pub(crate) use super::dialog::{FormDialog, FormKind, FormPlan, PushForm, PushOutgoing};
use super::operate::{PUSH_PREVIEW, PushIntent, local_name, short};
use super::worktree::NameTarget;
use crate::app::{App, Focus, UserEvent};
use crate::askpass::Prompt;
use crate::layout::DialogPart;

/// 返回按键 `base` 对应的变基处理方式：取各命令的首字母。
fn action_of_key(base: &str) -> Option<RebaseAction> {
    match base {
        "p" => Some(RebaseAction::Pick),
        "r" => Some(RebaseAction::Reword),
        "e" => Some(RebaseAction::Edit),
        "s" => Some(RebaseAction::Squash),
        "f" => Some(RebaseAction::Fixup),
        "d" => Some(RebaseAction::Drop),
        _ => None,
    }
}

impl App {
    /// 处理交互式变基对话框列表上的按键；不是该对话框或列表没有焦点时返回 `false`。
    pub(in crate::app) fn rebase_list_key(
        &mut self,
        key: &winit::keyboard::Key,
        base: &str,
    ) -> bool {
        use winit::keyboard::{Key, NamedKey};
        let Some(dialog) = &self.overlays.form_dialog else {
            return false;
        };
        let FormKind::Rebase(form) = &dialog.kind else {
            return false;
        };
        if dialog.focus != DialogPart::RebaseList {
            return false;
        }
        let selected = form.selected;
        let alt = self.pointer.modifiers.alt_key();
        match key {
            Key::Named(NamedKey::ArrowUp) if alt => self.move_rebase_row(true),
            Key::Named(NamedKey::ArrowDown) if alt => self.move_rebase_row(false),
            Key::Named(NamedKey::ArrowUp) => self.select_rebase_row(selected.saturating_sub(1)),
            Key::Named(NamedKey::ArrowDown) => self.select_rebase_row(selected + 1),
            Key::Named(NamedKey::Enter) => self.submit_form_dialog(),
            _ => match action_of_key(base) {
                Some(action) => self.set_rebase_action(selected, action),
                None => return false,
            },
        }
        true
    }

    /// 返回「基于」一行要显示的文字：选了起点时是它的名字，否则是当前分支或当前提交。
    fn base_label(&self, base: Option<&str>) -> String {
        let current = self.git.branches.iter().find(|b| b.current && !b.remote);
        match (base, current) {
            (Some(base), _) => base.to_owned(),
            (None, Some(b)) => t!("git-form-base-current-branch", name = b.name.as_str()),
            (None, None) => t!("git-form-base-current-commit").to_owned(),
        }
    }

    /// 按对话框当前填写的内容，说明「创建」会做什么。
    pub(crate) fn form_plan(&self, dialog: &FormDialog) -> FormPlan {
        match &dialog.kind {
            FormKind::Property(edit) => self.property_plan(edit, dialog),
            FormKind::Worktree => self.worktree_plan(dialog),
            FormKind::Branch { at } => self.branch_plan(dialog, at.as_deref()),
            FormKind::Rebase(form) => self.rebase_plan(form, dialog.name.text()),
            FormKind::Clone => self.clone_plan(dialog.name.text().trim(), dialog.folder.as_deref()),
            FormKind::Remote { editing } => {
                self.remote_plan(editing.as_deref(), dialog.name.text(), dialog.second.text())
            }
            FormKind::Connect => self.connect_plan(dialog),
            FormKind::Push(form) => self.push_plan(dialog, form),
            FormKind::GoToRevision | FormKind::CompareRevision { .. } => {
                let revision = dialog.name.text().trim();
                // 提交号、分支名和标签名都不含空白，也不以 `-` 开头。
                let problem = revision.starts_with('-') || revision.contains(char::is_whitespace);
                FormPlan {
                    problem: problem.then(|| t!("git-go-to-invalid").to_owned()),
                    confirm: if matches!(dialog.kind, FormKind::GoToRevision) {
                        t!("git-go-to-button")
                    } else {
                        t!("git-compare-button")
                    },
                    enabled: !revision.is_empty() && !problem,
                    base: String::new(),
                    base_enabled: false,
                    dir: None,
                }
            }
            FormKind::RenameGroup => FormPlan {
                problem: None,
                confirm: t!("workspace-group-rename-button"),
                enabled: true,
                base: String::new(),
                base_enabled: false,
                dir: None,
            },
            FormKind::LogPath => {
                let path = dialog.name.text().trim();
                FormPlan {
                    problem: None,
                    confirm: t!("git-log-path-button"),
                    enabled: !path.is_empty(),
                    base: String::new(),
                    base_enabled: false,
                    dir: None,
                }
            }
            FormKind::Question(prompt, _) => FormPlan {
                problem: None,
                confirm: QuestionView::of(prompt, None).confirm,
                // 用户名为空的登录请求必然失败；密码和口令允许为空。
                enabled: !matches!(prompt, Prompt::Username { .. })
                    || !dialog.name.text().is_empty(),
                base: String::new(),
                base_enabled: false,
                dir: None,
            },
        }
    }

    /// 推送：目标分支不能为空，不含空白，也不以 `-` 开头；有多个远程时可以换远程。
    fn push_plan(&self, dialog: &FormDialog, form: &PushForm) -> FormPlan {
        let name = dialog.name.text().trim();
        let invalid = name.starts_with('-') || name.contains(char::is_whitespace);
        let remote = dialog.base.clone().unwrap_or_else(|| form.remote.clone());
        FormPlan {
            problem: invalid.then(|| t!("git-push-form-branch-invalid").to_owned()),
            confirm: match form.intent {
                PushIntent::Publish(_) => t!("git-push-confirm-publish-button"),
                _ => t!("git-push-confirm-push-button"),
            },
            enabled: !name.is_empty() && !invalid && !remote.is_empty(),
            base: remote,
            base_enabled: self.git.remotes.len() > 1,
            dir: None,
        }
    }

    /// 新建分支：名称不能和已有的本地分支重名。
    fn branch_plan(&self, dialog: &FormDialog, at: Option<&str>) -> FormPlan {
        let name = dialog.name.text().trim();
        let (base, base_enabled) = match at {
            Some(at) => (t!("git-form-base-commit", commit = short(at)), false),
            None => (self.base_label(dialog.base.as_deref()), true),
        };
        let problem = if name.starts_with('-') {
            Some(t!("git-form-name-leading-dash"))
        } else if (self.git.branches.iter()).any(|b| !b.remote && b.name == name) {
            Some(t!("git-form-name-exists"))
        } else {
            None
        };
        FormPlan {
            enabled: !name.is_empty() && problem.is_none(),
            problem: problem.map(str::to_owned),
            confirm: t!("git-form-create"),
            base,
            base_enabled,
            dir: None,
        }
    }

    /// 新建工作树：按 [`App::worktree_name_target`] 看名称指的是什么。
    fn worktree_plan(&self, dialog: &FormDialog) -> FormPlan {
        let name = dialog.name.text().trim();
        let base = self.base_label(dialog.base.as_deref());
        let dir = |local: &str| {
            let local = if local.is_empty() { "…" } else { local };
            match &dialog.folder {
                Some(folder) => Some(unique_dir(folder, local)),
                None => self.new_worktree_dir(local),
            }
        };
        let plan = |problem: Option<String>| FormPlan {
            enabled: problem.is_none(),
            problem,
            confirm: t!("git-form-create"),
            base: base.clone(),
            base_enabled: true,
            dir: dir(name),
        };
        // 不是新建分支时没有起点可选，「基于」写成灰的。
        let fixed = |plan: FormPlan, text: &str| FormPlan {
            base: text.to_owned(),
            base_enabled: false,
            ..plan
        };
        // 所选目录与仓库不在同一系统中时不允许新建工作树：一侧的 `git` 无法使用另一侧的路径。
        let elsewhere = (dialog.folder.as_deref())
            .zip(self.git.root.as_deref())
            .is_some_and(|(folder, root)| !winc_editor::wsl::same_system(root, folder));
        let creating = |plan: FormPlan| {
            if elsewhere {
                FormPlan {
                    enabled: false,
                    problem: Some(t!("git-form-other-system").to_owned()),
                    ..plan
                }
            } else {
                plan
            }
        };

        if name.is_empty() {
            return creating(FormPlan {
                enabled: false,
                ..plan(None)
            });
        }
        match self.worktree_name_target(name) {
            // `git worktree add -b` 把名字原样交给 `git branch`，「-」开头的会被当成选项。
            NameTarget::New if name.starts_with('-') => FormPlan {
                dir: None,
                ..plan(Some(t!("git-form-name-leading-dash").to_owned()))
            },
            NameTarget::New => creating(plan(None)),
            // 远程分支：签出同名的本地分支，「基于」写跟踪的远程分支。
            NameTarget::Branch(branch) if branch.remote => creating(FormPlan {
                dir: dir(local_name(&branch)),
                ..fixed(plan(None), &branch.name)
            }),
            NameTarget::Branch(_) => {
                creating(fixed(plan(None), t!("git-form-base-existing-branch")))
            }
            // 已在别的工作树中签出：主按钮换成「打开」，位置写那个工作树的目录。
            NameTarget::Held(i) => FormPlan {
                confirm: t!("git-form-open"),
                dir: Some(self.git.worktrees[i].path.clone()),
                ..fixed(plan(None), t!("git-form-base-existing-branch"))
            },
            NameTarget::Current => FormPlan {
                dir: None,
                ..fixed(
                    plan(Some(t!("git-form-current-branch-held").to_owned())),
                    t!("git-form-base-current"),
                )
            },
            NameTarget::Stale => FormPlan {
                dir: None,
                ..fixed(
                    plan(Some(t!("git-form-stale-worktree").to_owned())),
                    t!("git-form-base-existing-branch"),
                )
            },
        }
    }

    /// 按「创建」（或回车）：按 [`FormPlan`] 执行。
    ///
    /// 不能做时什么都不做，原因已经写在输入框里。
    pub(in crate::app) fn submit_form_dialog(&mut self) {
        let Some(dialog) = &self.overlays.form_dialog else {
            return;
        };
        if dialog.running.is_some() || !self.form_plan(dialog).enabled {
            return;
        }
        if matches!(dialog.kind, FormKind::Question(..)) {
            self.answer_git_question();
            return;
        }
        if matches!(dialog.kind, FormKind::Connect) {
            self.submit_connect();
            return;
        }
        if matches!(dialog.kind, FormKind::GoToRevision) {
            let revision = dialog.name.text().trim().to_owned();
            self.close_form_dialog();
            self.go_to_log_revision(revision);
            return;
        }
        if let FormKind::CompareRevision { commit } = &dialog.kind {
            let commit = commit.clone();
            let revision = dialog.name.text().trim().to_owned();
            self.close_form_dialog();
            self.compare_log_with_revision(commit, revision);
            return;
        }
        if matches!(dialog.kind, FormKind::RenameGroup) {
            let name = dialog.name.text().to_owned();
            self.close_form_dialog();
            self.rename_group(&name);
            return;
        }
        if matches!(dialog.kind, FormKind::LogPath) {
            // 写成相对仓库根目录的样子：开头的 `./`、`/` 和末尾的 `/` 去掉。
            let text = dialog.name.text().trim();
            let path = text.trim_start_matches("./").trim_matches('/').to_owned();
            self.close_form_dialog();
            if !path.is_empty() {
                self.toggle_log_path(path);
            }
            return;
        }
        // 须在发起操作前记录编号：没有窗口时（测试中）后台操作会同步完成。
        let next = self.git.branch.started + 1;
        let kind = dialog.kind.clone();
        if let Some(dialog) = &mut self.overlays.form_dialog {
            dialog.running = Some(next);
            dialog.error = None;
        }
        let Some(dialog) = &self.overlays.form_dialog else {
            return;
        };
        match kind {
            FormKind::Property(edit) => self.submit_property(edit),
            FormKind::Worktree => self.submit_worktree(),
            FormKind::Branch { at } => {
                let name = dialog.name.text().trim().to_owned();
                let from = at.or_else(|| dialog.base.clone());
                let checkout = dialog.checkout;
                self.git_new_branch(name, from, checkout);
            }
            FormKind::Question(..)
            | FormKind::Connect
            | FormKind::GoToRevision
            | FormKind::CompareRevision { .. }
            | FormKind::RenameGroup
            | FormKind::LogPath => {}
            FormKind::Clone => self.submit_clone(),
            FormKind::Push(form) => {
                let remote = dialog.base.clone().unwrap_or_else(|| form.remote.clone());
                let target = dialog.name.text().trim().to_owned();
                let (tags, never) = (dialog.copy, dialog.new_window);
                self.submit_push(*form, (remote, target), tags, never);
            }
            FormKind::Rebase(_) => self.submit_rebase(),
            FormKind::Remote { editing } => self.submit_remote(editing),
        }
        // 已发起 git 操作时，待其完成后再关闭（[`App::form_operation_done`]），期间按钮显示
        // 「正在创建…」；未发起操作（上一个操作未完成、直接打开已有的工作树）时不等待。
        if let Some(dialog) = &mut self.overlays.form_dialog
            && self.git.branch.started != next
        {
            dialog.running = None;
        }
        self.request_redraw();
    }

    /// 返回键盘焦点可停留的部件，按 `Tab` 的顺序排列。
    ///
    /// 跳过不可用的部件：不可选的起点、不可按的主按钮。
    fn form_focus_order(&self) -> Vec<DialogPart> {
        let Some(dialog) = &self.overlays.form_dialog else {
            return Vec::new();
        };
        let plan = self.form_plan(dialog);
        if let FormKind::Rebase(form) = &dialog.kind {
            let mut order = vec![DialogPart::RebaseList];
            if form.rewording() {
                order.push(DialogPart::Branch);
            }
            order.push(DialogPart::Cancel);
            if plan.enabled {
                order.push(DialogPart::Confirm);
            }
            return order;
        }
        let layout = self.dialog_layout();
        let mut order = Vec::new();
        // 推送对话框的远程菜单在分支输入框上面，按从上到下的顺序先到它。
        let pushing = matches!(dialog.kind, FormKind::Push(_));
        if pushing && plan.base_enabled {
            order.push(DialogPart::Base);
        }
        // 无需输入的请求没有输入框；第二个输入框只用于登录和添加远程仓库。
        if layout.branch.w > 0.0 {
            order.push(DialogPart::Branch);
        }
        if layout.second.w > 0.0 {
            order.push(DialogPart::Second);
        }
        if layout.third.w > 0.0 {
            order.push(DialogPart::Third);
        }
        if plan.base_enabled && !pushing {
            order.push(DialogPart::Base);
        }
        if layout.change().is_some() {
            order.push(DialogPart::Change);
        }
        order.extend(layout.options.iter().map(|&(part, _)| part));
        order.push(DialogPart::Cancel);
        if plan.enabled && dialog.running.is_none() {
            order.push(DialogPart::Confirm);
        }
        order
    }

    /// 将键盘焦点移到下一个部件（`back` 时为上一个，对应 `⇧Tab`），到达末尾时从头循环。
    pub(in crate::app) fn form_focus_move(&mut self, back: bool) {
        let order = self.form_focus_order();
        let Some(dialog) = &mut self.overlays.form_dialog else {
            return;
        };
        let n = order.len();
        // 焦点不在任何部件上时（无需输入的请求刚打开时），`Tab` 从第一个部件开始。
        let next = match order.iter().position(|&p| p == dialog.focus) {
            Some(at) if back => (at + n - 1) % n,
            Some(at) => (at + 1) % n,
            None if back => n - 1,
            None => 0,
        };
        dialog.focus = order[next];
        self.ime.field_preedit.clear();
        self.request_overlay_redraw();
    }

    /// 以空格、回车激活键盘焦点所在的部件（名称除外）。
    pub(in crate::app) fn form_activate(&mut self) {
        let Some(part) = self.overlays.form_dialog.as_ref().map(|d| d.focus) else {
            return;
        };
        self.press_form_part(part);
    }

    /// 新建工作树：新建分支、签出已有的，或者打开已经签出着的工作树。
    fn submit_worktree(&mut self) {
        let Some(dialog) = &self.overlays.form_dialog else {
            return;
        };
        let name = dialog.name.text().trim().to_owned();
        let (base, folder, new_window) = (
            dialog.base.clone(),
            dialog.folder.clone(),
            dialog.new_window,
        );
        let globs = if dialog.copy {
            self.config.git_worktree_copy.clone()
        } else {
            Vec::new()
        };
        let target = self.worktree_name_target(&name);

        let dir = |app: &App, local: &str| match &folder {
            Some(folder) => Some(unique_dir(folder, local)),
            None => app.new_worktree_dir(local),
        };
        match target {
            NameTarget::Held(i) => {
                self.close_form_dialog();
                self.open_worktree(i, new_window);
            }
            NameTarget::Branch(branch) => {
                let Some(dir) = dir(self, local_name(&branch)) else {
                    return;
                };
                self.checkout_in_worktree(&branch, dir, globs, new_window);
            }
            NameTarget::New => {
                let Some(dir) = dir(self, &name) else {
                    return;
                };
                let target = self.git_path_arg(&dir);
                self.add_worktree(
                    t!("git-op-new-worktree", name = name.as_str()),
                    dir,
                    globs,
                    new_window,
                    move |git, root| {
                        let mut args =
                            vec!["worktree", "add", "-b", &name, "--end-of-options", &target];
                        args.extend(base.as_deref());
                        git.operate(root, &args)
                    },
                );
            }
            NameTarget::Stale | NameTarget::Current => {}
        }
    }

    /// 对话框中窗口坐标 (`x`, `y`) 处的部件；主按钮不能按、起点不能选时不算。
    pub(in crate::app) fn dialog_part_at(&self, x: f32, y: f32) -> Option<DialogPart> {
        let dialog = self.overlays.form_dialog.as_ref()?;
        let layout = self.dialog_layout();
        if let FormKind::Rebase(form) = &dialog.kind
            && let Some(part) = self.rebase_part_at(&layout, form, x, y)
        {
            return Some(part);
        }
        let part = layout.part_at(x, y)?;
        let plan = self.form_plan(dialog);
        match part {
            DialogPart::Branch if matches!(&dialog.kind, FormKind::Rebase(form) if !form.rewording()) => {
                None
            }
            DialogPart::Confirm if !plan.enabled || dialog.running.is_some() => None,
            DialogPart::Base if !plan.base_enabled => None,
            part => Some(part),
        }
    }

    /// 处理对话框打开时的鼠标按下：点击卡片外部时关闭对话框，点击部件时执行其操作。
    pub(in crate::app) fn click_form_dialog(&mut self, x: f32, y: f32) {
        let layout = self.dialog_layout();
        if !layout.card.contains(x, y) {
            // 点击卡片外部不关闭询问对话框：关闭表示取消，会使正在进行的操作失败。
            let asking = (self.overlays.form_dialog.as_ref())
                .is_some_and(|d| matches!(d.kind, FormKind::Question(..)));
            if !asking {
                self.close_form_dialog();
            }
            return;
        }
        match self.dialog_part_at(x, y) {
            Some(part @ (DialogPart::Branch | DialogPart::Second | DialogPart::Third)) => {
                self.focus = Focus::Dialog;
                if let Some(dialog) = &mut self.overlays.form_dialog {
                    dialog.focus = part;
                }
                self.click_field(x, y);
            }
            Some(part) => self.press_form_part(part),
            None => {}
        }
        self.request_redraw();
    }

    /// 执行点击（或以键盘激活）对话框中 `part` 的操作。
    fn press_form_part(&mut self, part: DialogPart) {
        let shown = self
            .overlays
            .form_dialog
            .as_ref()
            .map(|dialog| &dialog.kind);
        let connecting = matches!(shown, Some(FormKind::Connect));
        match part {
            DialogPart::Close | DialogPart::Cancel => self.close_form_dialog(),
            DialogPart::Confirm => self.submit_form_dialog(),
            DialogPart::Branch | DialogPart::Second | DialogPart::Third => {
                self.submit_form_dialog();
            }
            DialogPart::Base => {
                use crate::app::input::context_menu::{MenuTarget, menu_below};
                let base = self.dialog_layout().base;
                let target = match shown {
                    Some(FormKind::Push(_)) => MenuTarget::FormRemote,
                    _ => MenuTarget::FormBase,
                };
                self.open_context_menu(target, menu_below(base));
            }
            DialogPart::Change if connecting => self.pick(crate::app::Pick::KeyFile),
            DialogPart::Change => self.pick(crate::app::Pick::WorktreeFolder),
            DialogPart::RebaseList => self.submit_form_dialog(),
            part @ (DialogPart::RebaseRow(_)
            | DialogPart::RebaseAction(_)
            | DialogPart::RebaseUp
            | DialogPart::RebaseDown) => self.press_rebase_part(part),
            DialogPart::Copy | DialogPart::NewWindow | DialogPart::Checkout => {
                if let Some(dialog) = &mut self.overlays.form_dialog {
                    let option = match part {
                        DialogPart::Copy => &mut dialog.copy,
                        DialogPart::NewWindow => &mut dialog.new_window,
                        _ => &mut dialog.checkout,
                    };
                    *option = !*option;
                }
            }
        }
        self.request_redraw();
    }

    /// 对话框打开时移动鼠标：只认对话框里的部件；在输入框里拖动时选择文字。
    pub(in crate::app) fn hover_form_dialog(&mut self, x: f32, y: f32) {
        let part = self.dialog_part_at(x, y);
        if let Some(dialog) = &mut self.overlays.form_dialog
            && dialog.hover != part
        {
            dialog.hover = part;
            self.request_overlay_redraw();
        }
        self.update_cursor_icon();
        if self.pointer.drag == crate::app::Drag::Field
            && let Some(field) = self.focused_field()
        {
            let pos = field.view.position_at_point(x, y);
            field.set_caret(pos, true);
            self.request_overlay_redraw();
        }
    }

    /// 把对话框的起点设为「基于」菜单中选的分支 `base`；`None` 是当前分支。
    pub(in crate::app) fn choose_form_base(&mut self, base: Option<String>) {
        if let Some(dialog) = &mut self.overlays.form_dialog {
            dialog.base = base;
        }
        self.refresh_push_list();
        self.request_redraw();
    }

    /// 推送对话框的目标（远程、分支）改了以后，在后台重读会送出的提交，读到时由
    /// [`App::take_push_outgoing`] 换上；目标没变或分支名无效时不读。
    pub(in crate::app) fn refresh_push_list(&mut self) {
        use winc_editor::git::Outgoing;
        let Some(dialog) = &mut self.overlays.form_dialog else {
            return;
        };
        let FormKind::Push(form) = &mut dialog.kind else {
            return;
        };
        let remote = dialog.base.clone().unwrap_or_else(|| form.remote.clone());
        let branch = dialog.name.text().trim().to_owned();
        let invalid = branch.starts_with('-') || branch.contains(char::is_whitespace);
        let wanted = (remote, branch);
        if invalid || wanted.1.is_empty() || wanted == form.read_for {
            return;
        }
        form.read_for = wanted.clone();
        let default = wanted.0 == form.remote && wanted.1 == form.target;
        let publish = matches!(form.intent, PushIntent::Publish(_));
        let (Some(root), slot) = (self.git.root.clone(), Arc::clone(&self.git.push_outgoing))
        else {
            return;
        };
        self.spawn_git(UserEvent::GitStatus, move |tool| {
            let (remote, branch) = &wanted;
            let target = match (default, publish) {
                (true, true) => Outgoing::Publish,
                (true, false) => Outgoing::Upstream,
                (false, _) => Outgoing::To { remote, branch },
            };
            let read = tool
                .ok()
                .map(|git| git.outgoing(&root, target, PUSH_PREVIEW));
            let (subjects, total) = read.and_then(Result::ok).unwrap_or_default();
            let read = Some((wanted.clone(), subjects, total));
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = read;
        });
        // 没有事件循环（测试中）时后台的工作已经就地做完：直接取用。
        self.take_push_outgoing();
    }

    /// 收下 [`App::refresh_push_list`] 读到的提交；读的不是对话框现在的目标时不用。
    pub(in crate::app) fn take_push_outgoing(&mut self) {
        let read = (self.git.push_outgoing.lock())
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some((target, subjects, total)) = read else {
            return;
        };
        let dialog = self.overlays.form_dialog.as_mut();
        let Some(FormKind::Push(form)) = dialog.map(|d| &mut d.kind) else {
            return;
        };
        if form.read_for == target {
            form.subjects = subjects;
            form.total = total;
            form.rows = form.rows.max(form.lines());
            self.request_redraw();
        }
    }
}
