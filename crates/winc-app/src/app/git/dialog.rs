//! Git 表单对话框的模态壳层与数据结构：对话框状态、各类表单的内容描述、打开/关闭生命周期与
//! 几何布局。表单的校验和提交分发见 [`super::form`]。

use std::path::{Path, PathBuf};

use winc_editor::git::{RebaseAction, RebaseCommit, RebasePlan, RebaseStep};
use winc_i18n::t;

use super::operate::{Asker, PushIntent};
use crate::app::editor::find::ref_name_field;
use crate::app::{App, Focus, Hover, TextField};
use crate::askpass::{InputKind, Prompt, host_of};
use crate::layout::{DialogLayout, DialogPart};

/// 询问对话框中的一行只读信息。
pub(crate) struct QuestionValue {
    /// 名称。
    pub label: &'static str,
    /// 内容。
    pub text: String,
    /// 是否使用等宽字体：指纹需要逐字符核对。
    pub mono: bool,
}

/// 询问对话框中的一个输入框。
pub(crate) struct QuestionInput {
    /// 名称。
    pub label: &'static str,
    /// 占位文字。
    pub placeholder: &'static str,
}

/// 询问对话框显示的内容。
pub(crate) struct QuestionView {
    /// 标题。
    pub title: String,
    /// 说明文字。
    pub message: Option<String>,
    /// 为说明文字保留的行数（行）。
    pub message_lines: usize,
    /// 只读信息。
    pub values: Vec<QuestionValue>,
    /// 输入框，顺序为 `name`、`password`。
    pub inputs: Vec<QuestionInput>,
    /// 主按钮的文字。
    pub confirm: &'static str,
}

/// 无法识别的提示在有输入框时保留的行数（行）：较长的提示折行后通常不超过 3 行。
const OTHER_LINES: usize = 3;
/// 无法识别的提示在无输入框时保留的行数（行）。
///
/// 主要用于带其他已知名称的主机密钥说明：主机、指纹、各个已知名称和确认语各占一至两行。
const OTHER_CONFIRM_LINES: usize = 8;
/// 主机密钥说明保留的行数（行）：两种界面语言的说明在对话框宽度内都不超过 1 行。
const HOST_KEY_LINES: usize = 1;

impl QuestionView {
    /// 返回「转到提交号、分支或标签」对话框的内容：一个输入框。
    pub(super) fn go_to_revision() -> Self {
        Self {
            title: t!("git-go-to-title").to_owned(),
            message: None,
            message_lines: 0,
            values: Vec::new(),
            inputs: vec![QuestionInput {
                label: t!("git-go-to-label"),
                placeholder: t!("git-go-to-placeholder"),
            }],
            confirm: t!("git-go-to-button"),
        }
    }

    /// 返回「与指定版本比较」对话框的内容：提交 `commit` 的提交号和一个输入框。
    pub(super) fn compare_revision(commit: &str) -> Self {
        Self {
            title: t!("git-compare-revision-title").to_owned(),
            message: None,
            message_lines: 0,
            values: vec![QuestionValue {
                label: t!("git-compare-commit"),
                text: commit.to_owned(),
                mono: true,
            }],
            inputs: vec![QuestionInput {
                label: t!("git-compare-with"),
                placeholder: t!("git-go-to-placeholder"),
            }],
            confirm: t!("git-compare-button"),
        }
    }

    /// 返回给一组目录起名的对话框内容：一个输入框。
    pub(super) fn rename_group() -> Self {
        Self {
            title: t!("workspace-group-rename-title").to_owned(),
            message: None,
            message_lines: 0,
            values: Vec::new(),
            inputs: vec![QuestionInput {
                label: t!("workspace-group-rename-label"),
                placeholder: t!("workspace-group-rename-placeholder"),
            }],
            confirm: t!("workspace-group-rename-button"),
        }
    }

