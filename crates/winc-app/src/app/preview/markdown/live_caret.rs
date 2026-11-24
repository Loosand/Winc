//! 可编辑 Markdown 的光标移动、折行归属和视口跟随。

use winc_core::{Selection, movement};

use super::{laid, line_end};
use crate::app::{App, ScrollTarget};

impl App {
    /// 可编辑的预览中文字光标的原文位置（字节偏移，按当前文字计算）。
    pub(crate) fn live_caret_source(&self) -> Option<usize> {
        let doc = self.editor.active()?;
        let rope = doc.text().rope()?;
        Some(rope.char_to_byte(doc.selection.head.min(rope.len_chars())))
    }

    /// ←→ 略过不显示的块标记和块间分隔行。普通文字和可编辑的空行交给编辑区处理。
    pub(super) fn live_horizontal(&mut self, right: bool) -> bool {
        self.preview.markdown.line_end = None;
        let extend = self.pointer.modifiers.shift_key();
        let Some(doc) = self.editor.active() else {
            return false;
        };
        let sel = doc.selection;
        if !sel.is_empty() && !extend {
            return false;
        }
        // 和文档层用同一个字素边界，包括跨过整组 CRLF。
        let next = if right {
            movement::next_grapheme(doc.text(), sel.head)
        } else {
            movement::prev_grapheme(doc.text(), sel.head)
        };
        let target = self.live_visible(next, right);
        if target == next || next == sel.head {
            return false;
        }
        if let Some(doc) = self.editor.active_mut() {
            doc.goal_cell = None;
            doc.collapse_selections();
            doc.selection = doc.selection.moved_to(target, extend);
        }
        self.preview.markdown.goal = None;
        self.start_live_editing();
        self.reset_blink();
        self.request_redraw();
        true
    }

    /// 光标落在块标记或分隔行时，返回移动方向上最近的可见位置（字符偏移）。
    ///
    /// 向后（`forward`）时取该行内容的开头；向前时优先停在上方可编辑的空行，否则取上一行文字的末尾。
    pub(super) fn live_visible(&self, pos: usize, forward: bool) -> usize {
        let Some(doc) = self.editor.active() else {
            return pos;
        };
        let Some(rope) = doc.text().rope() else {
            return pos;
        };
        let body_start = self.property_body_start().unwrap_or(0);
        let mut pos = pos.max(body_start).min(rope.len_chars());
        let preview = &self.preview.markdown;
        let current = preview.doc == Some(doc.id) && preview.checked == Some(doc.revision);
        if current
            && let Some(separator) = preview
                .parsed
                .as_ref()
                .and_then(|md| md.collapsed_separator_at(rope.char_to_byte(pos)))
        {
            if !forward {
                let above = rope.byte_to_line(separator.start) - 1;
                return line_end(rope, above).max(body_start);
            }
            pos = rope.byte_to_char(separator.end);
        }
        let line = rope.char_to_line(pos);
        let mapped = |line| {
            current
                && preview
                    .layout
                    .as_ref()
                    .is_some_and(|laid| laid.blank_point(rope.line_to_byte(line)).is_some())
        };
        if mapped(line) {
            return pos;
        }
        let start = rope.line_to_char(line);
        let hidden = self.live_hidden_prefix(line);
        if pos - start >= hidden {
            return pos;
        }
        if !forward {
            let visible = |l| mapped(l) || !rope.line(l).chars().all(char::is_whitespace);
            if let Some(above) = (0..line).rev().find(|&l| visible(l)) {
                return line_end(rope, above).max(body_start);
            }
        }
        start + hidden
    }

    /// 编辑区的命令移动光标之后调用：光标落在不显示的块标记中时移到可见位置。
    ///
    /// `before` 是移动之前光标的位置（字符偏移），据此判断移动方向。按词移动、移到行首等
    /// 命令按原文计算位置，结果可能落在列表记号或续行的缩进中。
    pub(in crate::app) fn live_settle_caret(&mut self, before: usize) {
        if !self.markdown_live() {
            return;
        }
        self.parse_markdown_inline();
        let Some(doc) = self.editor.active() else {
            return;
        };
        let sel = doc.selection;
        if sel.head == before || !doc.extra_selections().is_empty() {
            return;
        }
        let head = self.live_visible(sel.head, sel.head > before);
        if head == sel.head {
            return;
        }
        if let Some(doc) = self.editor.active_mut() {
            let anchor = if sel.is_empty() { head } else { sel.anchor };
            doc.selection = Selection::new(anchor, head);
        }
        self.request_redraw();
    }

