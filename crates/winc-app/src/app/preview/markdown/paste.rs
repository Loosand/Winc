//! Markdown 中粘贴与复制适配器：智能改写逻辑由 `winc_richtext::commands` 提供。

use winc_richtext::{PasteSpot, commands};

use super::*;

/// 粘贴处上下文（转调 `winc_richtext::PasteSpot`）。
pub(super) type Spot = PasteSpot;

impl App {
    /// 返回 Markdown 中粘贴 `content` 时改写成的文字；按普通方式粘贴时为 `None`。
    pub(in crate::app) fn markdown_paste(&self, content: &str) -> Option<String> {
        self.markdown_url_paste(content).or_else(|| {
            let doc = self.editor.active()?;
            let spot = self.prose_spot()?;
            commands::plan_paste_tsv(doc.selection, &spot, content, self.markdown_live())
        })
    }

    /// Pasting a bare URL over a selection keeps using the selected words as the label.
    pub(in crate::app) fn markdown_url_paste(&self, content: &str) -> Option<String> {
        let doc = self.editor.active()?;
        let rope = doc.text().rope()?;
        let rewritten = commands::plan_paste_url(rope, doc.selection, content)?;
        self.prose_spot()?;
        Some(rewritten)
    }

    /// 返回 Markdown 中粘贴剪贴板 HTML 时写成的 Markdown；HTML 没有结构时为 `None`。
    pub(in crate::app) fn markdown_html_paste(&mut self, has_plain: bool) -> Option<String> {
        let spot = self.prose_spot()?;
        let html = self.clipboard.html()?;
        commands::plan_paste_html(&spot, &html, has_plain, self.markdown_live())
    }

    /// Edit the visible selection and any hidden Markdown syntax it fully covers.
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

    /// Keep the unselected ends of a simple emphasis span intact across source lines.
    fn repair_markdown_emphasis(
        &self,
        doc: &winc_editor::Document,
        range: std::ops::Range<usize>,
        replace: &str,
    ) -> Option<winc_core::Transaction> {
        let rope = doc.text().rope()?;
        let tx = commands::plan_repair_emphasis(rope, doc.selection, range, replace, |byte| {
            self.live_prose_at(byte)
        })?;
        let change = tx.changes.first()?;
        let end = change.start + change.removed.chars().count();
        doc.fits_edit(change.start, end).then_some(tx)
    }

    /// Copy a single note selection without changing the note or caret.
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
        use winc_preview::markdown::clipboard;
        const HTML_BUDGET: usize = 512 * 1024;
        let operation = self.markdown_operation_range();
        let expanded = operation.clone().unwrap_or_else(|| sel.range());
        let (mut start, mut end) = (expanded.start, expanded.end);
        let line = rope.char_to_line(start);
        let end_line = rope.char_to_line(end);
        let parsed = self
            .preview
            .markdown
            .parsed
            .as_ref()
            .filter(|_| self.preview.markdown.checked == Some(doc.revision));
        let literal = parsed.is_some_and(|parsed| {
            let from = rope.char_to_byte(start);
            let to = rope.char_to_byte(end);
            let i = parsed
                .leaves
                .partition_point(|leaf| leaf.range.start <= from);
            i.checked_sub(1).is_some_and(|i| {
                let leaf = &parsed.leaves[i];
                !leaf.prose && !leaf.table && from < leaf.range.end && to <= leaf.range.end
            })
        });
        let mut copied_run = None;
        if !literal && end_line == line && rope.line(line).len_bytes() <= 32 * 1024 {
            let offset = rope.line_to_byte(line);
            let source = rope.line(line).to_string();
            let range = rope.char_to_byte(start) - offset..rope.char_to_byte(end) - offset;
            let expanded = if operation.is_some() {
                range.clone()
            } else {
                clipboard::include_markers(&source, range.clone())
            };
            if !markdown && expanded == range && (start..end) == sel.range() {
                copied_run = clipboard::formatted_run(&source, range);
            }
            start = rope.byte_to_char(offset + expanded.start);
            end = rope.byte_to_char(offset + expanded.end);
        } else if !literal && !markdown && end_line != line {
            let (a, b) = (rope.char_to_byte(start), rope.char_to_byte(end));
            let leaf_at = |at| {
                let leaves = &parsed?.leaves;
                let index = leaves.partition_point(|leaf| leaf.range.start <= at);
                let leaf = leaves.get(index.checked_sub(1)?)?;
                (leaf.prose && at <= leaf.range.end).then_some(&leaf.range)
            };
            let first = leaf_at(a).map_or(line, |r| rope.byte_to_line(r.start));
            let last = leaf_at(b).map_or(end_line, |r| rope.byte_to_line(r.end));
            let from = rope.line_to_byte(first);
            let to = rope.line_to_byte(last + 1);
            if to - from <= HTML_BUDGET {
                let context = std::borrow::Cow::<str>::from(rope.byte_slice(from..to));
                copied_run = clipboard::formatted_range(&context, a - from..b - from);
            }
        }
        let Some(source) = doc.text_range(start, end) else {
            self.flash(winc_i18n::t!(
                "editor-selection-too-large-to-copy",
                size = winc_editor::COPY_LIMIT >> 20
            ));
            return true;
        };
        let written = if markdown || literal || source.len() > HTML_BUDGET {
            self.clipboard.set_text(source, false)
        } else {
            let (plain, html) = copied_run.unwrap_or_else(|| clipboard::formatted(&source));
            if plain.is_empty() || !self.clipboard.set_formatted(html, plain.clone()) {
                self.clipboard
                    .set_text(if plain.is_empty() { source } else { plain }, false)
            } else {
                true
            }
        };
        if written {
            self.forget_remote_copy();
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
        let from = doc.selection.from().min(rope.len_chars());
        let (from, line) = (rope.char_to_byte(from), rope.char_to_line(from));
        let start = rope.line_to_byte(line);
        let before = rope.byte_slice(start..from).to_string();
        if before.matches('`').count() % 2 == 1 {
            return None;
        }
        if self.preview.markdown.checked == Some(doc.revision)
            && let Some(parsed) = self.preview.markdown.parsed.as_ref()
        {
            let i = parsed.leaves.partition_point(|l| l.range.start <= from);
            if let Some(leaf) = i.checked_sub(1).map(|i| &parsed.leaves[i])
                && leaf.range.contains(&from)
                && !leaf.prose
            {
                return None;
            }
        } else if in_fence(rope.lines_at(0).take(line)) {
            return None;
        }
        let to = doc.selection.to().min(rope.len_chars());
        let end_line = rope.char_to_line(to);
        let end = rope.line_to_byte((end_line + 1).min(rope.len_lines()));
        let after = rope.byte_slice(rope.char_to_byte(to)..end).to_string();
        let blank_above = line == 0 || rope.line(line - 1).to_string().trim().is_empty();
        Some(Spot {
            before,
            after,
            blank_above,
        })
    }
}
