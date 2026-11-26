//! 可编辑 Markdown 的鼠标选区。原文命中使用字节偏移，文档选区使用字符偏移。

use winc_core::{Lines, Selection, movement};
use winc_editor::DocumentId;

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
    fn matches_doc(&self, document: DocumentId, revision: u64) -> bool {
        self.document == document && self.revision == revision
    }

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
            self.preview.markdown.live.clear_layout_caret();
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
                    .is_some_and(|drag| drag.matches_doc(doc.id, doc.revision)));
        if selecting && !composing {
            let at = preview.live.layout_caret_for(doc.id, doc.revision)?;
            return self.live_leaf_at(at);
        }
        let leaf = self.live_leaf_at(caret);
        if !selecting {
            if leaf.is_some() {
                self.preview
                    .markdown
                    .live
                    .set_layout_caret(doc.id, doc.revision, caret);
            } else {
                self.preview.markdown.live.clear_layout_caret();
            }
        }
        leaf
    }

    /// 单击定位、双击选词、三击选段。按住 ⇧ 单击时扩展已有选区。
    pub(super) fn click_live(&mut self, x: f32, y: f32, native_click_count: Option<u8>) {
        self.ime.preedit.clear();
        self.preview.markdown.selection = None;
        self.preview.markdown.live.clear_navigation();
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
                y >= preview.area.y + layout.body_bottom() - preview.scroll()
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

        self.preview.markdown.live.record_click(
            doc.id,
            source,
            upstream,
            target.position,
            doc.revision,
        );
        self.preview.markdown.live_drag = Some(LiveDrag {
            origin,
            unit,
            document: doc.id,
            revision: doc.revision,
            tail_click: tail.then_some((x, y)),
        });
        self.pointer.drag = Drag::MarkdownText;
        self.reset_blink();
    }

    /// 按发起拖动时的单位扩展选区。
    pub(super) fn drag_live(&mut self, x: f32, y: f32) {
        let Some(doc) = self.editor.active() else {
            return;
        };
        let (doc_id, doc_rev) = (doc.id, doc.revision);
        let Some(drag) = self
            .preview
            .markdown
            .live_drag
            .as_mut()
            .filter(|drag| drag.matches_doc(doc_id, doc_rev))
        else {
            return;
        };
        if let Some((a, b)) = drag.tail_click {
            if (x - a).abs().max((y - b).abs()) <= 2.0 {
                return;
            }
            drag.tail_click = None;
        }
        let drag = *drag;
        let Some(source) = self.preview.markdown.source_at(x, y) else {
            return;
        };
        let Some(target) = self.selection_target(source, drag.unit) else {
            return;
        };
        let upstream = drag.unit == SelectionUnit::Character && self.live_row_affinity(source, y);

        let Some(doc) = self.editor.active_mut() else {
            return;
        };
        self.preview
            .markdown
            .live
            .set_line_end(upstream, target.position, doc.revision);
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
        let fresh = preview.doc == Some(doc.id) && preview.checked == Some(doc.revision);
        let revealed_at = preview.live.layout_caret_for(doc.id, doc.revision);
        Some(winc_richtext::clipboard::operation_range(
            rope,
            selection,
            preview.parsed.as_deref(),
            fresh,
            revealed_at,
        ))
    }

    /// 点击正文时，选区从属性面板下方的原文开始。
    fn live_click_source(&self, x: f32, y: f32) -> Option<usize> {
        let body_start = self.property_body_start().and_then(|at| {
            let rope = self.editor.active()?.text().rope()?;
            Some(rope.char_to_byte(at))
        });
        self.preview
            .markdown
            .source_at(x, y)
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