    /// 将光标移到所在显示行的开头或末尾（`end`），按住 ⇧ 时扩展选区。
    ///
    /// 正文按预览中折行后的行计算，一段原文折成多行时不会移到整段的开头、末尾。光标位于
    /// 代码块、公式等按原文显示的块中时返回 `false`，按编辑区的方式处理。
    pub(super) fn live_line_edge(&mut self, end: bool) -> bool {
        if self.live_leaf().is_none_or(|l| !l.prose && !l.table) {
            return false;
        }
        let extend = self.pointer.modifiers.shift_key();
        let Some(source) = self.live_caret_source() else {
            return false;
        };
        let Some(laid) = self.preview.markdown.layout.as_ref() else {
            return false;
        };
        let Some((_, y, h)) =
            laid.caret_point_with_affinity(source, self.live_upstream() == Some(source))
        else {
            return false;
        };
        // 横坐标取内容一栏之外：命中的是该行最左、最右的字缝。
        let x = if end {
            laid.left + laid.width() + self.preview.markdown.area.w
        } else {
            laid.left - self.preview.markdown.area.w
        };
        let sy = self.preview.markdown.area.y - self.preview.markdown.scroll() + y + h / 2.0;
        let Some(found) = laid::source_at(self, x, sy) else {
            return false;
        };
        let wrapped = end && self.live_row_affinity(found, sy);
        let Some(doc) = self.editor.active_mut() else {
            return false;
        };
        let Some(rope) = doc.text().rope() else {
            return false;
        };
        let pos = rope.byte_to_char(found.min(rope.len_bytes()));
        let revision = doc.revision;
        doc.goal_cell = None;
        doc.collapse_selections();
        doc.selection = doc.selection.moved_to(pos, extend);
        self.preview.markdown.line_end = wrapped.then_some((pos, revision));
        self.preview.markdown.goal = None;
        self.start_live_editing();
        self.reset_blink();
        self.request_redraw();
        true
    }

    /// 原文位置 `source` 位于折行处时，判断窗口纵坐标 `y` 是否更接近上一显示行。
    ///
    /// 折行处的位置同时是上一行的末尾和下一行的开头：在上一行的右端点击、从上方按 ↓ 移到
    /// 行尾时，光标应留在上一行。
    pub(super) fn live_row_affinity(&self, source: usize, y: f32) -> bool {
        let Some(laid) = self.preview.markdown.layout.as_ref() else {
            return false;
        };
        let y = y - (self.preview.markdown.area.y - self.preview.markdown.scroll());
        let far = |p: Option<(f32, f32, f32)>| p.map(|(_, top, h)| (top + h / 2.0 - y).abs());
        let below = laid.caret_point_with_affinity(source, false);
        let above = laid.caret_point_with_affinity(source, true);
        match (far(above), far(below)) {
            (Some(a), Some(b)) => a + 0.5 < b,
            _ => false,
        }
    }

    /// 可编辑的预览中光标应画在上一显示行末尾时，返回光标的原文位置（字节偏移），见
    /// `MarkdownPreview::line_end`。
    pub(crate) fn live_upstream(&self) -> Option<usize> {
        let doc = self.editor.active()?;
        let (pos, revision) = self.preview.markdown.line_end?;
        (doc.selection.head == pos && doc.revision == revision)
            .then(|| self.live_caret_source())
            .flatten()
    }

    /// 将光标向下（`down`）或向上移动一个显示行，或 `by` 逻辑像素；移出第一行、最后一行时
    /// 移到文档的开头、末尾。
    pub(super) fn live_vertical(&mut self, down: bool, by: Option<f32>) {
        let extend = self.pointer.modifiers.shift_key();
        let Some(source) = self.live_caret_source() else {
            return;
        };
        let Some(laid) = self.preview.markdown.layout.as_ref() else {
            return;
        };
        let Some((x, y, h)) =
            laid.caret_point_with_affinity(source, self.live_upstream() == Some(source))
        else {
            return;
        };
        let head = self.editor.active().map_or(0, |d| d.selection.head);
        let goal = match self.preview.markdown.goal {
            Some((goal, at)) if at == head => goal,
            _ => x,
        };
        let step = by.unwrap_or(h);
        let sign = if down { 1.0 } else { -1.0 };
        let height = laid.height();
        let screen_x = laid.left + goal;
        let screen_top = self.preview.markdown.area.y - self.preview.markdown.scroll();
        // 块间留白高于一行时，下一行的目标位置落在留白中，取到的最近一行仍是原来的行；
        // 因此每次再前进半行，直到到达其他行或超出内容范围。
        let center = y + h / 2.0;
        let mut target = center + sign * step;
        let moved = loop {
            let probe = if by.is_none() {
                laid.limit_blank_step(center, target)
            } else {
                target
            };
            if !(0.0..=height).contains(&probe) {
                break None;
            }
            let screen_y = screen_top + probe;
            let found = laid::source_at(self, screen_x, screen_y).and_then(|source| {
                let upstream = self.live_row_affinity(source, screen_y);
                let point = laid.caret_point_with_affinity(source, upstream);
                point
                    .is_none_or(|(_, row_y, _)| (row_y - y).abs() >= 0.5)
                    .then_some((source, upstream))
            });
            if found.is_some() || by.is_some() || !(0.0..=height).contains(&target) {
                break found;
            }
            target += sign * h / 2.0;
        };
        // 文档以代码块、表格、分隔线等结束：再向下时在其后另起一段，光标有处可去。
        if moved.is_none() && down && !extend && self.live_append_paragraph() {
            return;
        }
        // 仍在原来的行上（已是第一行或最后一行）时，移到可编辑正文的开头、末尾。
        let body_start = self.property_body_start().unwrap_or(0);
        let Some(doc) = self.editor.active_mut() else {
            return;
        };
        let Some(rope) = doc.text().rope() else {
            return;
        };
        let pos = match moved {
            Some((source, _)) => rope.byte_to_char(source.min(rope.len_bytes())),
            None if down => rope.len_chars(),
            None => body_start,
        }
        .max(body_start);
        doc.goal_cell = None;
        doc.collapse_selections();
        doc.selection = doc.selection.moved_to(pos, extend);
        let revision = doc.revision;
        self.preview.markdown.line_end = moved
            .is_some_and(|(_, upstream)| upstream)
            .then_some((pos, revision));
        self.preview.markdown.goal = Some((goal, pos));
        self.preview.markdown.selection = None;
        self.start_live_editing();
        self.reset_blink();
        self.request_redraw();
    }

