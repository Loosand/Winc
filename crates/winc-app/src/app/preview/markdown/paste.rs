//! Markdown 中粘贴与复制适配器：智能改写、隐藏语法修复与复制载荷均由 `winc_richtext::clipboard` 计算。

use winc_richtext::{CopyPayload, PasteSpot, clipboard};

use super::*;

/// 粘贴处上下文（转调 `winc_richtext::PasteSpot`）。
pub(super) type Spot = PasteSpot;

impl App {
    /// 返回 Markdown 中粘贴 `content` 时改写成的文字；按普通方式粘贴时为 `None`。
    pub(in crate::app) fn markdown_paste(&self, content: &str) -> Option<String> {
        let doc = self.editor.active()?;
        let rope = doc.text().rope()?;
        let spot = self.prose_spot()?;
        clipboard::plan_paste_text(rope, doc.selection, &spot, content, self.markdown_live())
    }

    /// 选中文字后粘贴裸网址时将选中文字作为链接标签。
    pub(in crate::app) fn markdown_url_paste(&self, content: &str) -> Option<String> {
        let doc = self.editor.active()?;
        let rope = doc.text().rope()?;
        let rewritten = clipboard::plan_paste_url(rope, doc.selection, content)?;
        self.prose_spot()?;
        Some(rewritten)
    }

    /// 返回 Markdown 中粘贴剪贴板 HTML 时写成的 Markdown；HTML 没有结构时为 `None`。
    pub(in crate::app) fn markdown_html_paste(&mut self, has_plain: bool) -> Option<String> {
        let spot = self.prose_spot()?;
        let html = self.clipboard.html()?;
        clipboard::plan_paste_html(&spot, &html, has_plain, self.markdown_live())
    }

    /// 编辑可见选区及其完整覆盖的隐藏 Markdown 语法。
    pub(in crate::app) fn markdown_selection_replacement(
        &self,
        command: &winc_editor::Command,
        tab_width: usize,
    ) -> Option<winc_core::Transaction> {
        use winc_editor::Command;
        let replace = match command {
            Command::Insert(text) | Command::Paste(text) => text,
            Command::Backspace { .. } | Command::Delete { .. } => "",
            _ => return None,
        };
        let range = self.markdown_operation_range()?;
        let doc = self.editor.active()?;
        if let Some(repair) = self.repair_markdown_emphasis(doc, range.clone(), replace) {
            return Some(repair);
        }
        (range != doc.selection.range())
            .then(|| doc.transaction_with_range(command, tab_width, range))
            .flatten()
    }

    /// 跨行替换选区时修复两端未被选中的简单强调标记。
    fn repair_markdown_emphasis(
        &self,
        doc: &winc_editor::Document,
        range: std::ops::Range<usize>,
        replace: &str,
    ) -> Option<winc_core::Transaction> {
        let rope = doc.text().rope()?;
        let tx = clipboard::plan_repair_emphasis(rope, doc.selection, range, replace, |byte| {
            self.live_prose_at(byte)
        })?;
        let change = tx.changes.first()?;
        let end = change.start + change.removed.chars().count();
        doc.fits_edit(change.start, end).then_some(tx)
    }

    /// 复制笔记中的单选区（不修改原文或光标）。
    pub(in crate::app) fn copy_note_selection(&mut self, markdown: bool) -> bool {
        let Some(doc) = self.editor.active() else {
            return false;
        };
        let Some(rope) = doc.text().rope() else {
            return false;
        };
        let sel = doc.selection;
        if doc.preview_kind() != Some(PreviewKind::Markdown) || sel.is_empty() || doc.has_cursors()
        {
            return false;
        }
        let fresh = self.preview.markdown.doc == Some(doc.id)
            && self.preview.markdown.checked == Some(doc.revision);
        let operation = self.markdown_operation_range();
        let Some(payload) = clipboard::plan_copy(
            rope,
            sel,
            self.preview.markdown.parsed.as_deref(),
            fresh,
            operation,
            markdown,
            winc_editor::COPY_LIMIT,
        ) else {
            self.flash(winc_i18n::t!(
                "editor-selection-too-large-to-copy",
                size = winc_editor::COPY_LIMIT >> 20
            ));
            return true;
        };
        let written = match payload {
            CopyPayload::Plain(text) => self.clipboard.set_text(text, false),
            CopyPayload::Formatted { html, plain } => {
                self.clipboard.set_formatted(html, plain.clone())
                    || self.clipboard.set_text(plain, false)
            }
        };
        if written {
            self.flash(winc_i18n::t!("editor-selection-copied"));
        } else {
            self.flash(winc_i18n::t!("editor-paste-clipboard-unavailable"));
        }
        true
    }

    /// 原样插入剪贴板中的文字，不改写为 Markdown 的写法。
    pub(in crate::app) fn paste_plain(&mut self) {
        if let Some(text) = self.clipboard_text() {
            self.run(winc_editor::Command::Paste(text));
        }
    }

    /// 返回光标处的粘贴处；不是 Markdown 文档、有多个选区，或光标在代码块、公式、HTML 块、
    /// 行内代码中时为 `None`。
    pub(super) fn prose_spot(&self) -> Option<Spot> {
        let doc = self.editor.active()?;
        if doc.preview_kind() != Some(PreviewKind::Markdown) || !doc.extra_selections().is_empty() {
            return None;
        }
        let rope = doc.text().rope()?;
        let fresh = self.preview.markdown.doc == Some(doc.id)
            && self.preview.markdown.checked == Some(doc.revision);
        clipboard::prose_spot(
            rope,
            doc.selection,
            self.preview.markdown.parsed.as_deref(),
            fresh,
        )
    }
}
