//! `git`、`ssh` 在操作中的输入请求：登录凭据、私钥口令和主机密钥确认。
//!
//! 请求由 [`crate::askpass`] 从后台线程送达，通过表单对话框（[`FormKind::Question`]）答复。操作结束
//! 时关闭未答复的对话框；用户关闭对话框表示取消，该操作随后的失败不按错误提示。

use std::mem;

use winc_i18n::t;

use super::dialog::FormKind;
pub(crate) use super::dialog::{QuestionInput, QuestionValue, QuestionView};
pub(in crate::app) use super::operate::Asker;
use crate::app::NoticeKind;
use crate::app::lifecycle::notice::NoticeAction;
use crate::app::{App, Focus, TextField, UserEvent};
use crate::askpass::{CONFIRMED, InputKind, Prompt, Question};
use crate::layout::DialogPart;

/// 在后台线程中删除全部已保存的凭据。
pub(in crate::app) fn forget_credentials() {
    Asker::forget_credentials();
}

impl App {
    /// 处理后台送达的输入请求；所属操作已结束的请求被丢弃，askpass 程序按取消处理。
    pub(in crate::app) fn poll_git_questions(&mut self) {
        while let Ok((id, question)) = self.git.branch.questions.1.try_recv() {
            if self.git.branch.running.is_some() && self.git.branch.started == id {
                self.open_git_question(id, question);
            }
        }
    }

    /// 答复第 `id` 个操作的输入请求：优先使用登录对话框中已填写的密码，否则打开对话框。
    ///
    /// `git` 依次请求用户名和密码，登录对话框一次收集两项，其中的密码用于答复紧随其后的密码请求。
    /// 已打开的表单被关闭，因为操作正在等待答复。无需输入的请求没有默认答复：键盘焦点不落在按钮上，
    /// 接受主机密钥必须由用户明确选择。
    pub(in crate::app) fn open_git_question(&mut self, id: u64, question: Question) {
        // 用户已取消过这个操作的询问：之后的询问同样按取消答复，不再弹出。
        if self.git.branch.declined {
            return;
        }
        self.open_question(Asker::Git(id), question);
    }

    /// 答复来自 `asker` 的输入请求，规则见 [`App::open_git_question`]。
    pub(in crate::app) fn open_question(&mut self, asker: Asker, question: Question) {
        let Question {
            prompt,
            reply,
            remember,
        } = question;
        if let Prompt::Password { server, .. } = &prompt
            && let Some((known, password)) = self.git.branch.password.take()
            && known == *server
        {
            let _ = reply.send(password);
            return;
        }
        self.close_form_dialog();
        let (name, focus) = match &prompt {
            Prompt::Username { .. } => (TextField::default(), DialogPart::Branch),
            Prompt::Other {
                input: InputKind::Visible,
                ..
            } => (TextField::default(), DialogPart::Branch),
            prompt if prompt.is_confirmation() => (TextField::default(), DialogPart::Close),
            Prompt::Passphrase { .. }
            | Prompt::Password { .. }
            | Prompt::SshPassword { .. }
            | Prompt::Other { .. }
            | Prompt::HostKey { .. } => (TextField::masked(), DialogPart::Branch),
        };
        self.git.branch.asking = Some((asker, reply));
        let offered = remember.is_some();
        self.git.branch.remember = remember;
        self.show_form_dialog(FormKind::Question(prompt, None), name, focus);
        // 「记住密码」借用 `checkout` 一项，默认勾选。
        if let Some(dialog) = &mut self.overlays.form_dialog {
            dialog.checkout = offered;
        }
    }

