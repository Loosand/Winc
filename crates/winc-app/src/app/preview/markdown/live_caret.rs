//! 可编辑 Markdown 的光标移动、折行归属和视口跟随适配器。
//!
//! 纯状态与导航算法位于 `winc_richtext::session`，本模块负责对接活动文档与滚动视图。

use winc_core::{Selection, movement};
use winc_richtext::{row_affinity, vertical_move_target, visible_position};

use crate::app::{App, ScrollTarget};

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
        let revision = doc.revision;
        let Some((at, ending)) =
            winc_richtext::commands::plan_open_paragraph_below_body(rope, doc.selection)
        else {
            return;
        };
        self.run(winc_editor::Command::Insert(ending));
        self.remember_empty_paragraph(revision, at, 0);
    }

    /// 将 `winc-richtext` 产出的 [`winc_richtext::EditPlan`] 应用到当前活动文档。
    pub(super) fn apply_edit_plan(
        &mut self,
        plan: winc_richtext::EditPlan,
        record_table: bool,
    ) -> bool {
        if plan.slide {
            self.preview.markdown.slide = true;
        }
        let previous_revision = self.editor.active().map(|d| d.revision);
        let changed = !plan.replace.is_empty() || !plan.insert.is_empty();
        if let Some(doc) = self.editor.active_mut() {
            doc.collapse_selections();
            doc.selection = winc_core::Selection::new(plan.replace.start, plan.replace.end);
        } else {
            return false;
        }
        if changed {
            let cmd = if plan.paste {
                winc_editor::Command::Paste(plan.insert)
            } else {
                winc_editor::Command::Insert(plan.insert)
            };
            self.run_source_edit(cmd);
        }
        if let Some(doc) = self.editor.active_mut() {
            let chars = doc.text().rope().map_or(0, ropey::Rope::len_chars);
            doc.selection = winc_core::Selection::new(
                plan.selection.anchor.min(chars),
                plan.selection.head.min(chars),
            );
            if record_table && Some(doc.revision) != previous_revision {
                self.preview
                    .markdown
                    .live
                    .record_table_edit(doc.id, doc.revision);
            }
        }
        true
    }

    /// 文档的最后一行属于代码块、表格、分隔线等不是正文的块时，在文末另起一段并把光标移到
    /// 该段，返回是否已处理。
    ///
    /// 文档以换行符结束时光标可以直接移到末尾的空行，不需要处理。
    pub(super) fn live_append_paragraph(&mut self) -> bool {
        let Some(doc) = self.editor.active() else {
            return false;
        };
        let Some(rope) = doc.text().rope() else {
            return false;
        };
        let end_prose = self.live_prose_at(rope.len_bytes());
        let open_fence = self.live_fenced().is_some_and(|fence| !fence.closed);
        let crlf = doc.line_ending == winc_core::LineEnding::CrLf;
        let Some(plan) =
            winc_richtext::commands::plan_append_paragraph(rope, end_prose, open_fence, crlf)
        else {
            return false;
        };
        self.apply_edit_plan(plan, false);
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
    pub(super) fn live_visible(&self, pos: usize, forward: bool) -> usize {
        let Some(doc) = self.editor.active() else {
            return pos;
        };
        let Some(rope) = doc.text().rope() else {
            return pos;
        };
        let body_start = self.property_body_start().unwrap_or(0);
        let preview = &self.preview.markdown;
        let fresh = preview.doc == Some(doc.id) && preview.checked == Some(doc.revision);
        visible_position(
            rope,
            preview.parsed.as_deref(),
            preview.layout.as_ref(),
            fresh,
            pos,
            forward,
            body_start,
        )
    }

    /// 编辑区的命令移动光标之后调用：光标落在不显示的块标记中时移到可见位置。
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
    pub(super) fn live_row_affinity(&self, source: usize, y: f32) -> bool {
        let Some(laid) = self.preview.markdown.layout.as_ref() else {
            return false;
        };
        let content_y = y - (self.preview.markdown.area.y - self.preview.markdown.scroll());
        row_affinity(laid, source, content_y)
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
        let moved = vertical_move_target(
            laid,
            self.preview.markdown.area.y,
            self.preview.markdown.scroll(),
            y,
            h,
            goal,
            down,
            by,
        );
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

    /// 可编辑的预览中是否显示文字光标。
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
