//! 编辑会话层：实时编辑状态、可见光标导航与折行归属。
//!
//! 本模块不依赖窗口系统或宿主应用，负责维护光标交互状态（[`LiveState`]）以及基于
//! 排版几何（[`Laid`]）的纯导航查询（跳过隐藏标记、折行归属、行首/行尾与上下行定位）。

use std::ops::Range;

use ropey::Rope;
use winc_core::Selection;
use winc_preview::markdown::Markdown;

use crate::layout::Laid;
use crate::mapping::{SourceBridge, line_end};

/// 刚插入的可编辑空段（光标离开或按退格时自动收回）。
struct EmptyParagraph<D> {
    document: D,
    revision: u64,
    /// 本次回车插入的原文范围（字符偏移），包括光标后的分隔换行。
    inserted: Range<usize>,
    caret: usize,
}

/// 可编辑 Markdown 预览的交互与导航状态。
///
/// 类型参数 `D` 为宿主文档标识（如 `winc_editor::DocumentId`），使内核与宿主文档表解耦。
pub struct LiveState<D> {
    /// 最近一次用于单光标排版的位置，拖选和键盘扩选时继续使用。
    layout_caret: Option<(D, u64, usize)>,
    /// 回车刚创建的空段，按退格时可整段收回。
    empty_paragraph: Option<EmptyParagraph<D>>,
    /// ↑↓ 移动的目标横坐标（相对内容左边）和移动后光标位置（字符偏移）。
    goal: Option<(f32, usize)>,
    /// 折行处光标位于上一行末尾的归属标记（字符偏移、文档版本）。
    line_end: Option<(usize, u64)>,
    /// 表格工具刚修改的文档版本：后台排版接手时保持页面滚动位置。
    table_edit_revision: Option<(D, u64)>,
    /// 上一帧跟踪的文档、光标原文位置与版本号。
    followed: Option<(D, usize, u64)>,
    /// 已进入编辑态的文档（光标所在块显示原文）。
    editing: Option<D>,
    /// 最近一次单击放下的光标（原文字节偏移）。
    clicked: Option<usize>,
}

impl<D> Default for LiveState<D> {
    fn default() -> Self {
        Self {
            layout_caret: None,
            empty_paragraph: None,
            goal: None,
            line_end: None,
            table_edit_revision: None,
            followed: None,
            editing: None,
            clicked: None,
        }
    }
}

impl<D: Copy + Eq> LiveState<D> {
    /// 切换文档时清除仅对当前文档有效的编辑辅助状态。
    pub fn reset_doc(&mut self) {
        self.empty_paragraph = None;
        self.table_edit_revision = None;
    }

    /// 离开预览模式时清除编辑态与光标跟随记录。
    pub fn leave_mode(&mut self) {
        self.followed = None;
        self.editing = None;
    }

    /// 清除竖直移动目标列与折行行尾归属。
    pub fn clear_navigation(&mut self) {
        self.goal = None;
        self.line_end = None;
    }

    /// 结束一次编辑命令：重置目标列并标记当前文档进入实时编辑态。
    pub fn finish_edit(&mut self, doc: Option<D>, clear_line_end: bool) {
        self.goal = None;
        if clear_line_end {
            self.line_end = None;
        }
        self.editing = doc;
    }

    /// 记录水平/拖动移动后的折行行尾归属。
    pub fn set_line_end(&mut self, upstream: bool, pos: usize, revision: u64) {
        self.line_end = upstream.then_some((pos, revision));
    }

    /// 完成行首/行尾跳转：写入折行归属、清空目标列并进入编辑态。
    pub fn finish_line_end_move(
        &mut self,
        doc: Option<D>,
        wrapped: bool,
        pos: usize,
        revision: u64,
    ) {
        self.line_end = wrapped.then_some((pos, revision));
        self.goal = None;
        self.editing = doc;
    }

