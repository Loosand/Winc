//! Markdown 中粘贴时改写为对应的写法：网址、图片网址、表格单元格和 HTML 内容。

use super::*;

/// 粘贴处：光标所在行中光标之前、之后的文字，以及上一行是否为空（或不存在）。
pub(super) struct Spot {
    before: String,
    after: String,
    blank_above: bool,
}

impl Spot {
    /// Return the inserted block and its separators in one edit. A table must leave
    /// a new prose line at the caret even when there is no existing text to its right.
    fn block(&self, text: &str, continue_after: bool) -> String {
        let lead = if !self.before.trim().is_empty() {
            "\n\n"
        } else if !self.blank_above {
            "\n"
        } else {
            ""
        };
        let text = text.trim_end_matches('\n');
        let tail = if continue_after || !self.after.trim().is_empty() {
            "\n\n"
        } else {
            ""
        };
        format!("{lead}{text}{tail}")
    }
}

impl App {
    /// 返回 Markdown 中粘贴 `content` 时改写成的文字；按普通方式粘贴时为 `None`。
    ///
    /// - 选中一行之内的文字后粘贴网址：`[文字](网址)`；选中的文字本身是网址时不改写。
    /// - 没有选区时粘贴图片网址：`![](网址)`。
    /// - 粘贴表格单元格（见 `cells::from_tsv`）：单独成块的 GFM 表格。
    pub(in crate::app) fn markdown_paste(&self, content: &str) -> Option<String> {
        self.markdown_url_paste(content).or_else(|| {
            let doc = self.editor.active()?;
            if !doc.selection.is_empty() || !content.contains('\t') {
                return None;
            }
            let spot = self.prose_spot()?;
            let table = winc_preview::markdown::cells::from_tsv(content)?;
            Some(spot.block(&table, self.markdown_live()))
        })
    }

    /// Pasting a bare URL over a selection keeps using the selected words as the label.
    pub(in crate::app) fn markdown_url_paste(&self, content: &str) -> Option<String> {
        let doc = self.editor.active()?;
        let is_url = super::writing::is_url;
        let url = content.trim();
        if !is_url(url) {
            return None;
        }
        let rope = doc.text().rope()?;
        let chars = rope.len_chars();
        let (from, to) = (
            doc.selection.from().min(chars),
            doc.selection.to().min(chars),
        );
        let selected = rope.slice(from..to).to_string();
        let link =
            !selected.is_empty() && !selected.contains(['\n', '`']) && !is_url(selected.trim());
        let image = selected.is_empty() && super::writing::is_image_url(url);
        if !link && !image {
            return None;
        }
        self.prose_spot()?;
        let text = if image { "" } else { selected.as_str() };
        Some(winc_preview::markdown::paragraph::link(text, Some(url), image).0)
    }

    /// 返回 Markdown 中粘贴剪贴板 HTML 时写成的 Markdown；HTML 没有结构时为 `None`。
    ///
    /// 只有一段文字时就地插入，有多块时单独成块。
    pub(in crate::app) fn markdown_html_paste(&mut self, has_plain: bool) -> Option<String> {
        let spot = self.prose_spot()?;
        let html = self.clipboard.html()?;
        let converted = winc_preview::markdown::from_html::from_html(&html);
        let body = converted.text.trim_end();
        if body.is_empty() || (!converted.rich && has_plain) {
            return None;
        }
        let starts_block = ["#", "- ", "> ", "|", "```", "---", "!["]
            .iter()
            .any(|p| body.starts_with(p))
            || body
                .split_once(". ")
                .is_some_and(|(n, _)| n.chars().all(|c| c.is_ascii_digit()));
        if !body.contains('\n') && !starts_block {
            return Some(body.to_owned());
        }
        // The HTML writer ends each top-level table row with a pipe. Use that
        // cheap check before asking the parser whether the last block is a table.
        let end_in_prose = self.markdown_live()
            && spot.after.trim().is_empty()
            && body
                .lines()
                .next_back()
                .is_some_and(|line| line.starts_with('|') && line.ends_with('|'))
            && matches!(
                markdown::parse(body).blocks.last(),
                Some(markdown::Block::Table(..))
            );
        Some(spot.block(&converted.text, end_in_prose))
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
        let mut start = sel.from();
        let mut end = sel.to();
        let line = rope.char_to_line(start);
        let literal = self.preview.markdown.checked == Some(doc.revision)
            && self.preview.markdown.parsed.as_ref().is_some_and(|parsed| {
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
        if !literal && rope.char_to_line(end) == line && rope.line(line).len_bytes() <= 32 * 1024 {
            let offset = rope.line_to_byte(line);
            let source = rope.line(line).to_string();
            let range = rope.char_to_byte(start) - offset..rope.char_to_byte(end) - offset;
            let expanded = clipboard::include_markers(&source, range.clone());
            if !markdown && expanded == range {
                copied_run = clipboard::formatted_run(&source, range);
            }
            start = rope.byte_to_char(offset + expanded.start);
            end = rope.byte_to_char(offset + expanded.end);
        }
        let Some(source) = doc.text_range(start, end) else {
            self.flash(winc_i18n::t!(
                "editor-selection-too-large-to-copy",
                size = winc_editor::COPY_LIMIT >> 20
            ));
            return true;
        };
        // Bound synchronous HTML conversion; large selections still copy the original source.
        let written = if markdown || literal || source.len() > 512 * 1024 {
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
        // 该行光标之前的反引号数量为奇数时位于行内代码中。
        if before.matches('`').count() % 2 == 1 {
            return None;
        }
        // 有当前版本的解析结果时按其判断所在的块，否则只按之前的围栏行判断代码块。
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
