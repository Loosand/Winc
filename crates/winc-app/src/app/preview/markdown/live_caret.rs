//! 可编辑 Markdown 的光标移动、折行归属和视口跟随。

use winc_core::{Selection, movement};
use winc_editor::DocumentId;

use super::line_end;
use crate::app::{App, ScrollTarget};

/// 刚插入的可编辑空段（光标离开或按退格时自动收回）。
struct EmptyParagraph {
    document: DocumentId,
    revision: u64,
    /// 本次回车插入的原文范围（字符偏移），包括光标后的分隔换行。
    inserted: std::ops::Range<usize>,
    caret: usize,
}

/// 可编辑 Markdown 预览的交互与导航状态。
#[derive(Default)]
pub(crate) struct LiveState {
    /// 最近一次用于单光标排版的位置，拖选和键盘扩选时继续使用。
    layout_caret: Option<(DocumentId, u64, usize)>,
    /// 回车刚创建的空段，按退格时可整段收回。
    empty_paragraph: Option<EmptyParagraph>,
    /// ↑↓ 移动的目标横坐标（相对内容左边）和移动后光标位置（字符偏移）。
    goal: Option<(f32, usize)>,
    /// 折行处光标位于上一行末尾的归属标记（字符偏移、文档版本）。
    line_end: Option<(usize, u64)>,
    /// 表格工具刚修改的文档版本：后台排版接手时保持页面滚动位置。
    table_edit_revision: Option<(DocumentId, u64)>,
    /// 上一帧跟踪的文档、光标原文位置与版本号。
    followed: Option<(DocumentId, usize, u64)>,
    /// 已进入编辑态的文档（光标所在块显示原文）。
    editing: Option<DocumentId>,
    /// 最近一次单击放下的光标（原文字节偏移）。
    clicked: Option<usize>,
}

impl LiveState {
    /// 切换文档时清除仅对当前文档有效的编辑辅助状态。
    pub(super) fn reset_doc(&mut self) {
        self.empty_paragraph = None;
        self.table_edit_revision = None;
    }

    /// 离开预览模式时清除编辑态与光标跟随记录。
    pub(super) fn leave_mode(&mut self) {
        self.followed = None;
        self.editing = None;
    }

    /// 清除竖直移动目标列与折行行尾归属。
    pub(super) fn clear_navigation(&mut self) {
        self.goal = None;
        self.line_end = None;
    }

    /// 结束一次编辑命令：重置目标列并标记当前文档进入实时编辑态。
    pub(super) fn finish_edit(&mut self, doc: Option<DocumentId>, clear_line_end: bool) {
        self.goal = None;
        if clear_line_end {
            self.line_end = None;
        }
        self.editing = doc;
    }

    /// 记录水平/拖动移动后的折行行尾归属。
    pub(super) fn set_line_end(&mut self, upstream: bool, pos: usize, revision: u64) {
        self.line_end = upstream.then_some((pos, revision));
    }

    /// 完成行首/行尾跳转：写入折行归属、清空目标列并进入编辑态。
    fn finish_line_end_move(
        &mut self,
        doc: Option<DocumentId>,
        wrapped: bool,
        pos: usize,
        revision: u64,
    ) {
        self.line_end = wrapped.then_some((pos, revision));
        self.goal = None;
        self.editing = doc;
    }

    /// 完成竖直方向移动：更新折行归属、目标横坐标并进入编辑态。
    fn finish_vertical_move(
        &mut self,
        doc: Option<DocumentId>,
        upstream: bool,
        pos: usize,
        revision: u64,
        goal: f32,
    ) {
        self.line_end = upstream.then_some((pos, revision));
        self.goal = Some((goal, pos));
        self.editing = doc;
    }

    /// 记录单击落点与折行行尾归属，同时标记文档进入实时编辑态。
    pub(super) fn record_click(
        &mut self,
        doc: DocumentId,
        source: usize,
        upstream: bool,
        pos: usize,
        revision: u64,
    ) {
        self.line_end = upstream.then_some((pos, revision));
        self.clicked = Some(source);
        self.editing = Some(doc);
    }

    /// 清除单光标排版锚点。
    pub(super) fn clear_layout_caret(&mut self) {
        self.layout_caret = None;
    }

    /// 记录单光标排版锚点。
    pub(super) fn set_layout_caret(&mut self, id: DocumentId, revision: u64, at: usize) {
        self.layout_caret = Some((id, revision, at));
    }