    /// 完成竖直方向移动：更新折行归属、目标横坐标并进入编辑态。
    pub fn finish_vertical_move(
        &mut self,
        doc: Option<D>,
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
    pub fn record_click(
        &mut self,
        doc: D,
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
    pub fn clear_layout_caret(&mut self) {
        self.layout_caret = None;
    }

    /// 记录单光标排版锚点。
    pub fn set_layout_caret(&mut self, id: D, revision: u64, at: usize) {
        self.layout_caret = Some((id, revision, at));
    }

    /// 若单光标排版锚点仍属于当前文档与版本，返回其原文字节偏移。
    pub fn layout_caret_for(&self, id: D, revision: u64) -> Option<usize> {
        self.layout_caret
            .filter(|&(doc_id, rev, _)| doc_id == id && rev == revision)
            .map(|(_, _, at)| at)
    }

    /// 记录表格工具修改的文档版本。
    pub fn record_table_edit(&mut self, id: D, revision: u64) {
        self.table_edit_revision = Some((id, revision));
    }

    /// 重排前检查是否需要保持该光标在屏幕上的位置不动，同时消费掉单次点击标记。
    pub fn allow_caret_anchor(
        &mut self,
        caret: usize,
        keyed: bool,
        source_changed: bool,
        doc: Option<(D, u64)>,
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
    pub fn is_upstream(&self, head: usize, revision: u64) -> bool {
        self.line_end == Some((head, revision))
    }

    /// 计算本次竖直移动应沿用的目标横坐标。
    pub fn goal_x_for(&self, head: usize, fallback_x: f32) -> f32 {
        match self.goal {
            Some((goal, at)) if at == head => goal,
            _ => fallback_x,
        }
    }

    /// 当前活动文档是否应显示实时编辑光标。
    pub fn caret_shown_for(&self, doc: Option<D>) -> bool {
        doc.is_some() && self.editing == doc
    }

    /// 显式将文档置入实时编辑态；切换文档时同步清理过期的表格编辑版本标记。
    pub fn begin_editing(&mut self, doc: Option<D>) {
        if self.editing != doc {
            self.table_edit_revision = None;
        }
        self.editing = doc;
    }

    /// 排版前同步编辑态：若光标或文档版本相对上一帧发生变化，则自动进入实时编辑态。
    pub fn sync_editing(&mut self, id: D, source: usize, revision: u64) -> bool {
        let now = (id, source, revision);
        if self.followed.is_some_and(|f| f.0 == id && f != now) {
            self.editing = Some(id);
        }
        self.editing == Some(id)
    }

    /// 排版后更新光标跟随记录，返回本帧是否发生了需要滚动跟随的光标移动。
    pub fn advance_followed(&mut self, current: Option<(D, usize, u64)>) -> bool {
        let Some(now) = current else {
            self.followed = None;
            return false;
        };
        let moved = self.followed.is_some_and(|f| f.0 == now.0 && f != now);
        self.followed = Some(now);
        moved && self.editing == Some(now.0)
    }

    /// 编程改写源码（任务框勾选、图片缩放）后保持原有编辑态并同步光标跟踪。
    pub fn sync_after_programmatic_edit(
        &mut self,
        was_editing: bool,
        clear_editing: bool,
        current: Option<(D, usize, u64)>,
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
    pub fn remember_empty(
        &mut self,
        document: D,
        revision: u64,
        inserted: Range<usize>,
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
    pub fn clear_empty(&mut self) {
        self.empty_paragraph = None;
    }

    /// 检查当前光标是否仍停留在刚插入的可编辑空段上，是则返回 `(start, end, caret)`。
    pub fn matching_empty_paragraph(
        &self,
        id: D,
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
    pub fn finish_empty_paragraph_backspace(&mut self, doc: Option<D>) {
        self.empty_paragraph = None;
        self.goal = None;
        self.editing = doc;
    }
}

/// 光标落在不显示的块标记或块间分隔行时，返回移动方向上最近的可见位置（字符偏移）。
///
/// 向后（`forward`）时取该行内容的开头；向前时优先停在上方可编辑的空行，否则取上一行文字的末尾。
pub fn visible_position(
    rope: &Rope,
    parsed: Option<&Markdown>,
    laid: Option<&Laid>,
    fresh: bool,
    pos: usize,
    forward: bool,
    body_start: usize,
) -> usize {
    let mut pos = pos.max(body_start).min(rope.len_chars());
    if fresh
        && let Some(separator) =
            parsed.and_then(|md| md.collapsed_separator_at(rope.char_to_byte(pos)))
    {
        if !forward {
            let above = rope.byte_to_line(separator.start) - 1;
            return line_end(rope, above).max(body_start);
        }
        pos = rope.byte_to_char(separator.end);
    }
    let line = rope.char_to_line(pos);
    let mapped = |line| {
        fresh && laid.is_some_and(|laid| laid.blank_point(rope.line_to_byte(line)).is_some())
    };
    if mapped(line) {
        return pos;
    }
    let start = rope.line_to_char(line);
    let hidden = parsed.map_or(0, |md| {
        SourceBridge::new(rope, md, 0, fresh.then_some(0)).hidden_prefix_chars(line)
    });
    if pos - start >= hidden
        || (fresh && laid.is_some_and(|laid| laid.shows_source(rope.char_to_byte(pos))))
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

/// 原文位置 `source` 位于折行处时，判断内容纵坐标 `content_y` 是否更接近上一显示行。
///
/// 折行处的位置同时是上一行的末尾和下一行的开头：在上一行的右端点击、从上方按 ↓ 移到
/// 行尾时，光标应留在上一行。
pub fn row_affinity(laid: &Laid, source: usize, content_y: f32) -> bool {
    let far = |p: Option<(f32, f32, f32)>| p.map(|(_, top, h)| (top + h / 2.0 - content_y).abs());
    let below = laid.caret_point_with_affinity(source, false);
    let above = laid.caret_point_with_affinity(source, true);
    match (far(above), far(below)) {
        (Some(a), Some(b)) => a + 0.5 < b,
        _ => false,
    }
}

/// 计算竖直方向（↑/↓ 或翻页）移动后的目标原文位置（字节偏移）与折行行尾归属。
///
/// 找不到更远的行（已在第一行或最后一行）时返回 `None`，由调用方回退到文档首尾或追加新段。
#[allow(clippy::too_many_arguments)]
pub fn vertical_move_target(
    laid: &Laid,
    area_y: f32,
    scroll: f32,
    y: f32,
    h: f32,
    goal_x: f32,
    down: bool,
    by: Option<f32>,
) -> Option<(usize, bool)> {
    let step = by.unwrap_or(h);
    let sign = if down { 1.0 } else { -1.0 };
    let height = laid.height();
    let screen_x = laid.column_bounds().0 + goal_x;
    let screen_top = area_y - scroll;
    let center = y + h / 2.0;
    let mut target = center + sign * step;
    loop {
        let probe = if by.is_none() {
            laid.limit_blank_step(center, target)
        } else {
            target
        };
        if !(0.0..=height).contains(&probe) {
            return None;
        }
        let screen_y = screen_top + probe;
        let found = laid
            .source_at(area_y, scroll, screen_x, screen_y)
            .and_then(|source| {
                let upstream = row_affinity(laid, source, probe);
                let point = laid.caret_point_with_affinity(source, upstream);
                point
                    .is_none_or(|(_, row_y, _)| (row_y - y).abs() >= 0.5)
                    .then_some((source, upstream))
            });
        if found.is_some() || by.is_some() || !(0.0..=height).contains(&target) {
            return found;
        }
        target += sign * h / 2.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winc_preview::markdown;

    #[test]
    fn live_state_tracks_editing_empty_paragraph_and_caret_anchor() {
        let mut state: LiveState<u64> = LiveState::default();
        assert!(!state.caret_shown_for(Some(1)));

        // 首次同步不自动进入编辑态；光标或版本变化后进入编辑态。
        assert!(!state.sync_editing(1, 0, 1));
        assert!(!state.advance_followed(Some((1, 0, 1))));
        assert!(state.sync_editing(1, 5, 2));
        assert!(state.caret_shown_for(Some(1)));
        assert!(state.advance_followed(Some((1, 5, 2))));

        // 记录空段并在精确匹配时可收回。
        state.remember_empty(1, 2, 10..12, 12);
        assert_eq!(
            state.matching_empty_paragraph(1, 2, Selection::point(12), false),
            Some((10, 12, 12))
        );
        assert_eq!(
            state.matching_empty_paragraph(1, 2, Selection::point(11), false),
            None
        );
        state.finish_empty_paragraph_backspace(Some(1));
        assert_eq!(
            state.matching_empty_paragraph(1, 2, Selection::point(12), false),
            None
        );

        // 单击落点首次不锚定，随后键入移动时恢复锚定。
        state.record_click(1, 5, true, 5, 2);
        assert!(state.is_upstream(5, 2));
        assert!(!state.allow_caret_anchor(5, false, false, Some((1, 2))));
        assert!(state.allow_caret_anchor(5, false, false, Some((1, 2))));
    }

    #[test]
    fn visible_position_skips_hidden_list_marker_and_collapsed_separator() {
        let text = "第一段\n\n- 第二项\n";
        let rope = Rope::from_str(text);
        let parsed = markdown::parse(text);

        // 落在第二块 `- ` 标记内部向后移动时跳到正文开头「第」。
        let list_marker = rope.line_to_char(2) + 1;
        let second_text = rope.line_to_char(2) + 2;
        assert_eq!(
            visible_position(&rope, Some(&parsed), None, true, list_marker, true, 0),
            second_text
        );
        // 落在两块之间的折叠分隔空行上向后移动时跳到下一块正文开头。
        let sep_char = rope.line_to_char(1);
        assert_eq!(
            visible_position(&rope, Some(&parsed), None, true, sep_char, true, 0),
            second_text
        );
        // 落在两块之间的折叠分隔空行上向前移动时回到第一段行尾。
        assert_eq!(
            visible_position(&rope, Some(&parsed), None, true, sep_char, false, 0),
            line_end(&rope, 0)
        );
    }
}