    /// 返回「按路径筛选」对话框的内容：一个输入框。
    pub(super) fn log_path() -> Self {
        Self {
            title: t!("git-log-path-title").to_owned(),
            message: None,
            message_lines: 0,
            values: Vec::new(),
            inputs: vec![QuestionInput {
                label: t!("git-log-path-label"),
                placeholder: t!("git-log-path-placeholder"),
            }],
            confirm: t!("git-log-path-button"),
        }
    }

    /// 返回添加远程仓库（`editing` 为 `None`）或修改其地址的对话框内容。
    pub(crate) fn remote(editing: Option<&str>) -> QuestionView {
        let input = |label, placeholder| QuestionInput { label, placeholder };
        let url = input(t!("git-remote-url"), t!("git-remote-url-placeholder"));
        match editing {
            Some(name) => QuestionView {
                title: t!("git-remote-edit-title").to_owned(),
                message: None,
                message_lines: 0,
                values: vec![QuestionValue {
                    label: t!("git-remote-name"),
                    text: name.to_owned(),
                    mono: false,
                }],
                inputs: vec![url],
                confirm: t!("git-remote-save-button"),
            },
            None => QuestionView {
                title: t!("git-remote-add-title").to_owned(),
                message: None,
                message_lines: 0,
                values: Vec::new(),
                inputs: vec![input(t!("git-remote-name"), "origin"), url],
                confirm: t!("git-remote-add-button"),
            },
        }
    }

    /// 返回连接远端主机的对话框内容：主机、端口和密钥文件。
    pub(crate) fn connect() -> QuestionView {
        let input = |label, placeholder| QuestionInput { label, placeholder };
        QuestionView {
            title: t!("workspace-remote-dialog-title").to_owned(),
            message: None,
            message_lines: 0,
            values: Vec::new(),
            inputs: vec![
                input(
                    t!("workspace-remote-dialog-host"),
                    t!("workspace-remote-dialog-host-placeholder"),
                ),
                input(t!("workspace-remote-dialog-port"), "22"),
                input(
                    t!("workspace-remote-dialog-key"),
                    t!("workspace-remote-dialog-key-placeholder"),
                ),
            ],
            confirm: t!("workspace-remote-dialog-connect"),
        }
    }

    /// 返回 `prompt` 对应的对话框内容；`origin` 是提出请求的远端主机，排在只读信息的第一行。
    pub(crate) fn of(prompt: &Prompt, origin: Option<&str>) -> QuestionView {
        let mut view = Self::of_prompt(prompt);
        if let Some(host) = origin {
            let from = QuestionValue {
                label: t!("git-question-origin"),
                text: host.to_owned(),
                mono: false,
            };
            view.values.insert(0, from);
        }
        view
    }

