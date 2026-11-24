//! 可编辑 Markdown 的鼠标选区。原文命中使用字节偏移，文档选区使用字符偏移。

use winc_core::{Lines, Selection, movement};
use winc_editor::DocumentId;
use winc_preview::markdown::{clipboard, enter};

use super::laid;
use crate::app::{App, Drag};

#[derive(Clone, Copy, PartialEq, Eq)]
enum SelectionUnit {
    Character,
    Word,
    Paragraph,
    /// 段落边界尚未就绪时，本次手势按原文行扩选。
    Line,
}

struct SelectionTarget {
    position: usize,
    selection: Selection,
}

#[derive(Clone, Copy)]
pub(super) struct LiveDrag {
    origin: Selection,
    unit: SelectionUnit,
    document: DocumentId,
    revision: u64,
    tail_click: Option<(f32, f32)>,
}

impl LiveDrag {
    fn extend_to(&self, target: SelectionTarget) -> Selection {
        if self.unit != SelectionUnit::Character && target.position < self.origin.anchor {
            Selection::new(self.origin.head, target.selection.anchor)
        } else {
            Selection::new(self.origin.anchor, target.selection.head)
        }
    }
}

impl App {
    /// 选择期间沿用起选时的语法显示位置。输入法组字仍跟随实际光标。
    pub(crate) fn live_layout_leaf(&mut self) -> Option<super::LiveLeaf> {
        if !self.live_editing() {
            self.preview.markdown.layout_caret = None;
            return None;
        }
        let doc = self.editor.active()?;
        let caret = self.live_caret_source()?;
        let composing = !self.preedit_at(crate::app::Focus::Editor).is_empty();
        let preview = &self.preview.markdown;
        let selecting = !doc.selection.is_empty()
            || (self.pointer.drag == Drag::MarkdownText
                && preview
                    .live_drag
                    .is_some_and(|drag| drag.document == doc.id && drag.revision == doc.revision));
        if selecting && !composing {
            let (_, _, at) = preview
                .layout_caret
                .filter(|&(id, revision, _)| id == doc.id && revision == doc.revision)?;
            return self.live_leaf_at(at);
        }
        let leaf = self.live_leaf_at(caret);
        if !selecting {
            self.preview.markdown.layout_caret =
                leaf.as_ref().map(|_| (doc.id, doc.revision, caret));
        }
        leaf
    }

    /// 单击定位、双击选词、三击选段。按住 ⇧ 单击时扩展已有选区。
    pub(super) fn click_live(&mut self, x: f32, y: f32, native_click_count: Option<u8>) {
        self.ime.preedit.clear();
        self.preview.markdown.selection = None;
        self.preview.markdown.goal = None;
        self.preview.markdown.line_end = None;
        self.request_redraw();

        let count = if let Some(count) = native_click_count {
            self.pointer.last_click = None;
            count
        } else {
            self.click_count(x, y)
        };
        let tail = count == 1
            && self.pointer.modifiers.is_empty()
            && self.preview.markdown.layout.as_ref().is_some_and(|layout| {
                let preview = &self.preview.markdown;
                y >= preview.area.y + layout.height() - layout.pad - preview.scroll()
            });
        let source = if tail {
            self.editor
                .active()
                .and_then(|doc| doc.text().rope())
                .map(|rope| rope.len_bytes())
        } else {
            self.live_click_source(x, y)
        };
        let Some(source) = source else {
            self.start_live_editing();
            return;
        };
        let Some((unit, target)) = self.selection_from_click(source, count) else {
            return;
        };
        let shift = self.pointer.modifiers.shift_key();
        let upstream = count == 1 && self.live_row_affinity(source, y);

        let Some(doc) = self.editor.active_mut() else {
            return;
        };
        doc.goal_cell = None;
        doc.record_cursors();
        doc.collapse_selections();
        let origin = if unit == SelectionUnit::Character && shift {
            let anchor = doc.selection.anchor;
            doc.selection = Selection::new(anchor, target.position);
            Selection::point(anchor)
        } else {
            doc.selection = target.selection;
            target.selection
        };

        self.preview.markdown.line_end = upstream.then_some((target.position, doc.revision));
        self.preview.markdown.clicked = Some(source);
        self.preview.markdown.live_drag = Some(LiveDrag {
            origin,
            unit,
            document: doc.id,
            revision: doc.revision,
            tail_click: tail.then_some((x, y)),
        });
        self.pointer.drag = Drag::MarkdownText;
        self.start_live_editing();
        self.reset_blink();
    }

    /// 按发起拖动时的单位扩展选区。
    pub(super) fn drag_live(&mut self, x: f32, y: f32) {
        let Some(doc) = self.editor.active() else {
            return;
        };
        let Some(drag) = self
            .preview
            .markdown
            .live_drag
            .filter(|drag| drag.document == doc.id && drag.revision == doc.revision)
        else {
            return;
        };
        if let Some((a, b)) = drag.tail_click {
            if (x - a).abs().max((y - b).abs()) <= 2.0 {
                return;
            }
            if let Some(drag) = &mut self.preview.markdown.live_drag {
                drag.tail_click = None;
            }
        }
        let Some(source) = laid::source_at(self, x, y) else {
            return;
        };
        let Some(target) = self.selection_target(source, drag.unit) else {
            return;
        };
        let upstream = drag.unit == SelectionUnit::Character && self.live_row_affinity(source, y);

        let Some(doc) = self.editor.active_mut() else {
            return;
        };
        self.preview.markdown.line_end = upstream.then_some((target.position, doc.revision));
        doc.selection = drag.extend_to(target);
        self.reset_blink();
    }