    /// 若单光标排版锚点仍属于当前文档与版本，返回其原文字节偏移。
    pub(super) fn layout_caret_for(&self, id: DocumentId, revision: u64) -> Option<usize> {
        self.layout_caret
            .filter(|&(doc_id, rev, _)| doc_id == id && rev == revision)
            .map(|(_, _, at)| at)
    }

    /// 记录表格工具修改的文档版本。
    pub(super) fn record_table_edit(&mut self, id: DocumentId, revision: u64) {
        self.table_edit_revision = Some((id, revision));
    }

    /// 重排前检查是否需要保持该光标在屏幕上的位置不动，同时消费掉单次点击标记。
    fn allow_caret_anchor(
        &mut self,
        caret: usize,
        keyed: bool,
        source_changed: bool,
        doc: Option<(DocumentId, u64)>,
    ) -> bool {
        let clicked = self.clicked.take();
        if clicked == Some(caret) && !keyed {
            return false;
        }
        if !source_changed {
            return true;
        }
        let Some((id, revision)) = doc else {
            return false;
        };
        self.table_edit_revision != Some((id, revision))
            && self.followed == Some((id, caret, revision))
    }

    /// 判断当前光标位置是否命中上一显示行末尾归属。
    fn is_upstream(&self, head: usize, revision: u64) -> bool {
        self.line_end == Some((head, revision))
    }

    /// 计算本次竖直移动应沿用的目标横坐标。
    fn goal_x_for(&self, head: usize, fallback_x: f32) -> f32 {
        match self.goal {
            Some((goal, at)) if at == head => goal,
            _ => fallback_x,
        }
    }

    /// 当前活动文档是否应显示实时编辑光标。
    pub(super) fn caret_shown_for(&self, doc: Option<DocumentId>) -> bool {
        doc.is_some() && self.editing == doc
    }

    /// 显式将文档置入实时编辑态；切换文档时同步清理过期的表格编辑版本标记。
    fn begin_editing(&mut self, doc: Option<DocumentId>) {
        if self.editing != doc {
            self.table_edit_revision = None;
        }
        self.editing = doc;
    }

    /// 排版前同步编辑态：若光标或文档版本相对上一帧发生变化，则自动进入实时编辑态。
    fn sync_editing(&mut self, id: DocumentId, source: usize, revision: u64) -> bool {
        let now = (id, source, revision);
        if self.followed.is_some_and(|f| f.0 == id && f != now) {
            self.editing = Some(id);
        }
        self.editing == Some(id)
    }

    /// 排版后更新光标跟随记录，返回本帧是否发生了需要滚动跟随的光标移动。
    fn advance_followed(&mut self, current: Option<(DocumentId, usize, u64)>) -> bool {
        let Some(now) = current else {
            self.followed = None;
            return false;
        };
        let moved = self.followed.is_some_and(|f| f.0 == now.0 && f != now);
        self.followed = Some(now);
        moved && self.editing == Some(now.0)
    }

    /// 编程改写源码（任务框勾选、图片缩放）后保持原有编辑态并同步光标跟踪。
    pub(super) fn sync_after_programmatic_edit(
        &mut self,
        was_editing: bool,
        clear_editing: bool,
        current: Option<(DocumentId, usize, u64)>,
    ) {
        if !was_editing {
            if clear_editing {
                self.editing = None;
            }
            if current.is_some() || clear_editing {
                self.followed = current;
            }
        }
    }

    /// 记录刚刚由回车或文末点击创建的可编辑空段。
    fn remember_empty(
        &mut self,
        document: DocumentId,
        revision: u64,
        inserted: std::ops::Range<usize>,
        caret: usize,
    ) {
        self.empty_paragraph = Some(EmptyParagraph {
            document,
            revision,
            inserted,
            caret,
        });
    }

    /// 清除可编辑空段记录（输入未产生新版本时调用）。
    fn clear_empty(&mut self) {
        self.empty_paragraph = None;
    }

    /// 检查当前光标是否仍停留在刚插入的可编辑空段上，是则返回 `(start, end, caret)`。
    pub(super) fn matching_empty_paragraph(
        &self,
        id: DocumentId,
        revision: u64,
        selection: Selection,
        has_cursors: bool,
    ) -> Option<(usize, usize, usize)> {
        let paragraph = self.empty_paragraph.as_ref()?;
        if id != paragraph.document
            || revision != paragraph.revision
            || selection != Selection::point(paragraph.caret)
            || has_cursors
        {
            return None;
        }
        Some((
            paragraph.inserted.start,
            paragraph.inserted.end,
            paragraph.caret,
        ))
    }