    /// 返回 `prompt` 本身对应的对话框内容。
    fn of_prompt(prompt: &Prompt) -> QuestionView {
        let value = |label, text: &str| QuestionValue {
            label,
            text: text.to_owned(),
            mono: false,
        };
        let input = |label| QuestionInput {
            label,
            placeholder: "",
        };
        let password = QuestionInput {
            label: t!("git-question-password"),
            placeholder: t!("git-question-password-placeholder"),
        };
        let base = QuestionView {
            title: String::new(),
            message: None,
            message_lines: 0,
            values: Vec::new(),
            inputs: Vec::new(),
            confirm: t!("git-question-sign-in-button"),
        };
        match prompt {
            Prompt::Username { server } => QuestionView {
                title: t!("git-question-sign-in", host = host_of(server)),
                values: vec![value(t!("git-question-server"), server)],
                inputs: vec![input(t!("git-question-username")), password],
                ..base
            },
            Prompt::Password { server, account } => QuestionView {
                title: t!("git-question-sign-in", host = host_of(server)),
                values: (account.iter())
                    .map(|account| value(t!("git-question-account"), account))
                    .chain([value(t!("git-question-server"), server)])
                    .collect(),
                inputs: vec![password],
                ..base
            },
            Prompt::SshPassword { account, host } => QuestionView {
                title: t!("git-question-sign-in", host = host.as_str()),
                values: vec![value(t!("git-question-account"), account)],
                inputs: vec![password],
                ..base
            },
            Prompt::Passphrase { key } => QuestionView {
                title: t!("git-question-unlock-key").to_owned(),
                values: vec![value(
                    t!("git-question-key"),
                    &winc_editor::display_path(Path::new(key), None),
                )],
                inputs: vec![input(t!("git-question-passphrase"))],
                confirm: t!("git-question-unlock-button"),
                ..base
            },
            Prompt::HostKey {
                host,
                key_type,
                fingerprint,
            } => QuestionView {
                title: t!("git-question-host-key").to_owned(),
                message: Some(t!("git-question-host-key-message").to_owned()),
                message_lines: HOST_KEY_LINES,
                values: vec![
                    value(t!("git-question-host"), host),
                    value(t!("git-question-key-type"), key_type),
                    QuestionValue {
                        mono: true,
                        ..value(t!("git-question-fingerprint"), fingerprint)
                    },
                ],
                confirm: t!("git-question-connect-button"),
                ..base
            },
            Prompt::Other { text, input: kind } => {
                let confirmation = *kind == InputKind::None;
                QuestionView {
                    title: if confirmation {
                        t!("git-question-confirm")
                    } else {
                        t!("git-question-input")
                    }
                    .to_owned(),
                    message: Some(text.clone()),
                    message_lines: if confirmation {
                        OTHER_CONFIRM_LINES
                    } else {
                        OTHER_LINES
                    },
                    inputs: if confirmation {
                        Vec::new()
                    } else {
                        vec![input(t!("git-question-response"))]
                    },
                    confirm: t!("git-question-ok-button"),
                    ..base
                }
            }
        }
    }
}

/// 交互式变基列表中的一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RebaseRow {
    /// 提交。
    pub commit: RebaseCommit,
    /// 处理方式。
    pub action: RebaseAction,
    /// 改写后的信息；只在 [`RebaseAction::Reword`] 时有值。
    pub message: Option<String>,
    /// 打开对话框时在第几行，用于判断顺序是否变化。
    original: usize,
}

/// 交互式变基对话框的内容。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RebaseForm {
    /// 变基的基点；从根提交开始时为 `None`。
    pub base: Option<String>,
    /// 各行，最早的提交在最前。
    pub rows: Vec<RebaseRow>,
    /// 选中的行。
    pub selected: usize,
    /// 列表的滚动位置（逻辑像素）。
    pub scroll: f32,
    /// 范围内已推送的提交数。
    pub pushed: usize,
}

// 浮点滚动位置不会是 NaN，整体比较是安全的。
impl Eq for RebaseForm {}

/// 处理方式在菜单和列表里的文字。
pub(crate) fn action_label(action: RebaseAction) -> &'static str {
    match action {
        RebaseAction::Pick => t!("git-rebase-pick"),
        RebaseAction::Reword => t!("git-rebase-reword"),
        RebaseAction::Edit => t!("git-rebase-edit"),
        RebaseAction::Squash => t!("git-rebase-squash"),
        RebaseAction::Fixup => t!("git-rebase-fixup"),
        RebaseAction::Drop => t!("git-rebase-drop"),
    }
}

/// 全部处理方式，顺序就是菜单里的顺序。
pub(crate) const REBASE_ACTIONS: [RebaseAction; 6] = [
    RebaseAction::Pick,
    RebaseAction::Reword,
    RebaseAction::Edit,
    RebaseAction::Squash,
    RebaseAction::Fixup,
    RebaseAction::Drop,
];

impl RebaseForm {
    /// 创建范围 `plan` 对应的对话框内容，全部为「保留」。
    pub(super) fn new(plan: RebasePlan) -> RebaseForm {
        let rows = (plan.commits.into_iter().enumerate())
            .map(|(original, commit)| RebaseRow {
                commit,
                action: RebaseAction::Pick,
                message: None,
                original,
            })
            .collect();
        RebaseForm {
            base: plan.base,
            rows,
            selected: 0,
            scroll: 0.0,
            pushed: plan.pushed,
        }
    }