    /// 可编辑的预览在本帧是否将光标所在的块显示为原文。
    ///
    /// 在预览上点击、移动光标或输入之后才显示原文；刚打开、仅阅读时整篇均按排版显示，
    /// 不会有某一块突然切换为原文。排版之前调用。
    pub(crate) fn live_editing(&mut self) -> bool {
        if !self.markdown_live() {
            return false;
        }
        let (Some(source), Some(doc)) = (self.live_caret_source(), self.editor.active()) else {
            return false;
        };
        let now = (doc.id, source, doc.revision);
        if self
            .preview
            .markdown
            .followed
            .is_some_and(|f| f.0 == doc.id && f != now)
        {
            self.preview.markdown.editing = Some(doc.id);
        }
        self.preview.markdown.editing == Some(doc.id)
    }

    /// 可编辑的预览中是否显示文字光标：在当前文档的预览上点击、移动光标或输入之后显示
    /// （见 [`App::live_editing`]）。
    pub(crate) fn live_caret_shown(&self) -> bool {
        let id = self.editor.active().map(|d| d.id);
        id.is_some() && self.preview.markdown.editing == id
    }

    /// 在可编辑的预览中开始编辑（点击、移动光标、组字）：光标所在的块显示原文。
    pub(in crate::app) fn start_live_editing(&mut self) {
        self.preview.markdown.editing = self.editor.active().map(|d| d.id);
    }

    /// 可编辑的预览中光标位置改变（输入、点击、移动、查找）后，滚动使光标可见并与边缘保留
    /// 一行的距离；光标未移动、仅滚动时不处理。
    ///
    /// 排版完成之后、绘制之前调用。
    pub(crate) fn follow_live_caret(&mut self) {
        let source = self
            .markdown_live()
            .then(|| self.live_caret_source())
            .flatten();
        let (Some(source), Some(doc)) = (source, self.editor.active()) else {
            self.preview.markdown.followed = None;
            return;
        };
        let now = (doc.id, source, doc.revision);
        let moved = self
            .preview
            .markdown
            .followed
            .is_some_and(|f| f.0 == doc.id && f != now);
        self.preview.markdown.followed = Some(now);
        if !moved || self.preview.markdown.editing != Some(doc.id) {
            return;
        }
        // 全选不滚动：选区的终点在文末，滚动过去会离开正在阅读的位置。
        let chars = doc.text().rope().map_or(0, ropey::Rope::len_chars);
        if doc.selection.from() == 0 && doc.selection.to() >= chars && chars > 0 {
            return;
        }
        let Some(laid) = self.preview.markdown.layout.as_ref() else {
            return;
        };
        let Some((_, y, h)) = laid.caret_point(source) else {
            return;
        };
        if self.markdown_typewriter() && self.scroll.keyed {
            // 打字机模式：平滑地滚到光标所在的行停在中间。
            let view = self.preview.markdown.area.h;
            let to = (y + h / 2.0 - view / 2.0).max(0.0);
            if (to - self.preview.markdown.scroll()).abs() >= 1.0 {
                self.glide_pane_to(ScrollTarget::Markdown, to);
            }
            return;
        }
        // 用键盘移动时离边缘留一行，点击时只在被挡住时滚动。
        let keyed = self.scroll.keyed;
        let md = &mut self.preview.markdown;
        let (scroll, view) = (md.scroll(), md.area.h);
        let margin = if keyed { h.min(view / 4.0) } else { 0.0 };
        if y - margin < scroll {
            md.scroll_to(y - margin);
        } else if y + h + margin > scroll + view {
            md.scroll_to(y + h + margin - view);
        }
    }
}