    /// 收回空段后清除记录、重置目标列并维持编辑态。
    pub(super) fn finish_empty_paragraph_backspace(&mut self, doc: Option<DocumentId>) {
        self.empty_paragraph = None;
        self.goal = None;
        self.editing = doc;
    }
}

impl App {
    /// 结束一次可编辑 Markdown 的编辑操作：清除竖直目标列并保持实时编辑态。
    pub(super) fn finish_live_edit(&mut self) {
        let doc = self.editor.active().map(|d| d.id);
        self.preview.markdown.live.finish_edit(doc, false);
    }

    /// 重排之前光标所在的那一行的高度（相对内容顶部，不含留白），重排后据此保持它在屏幕上不动。
    pub(crate) fn old_markdown_caret(
        &mut self,
        old: Option<&super::laid::Laid>,
        source: usize,
        live: Option<&super::laid::LiveKey>,
        caret: usize,
    ) -> Option<(usize, f32)> {
        let source_changed = old.is_some_and(|laid| laid.source != source);
        let doc = self.editor.active().map(|d| (d.id, d.revision));
        let allow = self.preview.markdown.live.allow_caret_anchor(
            caret,
            self.scroll.keyed,
            source_changed,
            doc,
        );
        let old = old?;
        if !allow {
            return None;
        }
        let (_, y, h) = old.caret_point(caret)?;
        if source_changed {
            // A background parse can replace an approximate layout for the same
            // edit. Anchor only its unchanged, visible caret; never chase one the
            // user left offscreen by scrolling or moved since the last frame.
            let scroll = self.preview.markdown.scroll();
            if y + h <= scroll || y >= scroll + self.preview.markdown.area.h {
                return None;
            }
        } else if old.live_revision() != live.map(|k| k.2) {
            return None;
        }
        Some((caret, old.content_y(y)))
    }

    /// 重排后光标所在的那一行在屏幕上不动：光标换了叶子块时，上面的块从原文换回排版（或反过来），
    /// 高度变了，内容整体上下错开；按光标的新旧高度之差滚动。
    pub(crate) fn keep_markdown_caret_still(
        &mut self,
        still: Option<(usize, f32)>,
        laid: &super::laid::Laid,
    ) {
        let Some((source, before)) = still else {
            return;
        };
        let Some((_, y, _)) = laid.caret_point(source) else {
            return;
        };
        let dy = laid.content_y(y) - before;
        if dy.abs() >= 0.5 {
            let scroll = self.preview.markdown.scroll();
            self.preview.markdown.scroll_to(scroll + dy);
        }
    }

    /// 记录刚插入的可编辑空段，供光标移出时自动收回。
    pub(super) fn remember_empty_paragraph(
        &mut self,
        previous_revision: u64,
        start: usize,
        trailing: usize,
    ) {
        if let Some(doc) = self
            .editor
            .active()
            .filter(|doc| doc.revision != previous_revision)
        {
            let caret = doc.selection.head;
            let end = caret + trailing * doc.line_ending.as_str().len();
            self.preview
                .markdown
                .live
                .remember_empty(doc.id, doc.revision, start..end, caret);
        } else {
            self.preview.markdown.live.clear_empty();
        }
    }

    /// 单击正文下方时在文末放置一个可编辑的空段。
    pub(super) fn open_paragraph_below_body(&mut self) {
        let Some(doc) = self.editor.active() else {
            return;
        };
        let Some(rope) = doc.text().rope() else {
            return;
        };
        let last = rope.len_lines() - 1;
        let blank = |line| rope.line(line).chars().all(char::is_whitespace);
        let ending = if blank(last) {
            if last == 0 || blank(last - 1) {
                return;
            }
            "\n"
        } else {
            "\n\n"
        };
        let (at, revision) = (rope.len_chars(), doc.revision);
        if doc.selection != winc_core::Selection::point(at) {
            return;
        }
        self.run(winc_editor::Command::Insert(ending.into()));
        self.remember_empty_paragraph(revision, at, 0);
    }