    /// 返回不能开始的原因；可以开始或没有更改时返回 `None`。
    pub(super) fn problem(&self, editing: &str) -> Option<&'static str> {
        let mut kept = self
            .rows
            .iter()
            .filter(|row| row.action != RebaseAction::Drop);
        match kept.next().map(|row| row.action) {
            None => return Some(t!("git-rebase-all-dropped")),
            Some(RebaseAction::Squash | RebaseAction::Fixup) => {
                return Some(t!("git-rebase-first-squash"));
            }
            Some(_) => {}
        }
        let empty = (self.rows.iter().enumerate())
            .filter(|(_, row)| row.action == RebaseAction::Reword)
            .any(|(i, row)| {
                let message = if i == self.selected {
                    editing
                } else {
                    row.message.as_deref().unwrap_or_default()
                };
                message.trim().is_empty()
            });
        empty.then(|| t!("git-rebase-empty-message"))
    }

    /// 与打开对话框时相比是否有更改。
    pub(super) fn changed(&self) -> bool {
        (self.rows.iter().enumerate())
            .any(|(i, row)| row.original != i || row.action != RebaseAction::Pick)
    }

    /// 返回对话框内容对应的待办步骤。
    pub(super) fn steps(&self) -> Vec<RebaseStep> {
        self.rows
            .iter()
            .map(|row| RebaseStep {
                oid: row.commit.oid.clone(),
                action: row.action,
                message: row.message.clone(),
            })
            .collect()
    }

    /// 选中的行是否在改写信息。
    pub(crate) fn rewording(&self) -> bool {
        (self.rows.get(self.selected)).is_some_and(|row| row.action == RebaseAction::Reword)
    }
}

/// 表单对话框是哪一种。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FormKind {
    /// 新建工作树：分支、基于、位置，复制被忽略的文件、在新窗口中打开。
    Worktree,
    /// 新建分支：名称、基于，创建后签出。
    ///
    /// `at` 是日志里选的提交（完整的提交号），从它新建时「基于」不能改；`None` 时「基于」可以
    /// 选分支。
    Branch { at: Option<String> },
    /// `git`、`ssh` 的输入请求，见 [`super::question`]；请求来自远端主机上的命令时，后一项是
    /// 那台主机。
    Question(Prompt, Option<String>),
    /// 克隆仓库：地址、位置。
    Clone,
    /// 添加远程仓库，或修改 `editing` 的地址。
    Remote { editing: Option<String> },
    /// 在 Git 日志中转到一个提交：输入提交号、分支名或标签名。
    GoToRevision,
    /// 把 Git 日志中的提交 `commit` 与输入的提交号、分支名或标签名比较。
    CompareRevision { commit: String },
    /// 按路径筛选 Git 日志：输入相对仓库根目录的路径或通配。
    LogPath,
    /// 给窗口中的这组目录起名。
    RenameGroup,
    /// 添加或编辑本地笔记的属性。
    Property(crate::app::preview::markdown::properties::PropertyEdit),
    /// 交互式变基。
    Rebase(Box<RebaseForm>),
    /// 连接远端主机：主机、端口，见 [`crate::app::remote`]。
    Connect,
    /// 推送：远程、目标分支、待推送的提交，可一并推送标签。
    Push(Box<PushForm>),
}

/// 推送对话框打开时的内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PushForm {
    /// 推送还是发布。
    pub intent: PushIntent,
    /// 当前分支的名字。
    pub branch: String,
    /// 默认的远程：上游所在的远程，发布时是选出的远程。
    pub remote: String,
    /// 默认的目标分支：上游的分支名，发布时与当前分支同名。
    pub target: String,
    /// 待推送的提交的标题，最新的在前；提交多时只有最新的几个。
    pub subjects: Vec<String>,
    /// 待推送的提交数。
    pub total: usize,
    /// 提交列表按哪个目标读出（远程、分支）；改了目标以后在后台重读，读到时换上。
    pub read_for: (String, String),
    /// 提交列表占的行数：打开以来最多的一次，改目标时对话框不忽高忽低。
    pub rows: usize,
}

