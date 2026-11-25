//! 交互式变基的对话框：调整提交的顺序和处理方式，确认后执行。
//!
//! 从日志里提交的右键菜单发起，范围是该提交到 `HEAD`。列表按执行顺序排列，最早的提交在最上面，
//! 因此「压缩」「修正」并入的是上一行。提交信息只在「改写」时可以编辑。

use winc_editor::git::{RebaseAction, RebasePlan};
use winc_i18n::t;

use super::dialog::{FormKind, FormPlan};
pub(crate) use super::dialog::{REBASE_ACTIONS, RebaseForm, action_label};
use super::operate::Then;
use crate::app::input::context_menu::{MenuCommand, MenuTarget};
use crate::app::{App, TextField};
use crate::layout::{DialogLayout, DialogPart, REBASE_PILL_W, REBASE_ROW_H};

impl App {
    /// 打开交互式变基对话框。
    pub(super) fn open_rebase_dialog(&mut self, plan: RebasePlan) {
        if self.overlays.form_dialog.is_some() {
            return;
        }
        let kind = FormKind::Rebase(Box::new(RebaseForm::new(plan)));
        self.show_form_dialog(kind, TextField::multiline(), DialogPart::RebaseList);
    }

    /// 读取从提交 `oid` 到 `HEAD` 的范围，打开交互式变基对话框。
    pub(in crate::app) fn start_interactive_rebase(&mut self, oid: String) {
        self.git_operate_then(t!("git-op-rebase-load").into(), move |git, root| {
            git.rebase_plan(root, &oid)
                .map(|plan| (String::new(), Then::RebasePlan(Box::new(plan))))
        });
    }

    /// 返回对话框的内容，附带 `f` 的修改权限；不是交互式变基对话框时返回 `None`。
    fn with_rebase<T>(
        &mut self,
        f: impl FnOnce(&mut RebaseForm, &mut TextField) -> T,
    ) -> Option<T> {
        let dialog = self.overlays.form_dialog.as_mut()?;
        let FormKind::Rebase(form) = &mut dialog.kind else {
            return None;
        };
        Some(f(form, &mut dialog.name))
    }

    /// 选中第 `row` 行，并把信息框换成这一行的信息。
    pub(super) fn select_rebase_row(&mut self, row: usize) {
        self.store_rebase_message();
        self.with_rebase(|form, field| {
            form.selected = row.min(form.rows.len().saturating_sub(1));
            load_message(form, field);
        });
        self.keep_rebase_row_visible();
        self.request_redraw();
    }

    /// 把信息框的内容存入选中的行。
    fn store_rebase_message(&mut self) {
        self.with_rebase(|form, field| {
            let selected = form.selected;
            if let Some(row) = form.rows.get_mut(selected)
                && row.action == RebaseAction::Reword
            {
                row.message = Some(field.text().to_owned());
            }
        });
    }

    /// 滚动列表，使选中的行可见。
    fn keep_rebase_row_visible(&mut self) {
        let list = self.dialog_layout().list;
        self.with_rebase(|form, _| {
            let top = form.selected as f32 * REBASE_ROW_H;
            if top < form.scroll {
                form.scroll = top;
            } else if top + REBASE_ROW_H > form.scroll + list.h {
                form.scroll = top + REBASE_ROW_H - list.h;
            }
        });
    }

    /// 把第 `row` 行的处理方式设为 `action`；改写时键盘焦点移到信息框。
    pub(in crate::app) fn set_rebase_action(&mut self, row: usize, action: RebaseAction) {
        self.select_rebase_row(row);
        self.with_rebase(|form, field| {
            let selected = form.selected;
            let Some(row) = form.rows.get_mut(selected) else {
                return;
            };
            row.action = action;
            row.message = (action == RebaseAction::Reword).then(|| {
                row.message
                    .clone()
                    .unwrap_or_else(|| row.commit.message.clone())
            });
            load_message(form, field);
        });
        if let Some(dialog) = &mut self.overlays.form_dialog {
            dialog.focus = if action == RebaseAction::Reword {
                DialogPart::Branch
            } else {
                DialogPart::RebaseList
            };
        }
        self.request_redraw();
    }

    /// 把选中的行上移（`up`）或下移一行。
    pub(super) fn move_rebase_row(&mut self, up: bool) {
        self.store_rebase_message();
        self.with_rebase(|form, _| {
            let selected = form.selected;
            let target = if up {
                selected.checked_sub(1)
            } else {
                Some(selected + 1).filter(|&next| next < form.rows.len())
            };
            if let Some(target) = target {
                form.rows.swap(selected, target);
                form.selected = target;
            }
        });
        self.keep_rebase_row_visible();
        self.request_redraw();
    }