    /// 文档的最后一行属于代码块、表格、分隔线等不是正文的块时，在文末另起一段并把光标移到
    /// 该段，返回是否已处理。
    ///
    /// 文档以换行符结束时光标可以直接移到末尾的空行，不需要处理。
    pub(super) fn live_append_paragraph(&mut self) -> bool {
        let Some(rope) = self.editor.active().and_then(|d| d.text().rope()) else {
            return false;
        };
        let chars = rope.len_chars();
        if chars == 0 || rope.char(chars - 1) == '\n' {
            return false;
        }
        if self.live_prose_at(rope.len_bytes()) != Some(false)
            || self.live_fenced().is_some_and(|fence| !fence.closed)
        {
            return false;
        }
        if let Some(doc) = self.editor.active_mut() {
            doc.collapse_selections();
            doc.selection = winc_core::Selection::point(chars);
        }
        self.run(winc_editor::Command::Insert("\n\n".to_owned()));
        let doc = self.editor.active().map(|d| d.id);
        self.preview.markdown.live.finish_edit(doc, true);
        self.reset_blink();
        self.request_redraw();
        true
    }

    /// ←→ 略过不显示的块标记和块间分隔行。普通文字和可编辑的空行交给编辑区处理。
    pub(super) fn live_horizontal(&mut self, right: bool) -> bool {
        self.preview.markdown.live.set_line_end(false, 0, 0);
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
        self.finish_live_edit();
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
        if pos - start >= hidden
            || (current
                && preview
                    .layout
                    .as_ref()
                    .is_some_and(|laid| laid.shows_source(rope.char_to_byte(pos))))
        {
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
        let (col_left, col_right) = laid.column_bounds();
        let x = if end {
            col_right + self.preview.markdown.area.w
        } else {
            col_left - self.preview.markdown.area.w
        };
        let sy = self.preview.markdown.area.y - self.preview.markdown.scroll() + y + h / 2.0;
        let Some(found) = self.preview.markdown.source_at(x, sy) else {
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
        let doc_id = doc.id;
        doc.selection = doc.selection.moved_to(pos, extend);
        self.preview
            .markdown
            .live
            .finish_line_end_move(Some(doc_id), wrapped, pos, revision);
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

    /// 可编辑的预览中光标应画在上一显示行末尾时，返回光标的原文位置（字节偏移）。
    pub(crate) fn live_upstream(&self) -> Option<usize> {
        let doc = self.editor.active()?;
        self.preview
            .markdown
            .live
            .is_upstream(doc.selection.head, doc.revision)
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
        let goal = self.preview.markdown.live.goal_x_for(head, x);
        let step = by.unwrap_or(h);
        let sign = if down { 1.0 } else { -1.0 };
        let height = laid.height();
        let screen_x = laid.column_bounds().0 + goal;
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
            let found = self
                .preview
                .markdown
                .source_at(screen_x, screen_y)
                .and_then(|source| {
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
        let (doc_id, revision) = (doc.id, doc.revision);
        let upstream = moved.is_some_and(|(_, upstream)| upstream);
        self.preview.markdown.live.finish_vertical_move(
            Some(doc_id),
            upstream,
            pos,
            revision,
            goal,
        );
        self.preview.markdown.selection = None;
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
        self.preview
            .markdown
            .live
            .sync_editing(doc.id, source, doc.revision)
    }

    /// 可编辑的预览中是否显示文字光标：在当前文档的预览上点击、移动光标或输入之后显示
    /// （见 [`App::live_editing`]）。
    pub(crate) fn live_caret_shown(&self) -> bool {
        let doc = self.editor.active().map(|d| d.id);
        self.preview.markdown.live.caret_shown_for(doc)
    }

    /// 在可编辑的预览中开始编辑（点击、移动光标、组字）：光标所在的块显示原文。
    pub(in crate::app) fn start_live_editing(&mut self) {
        let doc = self.editor.active().map(|d| d.id);
        self.preview.markdown.live.begin_editing(doc);
    }

    /// 结束一次会重置折行归属的编辑操作：清除导航记忆并保持实时编辑态。
    pub(super) fn finish_live_edit_reset_line(&mut self) {
        let doc = self.editor.active().map(|d| d.id);
        self.preview.markdown.live.finish_edit(doc, true);
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
        let current = source
            .zip(self.editor.active())
            .map(|(source, doc)| (doc.id, source, doc.revision));
        if !self.preview.markdown.live.advance_followed(current) {
            return;
        }
        let (Some(source), Some(doc)) = (source, self.editor.active()) else {
            return;
        };
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