impl PushForm {
    /// 提交列表占几行：每个标题一行，没有列出的提交另占一行；没有待推送的提交时是一行说明。
    pub(crate) fn lines(&self) -> usize {
        let more = usize::from(self.total > self.subjects.len());
        (self.subjects.len() + more).max(1)
    }
}

/// 推送对话框改了目标后在后台读到的提交：读的目标（远程、分支）、标题和总数。
pub(crate) type PushOutgoing = ((String, String), Vec<String>, usize);

impl FormKind {
    /// 返回使用询问对话框版式的表单的内容；其他表单返回 `None`。
    pub(crate) fn view(&self) -> Option<QuestionView> {
        match self {
            FormKind::Question(prompt, origin) => Some(QuestionView::of(prompt, origin.as_deref())),
            FormKind::Remote { editing } => Some(QuestionView::remote(editing.as_deref())),
            FormKind::Connect => Some(QuestionView::connect()),
            FormKind::GoToRevision => Some(QuestionView::go_to_revision()),
            FormKind::CompareRevision { commit } => Some(QuestionView::compare_revision(commit)),
            FormKind::LogPath => Some(QuestionView::log_path()),
            FormKind::RenameGroup => Some(QuestionView::rename_group()),
            FormKind::Property(edit) => Some(edit.view()),
            FormKind::Worktree
            | FormKind::Branch { .. }
            | FormKind::Clone
            | FormKind::Push(_)
            | FormKind::Rebase(_) => None,
        }
    }
}

/// 表单对话框的状态。
pub(crate) struct FormDialog {
    /// 哪一种。
    pub kind: FormKind,
    /// 名称的输入框；打开对话框时就在这里输入。
    pub name: TextField,
    /// 询问对话框的第二个输入框：登录的密码（遮盖）、远程仓库的地址；其他表单不显示。
    pub second: TextField,
    /// 连接对话框的第三个输入框：密钥文件；其他表单不显示。
    pub third: TextField,
    /// 新分支从哪个分支开始；`None` 是当前分支（游离时是当前提交）。
    pub base: Option<String>,
    /// 用「位置」右端的目录按钮选的目录，工作树建在它下面；`None` 是默认位置。
    pub folder: Option<PathBuf>,
    /// 是否复制配置 `git.worktreeCopy` 匹配的被忽略的文件。
    pub copy: bool,
    /// 工作树建好以后在新窗口打开（`false` 是当前窗口换过去）。
    pub new_window: bool,
    /// 分支建好以后签出它。
    pub checkout: bool,
    /// 鼠标悬停的部件。
    pub hover: Option<DialogPart>,
    /// 键盘焦点所在的部件。
    pub focus: DialogPart,
    /// 按下「创建」后仍在执行的 git 操作的编号（`BranchState::started`）。
    pub running: Option<u64>,
    /// 上一次创建失败的原因，显示在按钮左侧；修改名称时清空。
    pub error: Option<String>,
    /// 打开对话框前的焦点，关闭以后还回去。
    back: Focus,
}

/// 按「创建」会做什么，跟着输入的名称、选的起点变。
pub(crate) struct FormPlan {
    /// 不能创建的原因，用错误色写在输入框右端；框里就是名称，原因里不再重复。
    pub problem: Option<String>,
    /// 主按钮的字。
    pub confirm: &'static str,
    /// 主按钮能不能按。
    pub enabled: bool,
    /// 「基于」右边写的字。
    pub base: String,
    /// 能不能选起点。
    pub base_enabled: bool,
    /// 工作树的目录；已经签出着时是那个工作树的目录，建不了、新建分支时为 `None`。
    pub dir: Option<PathBuf>,
}

impl App {
    /// 打开新建工作树对话框。
    pub(in crate::app) fn open_worktree_dialog(&mut self) {
        self.open_form_dialog(FormKind::Worktree, None);
    }

    /// 打开新建分支对话框：`from` 是从哪个分支开始（`None` 是当前分支），`at` 是从日志里哪个
    /// 提交开始（这时「基于」不能改）。
    pub(in crate::app) fn open_branch_dialog(&mut self, from: Option<String>, at: Option<String>) {
        self.open_form_dialog(FormKind::Branch { at }, from);
    }