    /// 返回交互式变基对话框中 (`x`, `y`) 处的列表部件。
    pub(super) fn rebase_part_at(
        &self,
        layout: &DialogLayout,
        form: &RebaseForm,
        x: f32,
        y: f32,
    ) -> Option<DialogPart> {
        if layout.values.first().is_some_and(|r| r.contains(x, y)) {
            return Some(DialogPart::RebaseUp);
        }
        if layout.values.get(1).is_some_and(|r| r.contains(x, y)) {
            return Some(DialogPart::RebaseDown);
        }
        if !layout.list.contains(x, y) {
            return None;
        }
        let row = ((y - layout.list.y + form.scroll) / REBASE_ROW_H) as usize;
        (row < form.rows.len()).then_some(if x < layout.list.x + REBASE_PILL_W {
            DialogPart::RebaseAction(row)
        } else {
            DialogPart::RebaseRow(row)
        })
    }

    /// 执行点击（或以键盘激活）交互式变基对话框中 `part` 的操作。
    pub(in crate::app) fn press_rebase_part(&mut self, part: DialogPart) {
        match part {
            DialogPart::RebaseRow(row) => {
                if let Some(dialog) = &mut self.overlays.form_dialog {
                    dialog.focus = DialogPart::RebaseList;
                }
                self.select_rebase_row(row);
            }
            DialogPart::RebaseAction(row) => {
                self.select_rebase_row(row);
                let layout = self.dialog_layout();
                let scroll = self.with_rebase(|form, _| form.scroll).unwrap_or(0.0);
                let y = layout.list.y + (row + 1) as f32 * REBASE_ROW_H - scroll;
                self.open_context_menu(MenuTarget::RebaseAction(row), (layout.list.x, y));
            }
            DialogPart::RebaseUp => self.move_rebase_row(true),
            DialogPart::RebaseDown => self.move_rebase_row(false),
            _ => {}
        }
    }

    /// 在交互式变基对话框的列表上滚动 `dy`（逻辑像素）。
    pub(in crate::app) fn rebase_wheel(&mut self, dy: f32) {
        let list = self.dialog_layout().list;
        self.with_rebase(|form, _| {
            let content = form.rows.len() as f32 * REBASE_ROW_H;
            form.scroll = (form.scroll - dy).clamp(0.0, (content - list.h).max(0.0));
        });
        self.request_redraw();
    }

    /// 返回交互式变基对话框的 [`FormPlan`]。
    ///
    /// `editing` 是信息框的内容（见 [`RebaseForm::problem`]）。
    pub(super) fn rebase_plan(&self, form: &RebaseForm, editing: &str) -> FormPlan {
        let problem = form.problem(editing);
        FormPlan {
            enabled: problem.is_none() && form.changed(),
            problem: problem.map(str::to_owned),
            confirm: t!("git-rebase-button"),
            base: String::new(),
            base_enabled: false,
            dir: None,
        }
    }

    /// 开始变基。
    ///
    /// 变基可能停在冲突处等待处理，因此先关闭对话框，结果由状态栏和提示说明。
    pub(super) fn submit_rebase(&mut self) {
        self.store_rebase_message();
        let Some((base, steps)) = self.with_rebase(|form, _| (form.base.clone(), form.steps()))
        else {
            return;
        };
        self.close_form_dialog();
        self.git_operate_then(t!("git-op-rebase").into(), move |git, root| {
            git.interactive_rebase(root, base.as_deref(), &steps)
                .map(|stopped| {
                    if stopped {
                        (
                            String::new(),
                            Then::Warn(t!("git-rebase-stopped").to_owned()),
                        )
                    } else {
                        (String::new(), Then::Nothing)
                    }
                })
        });
    }

    /// 执行处理方式菜单 `command`：`row` 是弹出菜单时的行。
    pub(in crate::app) fn run_rebase_action_command(&mut self, row: usize, command: MenuCommand) {
        if let MenuCommand::RebaseAction(action) = command {
            self.set_rebase_action(row, action);
        }
    }
}

/// 把信息框换成选中行的信息；不在改写时清空。
fn load_message(form: &RebaseForm, field: &mut TextField) {
    let message = form
        .rows
        .get(form.selected)
        .and_then(|row| row.message.as_deref());
    field.set_text(message.unwrap_or_default());
    field.set_caret(field.text().len(), false);
}

#[cfg(test)]
mod tests;