    /// 提交询问对话框。
    ///
    /// 登录对话框在密码为空时先把键盘焦点移到密码框，密码可能确实为空，再次提交时按空密码答复。
    pub(super) fn answer_git_question(&mut self) {
        let Some(dialog) = &mut self.overlays.form_dialog else {
            return;
        };
        let FormKind::Question(prompt, _) = &dialog.kind else {
            return;
        };
        let answer = if prompt.is_confirmation() {
            CONFIRMED.to_owned()
        } else {
            dialog.name.text().replace(['\r', '\n'], "")
        };
        if let Prompt::Username { server } = prompt {
            if dialog.second.text().is_empty() && dialog.focus == DialogPart::Branch {
                dialog.focus = DialogPart::Second;
                self.request_redraw();
                return;
            }
            let password = dialog.second.text().replace(['\r', '\n'], "");
            self.git.branch.password = Some((server.clone(), password));
        }
        if let Some(flag) = self.git.branch.remember.take() {
            flag.store(dialog.checkout, std::sync::atomic::Ordering::Relaxed);
        }
        if let Some((_, reply)) = self.git.branch.asking.take() {
            let _ = reply.send(answer);
        }
        self.close_form_dialog();
    }

    /// 第 `id` 个操作结束时调用：清除已填写的密码，关闭未答复的对话框，不按用户取消处理。
    pub(in crate::app) fn end_git_questions(&mut self, id: u64) {
        if self.git.branch.started == id {
            self.git.branch.password = None;
        }
        self.end_questions(Asker::Git(id));
    }

    fn end_questions(&mut self, asker: Asker) {
        if self
            .git
            .branch
            .asking
            .as_ref()
            .is_some_and(|(at, _)| *at == asker)
        {
            self.git.branch.asking = None;
            self.git.branch.remember = None;
            self.close_form_dialog();
        }
    }

    /// 报告操作失败：用户取消输入时提示已取消，否则显示 `message` 并提供查看完整输出的按钮。
    pub(in crate::app) fn report_git_failure(&mut self, what: &str, message: String) {
        if mem::take(&mut self.git.branch.declined) {
            self.flash(t!("git-op-canceled", what = what));
        } else {
            self.notify_with(NoticeKind::Error, message, NoticeAction::GitOutput);
        }
    }

    /// 在只读标签中显示 Git 的输出记录；标签开着时内容实时更新（[`App::refresh_git_output`]）。
    pub(in crate::app) fn show_git_output(&mut self) {
        let (text, written) = winc_editor::git::output_snapshot();
        let shown = if text.is_empty() {
            t!("git-output-empty").to_owned()
        } else {
            text
        };
        self.editor.show_output(t!("git-output-title"), &shown);
        self.git.output_shown = written;
        self.focus = Focus::Editor;
        self.relayout();
        self.ensure_caret_visible();
        self.request_redraw();
    }

    /// 让 Git 输出的新内容送到本窗口：记录每写入一次就向事件循环发一个事件，窗口关闭后不再发。
    ///
    /// 测试中没有事件循环，不登记；测试直接调用 [`App::refresh_git_output`]。
    pub(crate) fn watch_git_output(&self) {
        if let Some(proxy) = self.link.proxy.clone() {
            winc_editor::git::on_output(move || proxy.send_event(UserEvent::GitOutput).is_ok());
        }
    }

    /// Git 输出的标签开着时，把新写入的内容接在末尾：命令一开始、输出每到一行都看得到，不用关掉
    /// 重开。
    ///
    /// 标签没开着时不做事；不抢焦点、不切换标签，正在看的位置不动。
    pub(in crate::app) fn refresh_git_output(&mut self) {
        let title = t!("git-output-title");
        // 标签里是「暂无输出」的说明，或者接不上（最早的内容已被丢弃）：整篇换掉。
        let (more, text, written) = match winc_editor::git::output_since(self.git.output_shown) {
            Some((more, written)) => (Some(more), String::new(), written),
            None => {
                let (text, written) = winc_editor::git::output_snapshot();
                (None, text, written)
            }
        };
        if written == self.git.output_shown {
            return;
        }
        if !self.editor.refresh_output(title, more.as_deref(), &text) {
            return;
        }
        self.git.output_shown = written;
        self.relayout();
        self.request_redraw();
    }

    /// 打开的目录不是仓库：已经找过、没有找到，也没有读取失败。提交视图据此给出初始化和克隆。
    pub(crate) fn git_no_repository(&self) -> bool {
        self.editor.root().is_some()
            && self.git.root.is_none()
            && self.git.problem.is_none()
            && !self.git_scanning()
    }
}