    /// 打开推送对话框：`subjects` 是待推送的提交的标题，`total` 是提交数。
    pub(in crate::app) fn open_push_dialog(
        &mut self,
        intent: PushIntent,
        subjects: Vec<String>,
        total: usize,
    ) {
        if self.overlays.form_dialog.is_some() {
            return;
        }
        let current = self.git.branches.iter().find(|b| b.current && !b.remote);
        let branch = current.map_or("HEAD", |b| b.name.as_str()).to_owned();
        let upstream = current.and_then(|b| b.upstream.clone()).unwrap_or_default();
        // 上游写作「远程名/分支名」，远程名本身可以含 `/`，按已知的远程名匹配最长的一个。
        let tracked = (self.git.remotes.iter())
            .filter(|r| upstream.starts_with(&format!("{r}/")))
            .max_by_key(|r| r.len());
        let (remote, target) = match (&intent, tracked) {
            (PushIntent::Publish(remote), _) => (remote.clone(), branch.clone()),
            (_, Some(remote)) => (remote.clone(), upstream[remote.len() + 1..].to_owned()),
            (_, None) => (upstream, branch.clone()),
        };
        let mut name = ref_name_field();
        name.set_text(&target);
        name.set_caret(target.len(), false);
        let form = PushForm {
            intent,
            branch,
            remote: remote.clone(),
            target: target.clone(),
            subjects,
            total,
            read_for: (remote.clone(), target),
            rows: 0,
        };
        let form = PushForm {
            rows: form.lines(),
            ..form
        };
        self.show_form_dialog(FormKind::Push(Box::new(form)), name, DialogPart::Confirm);
        if let Some(dialog) = &mut self.overlays.form_dialog {
            dialog.base = Some(remote);
            dialog.copy = false;
        }
    }

    /// 打开表单对话框；不在 Git 仓库里时说明。
    fn open_form_dialog(&mut self, kind: FormKind, base: Option<String>) {
        if self.git.status.is_none() {
            self.flash(t!("git-not-a-repository"));
            return;
        }
        if self.overlays.form_dialog.is_some() {
            return;
        }
        self.show_form_dialog(kind, ref_name_field(), DialogPart::Branch);
        if let Some(dialog) = &mut self.overlays.form_dialog {
            dialog.base = base;
        }
    }

    /// 关闭其他浮层，显示表单对话框：`name` 是它的输入框，`focus` 是一开始的键盘焦点。
    pub(in crate::app) fn show_form_dialog(
        &mut self,
        kind: FormKind,
        name: TextField,
        focus: DialogPart,
    ) {
        if self.overlays.picker.open {
            self.close_picker();
        }
        if self.bars.goto_input.is_some() {
            self.close_prompt();
        }
        self.finish_name_input();
        self.close_settings();

        let back = match self.focus {
            focus @ (Focus::Editor | Focus::Tree | Focus::CommitList | Focus::Terminal) => focus,
            _ => Focus::Editor,
        };
        self.overlays.form_dialog = Some(FormDialog {
            kind,
            name,
            second: TextField::masked(),
            third: TextField::default(),
            base: None,
            folder: None,
            copy: true,
            new_window: false,
            checkout: true,
            hover: None,
            focus,
            running: None,
            error: None,
            back,
        });
        self.focus = Focus::Dialog;
        self.set_hover(Hover::None);
        self.request_redraw();
    }

    /// 关闭表单对话框，焦点还给打开之前的地方。
    pub(in crate::app) fn close_form_dialog(&mut self) {
        let Some(dialog) = self.overlays.form_dialog.take() else {
            return;
        };
        if matches!(dialog.kind, FormKind::Question(..)) {
            self.decline_git_question();
        }
        if matches!(dialog.kind, FormKind::Connect) {
            self.connect_dialog_closed();
        }
        self.ime.field_preedit.clear();
        if self.focus == Focus::Dialog {
            self.focus = dialog.back;
        }
        self.update_cursor_icon();
        self.request_redraw();
    }