    pub(in crate::app) fn finish_markdown_drag(&mut self) {
        let Some(drag) = self.preview.markdown.live_drag.take() else {
            return;
        };
        let Some(doc) = self
            .editor
            .active()
            .filter(|doc| doc.id == drag.document && doc.revision == drag.revision)
        else {
            return;
        };
        if drag.tail_click.is_some() && doc.selection.is_empty() {
            self.open_paragraph_below_body();
        }
        self.request_redraw();
    }

    /// 编辑和复制时补齐隐藏的语法。已展开的标记可以按实际选区单独编辑。
    pub(in crate::app) fn markdown_operation_range(&self) -> Option<std::ops::Range<usize>> {
        let doc = self.editor.active()?;
        let selection = doc.selection;
        if !self.markdown_live() || selection.is_empty() || doc.has_cursors() {
            return None;
        }
        let rope = doc.text().rope()?;
        let preview = &self.preview.markdown;
        let revealed_at = preview
            .layout_caret
            .filter(|&(id, revision, _)| id == doc.id && revision == doc.revision)
            .map(|(_, _, at)| at);
        let (from, to) = (
            rope.char_to_byte(selection.from()),
            rope.char_to_byte(selection.to()),
        );
        let (first, last) = (rope.byte_to_line(from), rope.byte_to_line(to));
        let (mut start, mut end) = (from, to);
        let Some(parsed) = preview
            .parsed
            .as_ref()
            .filter(|_| preview.checked == Some(doc.revision))
        else {
            return Some(selection.range());
        };
        let mut paragraph = None;
        for line in std::iter::once(first).chain((last != first).then_some(last)) {
            let text = rope.line(line);
            if text.len_bytes() > 32 * 1024 {
                continue;
            }
            let base = rope.line_to_byte(line);
            let text = text.to_string();
            let text = text.trim_end_matches(['\r', '\n']);
            let part = from.saturating_sub(base)..(to - base).min(text.len());
            if part.is_empty() {
                continue;
            }
            let leaf = [base + part.start, base + part.end - 1]
                .into_iter()
                .find_map(|at| {
                    super::live_structure::leaf_at_or_before(parsed, at)
                        .filter(|leaf| leaf.prose && leaf.range.contains(&at))
                });
            let Some(leaf) = leaf else {
                continue;
            };
            let range = &leaf.range;
            let multi_line =
                rope.byte_to_line(range.start) != rope.byte_to_line(range.end.saturating_sub(1));
            let relative_caret = |base: usize, len: usize| {
                revealed_at
                    .and_then(|at| at.checked_sub(base))
                    .filter(|&at| at <= len)
            };
            let mut expanded = if multi_line && range.len() <= 32 * 1024 {
                if paragraph != Some(range.start) {
                    let source = std::borrow::Cow::<str>::from(rope.byte_slice(range.clone()));
                    let selected =
                        from.saturating_sub(range.start)..(to - range.start).min(source.len());
                    let expanded = winc_preview::markdown::reveal::include_hidden_paragraph_markers(
                        &source,
                        selected,
                        relative_caret(range.start, source.len()),
                    );
                    start = start.min(range.start + expanded.start);
                    end = end.max(range.start + expanded.end);
                    paragraph = Some(range.start);
                }
                start.saturating_sub(base)..(end - base).min(text.len())
            } else {
                clipboard::include_hidden_markers(text, part, relative_caret(base, text.len()))
            };
            if let Some(marker) = enter::list_prefix(text)
                && expanded.start <= marker.end
                && expanded.end >= text.trim_end().len()
            {
                expanded.start = expanded.start.min(marker.start);
            }
            start = start.min(base + expanded.start);
            end = end.max(base + expanded.end);
        }
        Some(rope.byte_to_char(start)..rope.byte_to_char(end))
    }

    /// 点击正文时，选区从属性面板下方的原文开始。
    fn live_click_source(&self, x: f32, y: f32) -> Option<usize> {
        let body_start = self.property_body_start().and_then(|at| {
            let rope = self.editor.active()?.text().rope()?;
            Some(rope.char_to_byte(at))
        });
        laid::source_at(self, x, y)
            .map(|source| source.max(body_start.unwrap_or(0)))
            .or(body_start)
    }

    /// 在按下时决定整次手势使用的单位。
    fn selection_from_click(
        &self,
        source: usize,
        count: u8,
    ) -> Option<(SelectionUnit, SelectionTarget)> {
        let unit = match count {
            2 => SelectionUnit::Word,
            3 => SelectionUnit::Paragraph,
            _ => SelectionUnit::Character,
        };
        match self.selection_target(source, unit) {
            Some(target) => Some((unit, target)),
            None if unit == SelectionUnit::Paragraph => {
                let target = self.selection_target(source, SelectionUnit::Line)?;
                Some((SelectionUnit::Line, target))
            }
            None => None,
        }
    }

    /// 计算按下及拖动位置的选区。段落结构暂不可用时返回 `None`。
    fn selection_target(&self, source: usize, unit: SelectionUnit) -> Option<SelectionTarget> {
        let doc = self.editor.active()?;
        let rope = doc.text().rope()?;
        let source = source.min(rope.len_bytes());
        let position = rope.byte_to_char(source);
        let selection = match unit {
            SelectionUnit::Character => Selection::point(position),
            SelectionUnit::Word => {
                let (start, end) = movement::word_at(doc.text(), position);
                Selection::new(start, end)
            }
            SelectionUnit::Paragraph => self.paragraph_selection(doc, rope, source)?,
            SelectionUnit::Line => {
                let span = doc.text().line_span(rope.char_to_line(position));
                Selection::new(span.start, span.end)
            }
        };
        Some(SelectionTarget {
            position,
            selection,
        })
    }
}
