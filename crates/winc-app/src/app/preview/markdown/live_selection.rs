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
    /// 稳定拖选期间的排版。按下前还没进入编辑时为 `None`。
    caret_before: Option<usize>,
    selection_before: Option<Selection>,
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
    /// 按住鼠标时沿用按下前的光标，避免标记显隐改变鼠标下的排版。
    pub(super) fn live_layout_caret(&self) -> Option<usize> {
        match self.preview.markdown.live_drag {
            Some(drag) if self.pointer.drag == Drag::MarkdownText => drag.caret_before,
            _ => self.live_caret_source(),
        }
    }

    pub(super) fn live_layout_selection(&self) -> Option<Selection> {
        let doc = self.editor.active()?;
        match self.preview.markdown.live_drag {
            Some(drag)
                if self.pointer.drag == Drag::MarkdownText
                    && drag.document == doc.id
                    && drag.revision == doc.revision =>
            {
                drag.selection_before
            }
            _ => Some(doc.selection),
        }
        .filter(|selection| !selection.is_empty())
    }

    /// 单击定位、双击选词、三击选段。按住 ⇧ 单击时扩展已有选区。
    pub(super) fn click_live(&mut self, x: f32, y: f32, native_click_count: Option<u8>) {
        let editing = self.editor.active().map(|doc| doc.id) == self.preview.markdown.editing;
        let caret_before = self.live_caret_source().filter(|_| editing);
        let selection_before = caret_before.and_then(|_| self.live_layout_selection());
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
            caret_before,
            selection_before,
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
        } else if matches!(
            drag.unit,
            SelectionUnit::Character | SelectionUnit::Paragraph
        ) && !doc.selection.is_empty()
            && let Some(rope) = doc.text().rope()
        {
            let selection = self.include_selection_markers(rope, doc.selection);
            if let Some(doc) = self.editor.active_mut() {
                doc.selection = selection;
            }
        }
    }

    /// 用原文补齐选区端点被排版隐藏的标记，整条列表项包含它的行首记号。
    fn include_selection_markers(&self, rope: &ropey::Rope, selection: Selection) -> Selection {
        let (from, to) = (
            rope.char_to_byte(selection.from()),
            rope.char_to_byte(selection.to()),
        );
        let (first, last) = (rope.byte_to_line(from), rope.byte_to_line(to));
        let (mut start, mut end) = (from, to);
        for line in std::iter::once(first).chain((last != first).then_some(last)) {
            let text = rope.line(line);
            if text.len_bytes() > 32 * 1024 {
                continue;
            }
            let base = rope.line_to_byte(line);
            let text = text.to_string();
            let text = text.trim_end_matches(['\r', '\n']);
            let part = from.saturating_sub(base)..(to - base).min(text.len());
            if part.is_empty() || self.live_prose_at(base + part.start) != Some(true) {
                continue;
            }
            let mut expanded = clipboard::include_markers(text, part);
            if let Some(marker) = enter::list_prefix(text)
                && expanded.start <= marker.end
                && expanded.end >= text.trim_end().len()
            {
                expanded.start = expanded.start.min(marker.start);
            }
            start = start.min(base + expanded.start);
            end = end.max(base + expanded.end);
        }
        let (start, end) = (rope.byte_to_char(start), rope.byte_to_char(end));
        if selection.anchor <= selection.head {
            Selection::new(start, end)
        } else {
            Selection::new(end, start)
        }
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
            SelectionUnit::Paragraph => {
                let selection = self.paragraph_selection(doc, rope, source)?;
                self.include_selection_markers(rope, selection)
            }
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