    /// 关闭询问对话框时调用：尚未答复的请求按用户取消处理。
    fn decline_git_question(&mut self) {
        self.git.branch.remember = None;
        match self.git.branch.asking.take() {
            Some((Asker::Git(_), _)) => self.git.branch.declined = true,
            Some((Asker::Remote, _)) => self.cancel_remote(),
            None => {}
        }
    }

    /// 当前的询问对话框是否提供「记住密码」。
    pub(in crate::app) fn question_remembers(&self) -> bool {
        let asking = (self.overlays.form_dialog.as_ref())
            .is_some_and(|dialog| matches!(dialog.kind, FormKind::Question(..)));
        asking && self.git.branch.remember.is_some()
    }

    /// 对话框的几何；窗口太小时卡片缩到窗口以内。
    pub(crate) fn dialog_layout(&self) -> DialogLayout {
        let window = self.layout.window;
        match self.overlays.form_dialog.as_ref().map(|d| &d.kind) {
            Some(FormKind::Clone) => DialogLayout::clone_form(window),
            Some(FormKind::Connect) => DialogLayout::connect_form(window),
            Some(FormKind::Rebase(form)) => {
                DialogLayout::rebase(window, form.rows.len(), form.pushed > 0)
            }
            Some(kind) if let Some(view) = kind.view() => DialogLayout::question(
                window,
                view.message_lines,
                view.values.len(),
                view.inputs.len(),
                self.question_remembers(),
            ),
            Some(FormKind::Branch { .. }) => {
                DialogLayout::compute(window, false, &[DialogPart::Checkout])
            }
            // 两个勾选项借用已有的部件：`Copy` 是「推送标签」，`NewWindow` 是「不再询问」。
            Some(FormKind::Push(form)) => DialogLayout::push(
                window,
                form.rows,
                &[DialogPart::Copy, DialogPart::NewWindow],
            ),
            _ if self.config.git_worktree_copy.is_empty() => {
                DialogLayout::compute(window, true, &[DialogPart::NewWindow])
            }
            _ => DialogLayout::compute(window, true, &[DialogPart::Copy, DialogPart::NewWindow]),
        }
    }

    /// 处理后台完成的编号为 `id` 的 git 操作；操作由对话框发起时返回 `true`。
    pub(in crate::app) fn form_operation_done(&mut self, id: u64, error: Option<&str>) -> bool {
        let Some(dialog) = self
            .overlays
            .form_dialog
            .as_mut()
            .filter(|d| d.running == Some(id))
        else {
            return false;
        };
        dialog.running = None;
        match error {
            Some(e) => {
                dialog.error = Some(e.to_owned());
                dialog.focus = DialogPart::Branch;
                self.request_redraw();
            }
            None => self.close_form_dialog(),
        }
        true
    }

    /// 「基于」菜单里第 `i` 项是不是现在选的起点。
    pub(in crate::app) fn form_base_chosen(&self, i: usize) -> bool {
        let Some(dialog) = &self.overlays.form_dialog else {
            return false;
        };
        match i.checked_sub(1) {
            None => dialog.base.is_none(),
            Some(i) => self
                .git
                .branches
                .get(i)
                .is_some_and(|b| dialog.base.as_deref() == Some(b.name.as_str())),
        }
    }

    /// 推送对话框的远程菜单里第 `i` 个远程是不是现在选的。
    pub(in crate::app) fn form_remote_chosen(&self, i: usize) -> bool {
        let chosen = (self.overlays.form_dialog.as_ref()).and_then(|d| d.base.as_deref());
        chosen.is_some() && chosen == self.git.remotes.get(i).map(String::as_str)
    }

    /// 目录按钮选好了目录：工作树建在它下面。
    pub(in crate::app) fn worktree_folder_picked(&mut self, folder: PathBuf) {
        if let Some(dialog) = &mut self.overlays.form_dialog {
            dialog.folder = Some(folder);
        }
        self.request_redraw();
    }
}
