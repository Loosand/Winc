//! 滚动区域与滚动条的无状态几何计算原语。
//!
//! CPUI 不托管滚动偏移状态：应用以普通数值持有当前偏移，通过 [`ScrollMetrics`] 计算边界、
//! 滚轮步进与条目可见性，通过 [`ScrollbarSpec`] 换算滑块矩形与拖动偏移，并通过
//! [`VisibleWindow`] / [`virtual_list`] 实现 $O(\text{visible})$ 的长列表可视范围渲染。
//!
//! # 示例
//!
//! ```
//! use winc_cpui::scroll::{ScrollMetrics, VisibleWindow};
//!
//! let metrics = ScrollMetrics::new(120.0, 24_000.0, 240.0);
//! assert_eq!(metrics.scroll_by(60.0), 300.0);
//!
//! // 1,000 行等高列表仅计算视口覆盖的条目区间。
//! let window = VisibleWindow::fixed(1_000, 24.0, 120.0, 250.0);
//! let (range, offset, total_height) = window.bounds();
//! assert_eq!(range, 10..16);
//! assert!((offset - 10.0).abs() < 0.01);
//! assert_eq!(total_height, 24_000.0);
//! ```

use std::ops::Range;

use winc_render::Rect;

use crate::node::{Node, vstack};

/// 一维滚动区域的无状态度量快照（逻辑像素）。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ScrollMetrics {
    /// 可视区域沿滚动方向的长度。
    pub viewport: f32,
    /// 内容沿滚动方向的总长度。
    pub content: f32,
    /// 当前滚动偏移量。
    pub offset: f32,
}

impl ScrollMetrics {
    /// 构造滚动区域度量。
    pub fn new(viewport: f32, content: f32, offset: f32) -> Self {
        Self {
            viewport,
            content,
            offset,
        }
    }

    /// 返回允许的最大滚动偏移量；内容不超过可视区域时为 `0.0`。
    pub fn max_offset(self) -> f32 {
        (self.content - self.viewport).max(0.0)
    }

    /// 内容是否超出可视区域、需要滚动。
    pub fn can_scroll(self) -> bool {
        self.viewport > 0.0 && self.content > self.viewport
    }

    /// 将任意偏移量夹紧到 `[0.0, max_offset()]`。
    pub fn clamp(self, offset: f32) -> f32 {
        offset.clamp(0.0, self.max_offset())
    }

    /// 在当前偏移基础上叠加增量 `delta`（正数向下/右滚动），返回夹紧后的新偏移。
    pub fn scroll_by(self, delta: f32) -> f32 {
        self.clamp(self.offset + delta)
    }

    /// 计算将区间 `[item_start, item_start + item_len]` 完整滚入可视区所需的最小滚动偏移。
    ///
    /// 若条目已在可视区域内则保持当前偏移（并做边界夹紧）；条目高于可视区时优先对齐条目起点。
    pub fn ensure_visible(self, item_start: f32, item_len: f32) -> f32 {
        let current = self.clamp(self.offset);
        let next = if item_start < current {
            item_start
        } else if item_start + item_len > current + self.viewport {
            item_start + item_len - self.viewport
        } else {
            current
        };
        self.clamp(next)
    }
}

/// 滚动条滑块的几何规格（逻辑像素）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollbarSpec {
    /// 滑块厚度（竖直滚动条的宽度、水平滚动条的高度）。
    pub thickness: f32,
    /// 滑块与轨道外侧边缘之间的留白。
    pub inset: f32,
    /// 滑块活动范围相对轨道两端的缩进距离。
    pub end_margin: f32,
    /// 滑块的最小长度（内容很长时仍保持便于点中）。
    pub min_thumb: f32,
}

impl ScrollbarSpec {
    /// 返回滑块沿轨道方向的活动范围 `(起点, 可活动长度)`。
    fn room(self, start: f32, len: f32) -> (f32, f32) {
        (
            start + self.end_margin,
            (len - 2.0 * self.end_margin).max(0.0),
        )
    }

    /// 返回滑块沿轨道方向的 `(起点, 长度)`；内容不超过可视长度时返回 `None`。
    pub fn thumb_span(self, start: f32, len: f32, scroll: f32, content: f32) -> Option<(f32, f32)> {
        if content <= len || len <= 0.0 {
            return None;
        }
        let (start, room) = self.room(start, len);
        let size = (room * len / content).clamp(self.min_thumb.min(room), room);
        let offset = (scroll / (content - len)).clamp(0.0, 1.0) * (room - size);
        Some((start + offset, size))
    }

    /// 返回竖直滚动条滑块矩形（紧贴 `track` 右侧并留出 [`ScrollbarSpec::inset`]）。
    pub fn vertical_thumb(self, track: Rect, scroll: f32, content: f32) -> Option<Rect> {
        let (y, h) = self.thumb_span(track.y, track.h, scroll, content)?;
        let x = track.right() - self.inset - self.thickness;
        Some(Rect::new(x, y, self.thickness, h))
    }

    /// 返回水平滚动条滑块矩形（紧贴 `track` 下侧并留出 [`ScrollbarSpec::inset`]）。
    pub fn horizontal_thumb(self, track: Rect, scroll: f32, content: f32) -> Option<Rect> {
        let (x, w) = self.thumb_span(track.x, track.w, scroll, content)?;
        let y = track.bottom() - self.inset - self.thickness;
        Some(Rect::new(x, y, w, self.thickness))
    }

    /// 根据指针拖动位置反算新的滚动偏移量；按下点在滑块内的偏移 `grab` 保持不变。
    pub fn offset_for_thumb(
        self,
        start: f32,
        len: f32,
        thumb: f32,
        pointer: f32,
        grab: f32,
        max: f32,
    ) -> f32 {
        let (start, room) = self.room(start, len);
        ((pointer - grab - start) / (room - thumb).max(1.0)).clamp(0.0, 1.0) * max
    }
}

/// 虚拟滚动列表的可视区间计算结果。
///
/// 只描述落入当前视口的条目范围与首项的纵向裁剪偏移，配合 [`VisibleWindow::build`] 或
/// [`virtual_list`] 即可把长列表的布局与绘制复杂度限制在 $O(\text{visible})$。
#[derive(Debug, Clone, PartialEq)]
pub struct VisibleWindow {
    range: Range<usize>,
    scroll_offset: f32,
    total_height: f32,
}

impl VisibleWindow {
    /// 为等高长列表在 $O(1)$ 时间内计算可视窗口。
    pub fn fixed(total: usize, row_height: f32, viewport_height: f32, scroll: f32) -> Self {
        if total == 0 || row_height <= 0.0 || viewport_height <= 0.0 {
            return Self {
                range: 0..0,
                scroll_offset: 0.0,
                total_height: total as f32 * row_height.max(0.0),
            };
        }
        let total_height = total as f32 * row_height;
        let scroll = scroll.max(0.0);
        let start = ((scroll / row_height).floor() as usize).min(total);
        let end = (((scroll + viewport_height) / row_height).ceil() as usize)
            .min(total)
            .max(start);
        let scroll_offset = if start < end {
            scroll - start as f32 * row_height
        } else {
            0.0
        };
        Self {
            range: start..end,
            scroll_offset,
            total_height,
        }
    }

    /// 根据递增的条目顶部偏移切片 `tops` 与总高 `total_height`，在 $O(\log N)$ 内计算变高列表的可视窗口。
    ///
    /// 第 `i` 项占据区间 `[tops[i], next_top)`，其中最后一项的底边为 `total_height`。
    pub fn from_offsets(
        tops: &[f32],
        total_height: f32,
        viewport_height: f32,
        scroll: f32,
    ) -> Self {
        let total = tops.len();
        let total_height = total_height.max(0.0);
        if total == 0 || viewport_height <= 0.0 {
            return Self {
                range: 0..0,
                scroll_offset: 0.0,
                total_height,
            };
        }
        let top = scroll.max(0.0);
        let bottom = top + viewport_height;
        // 找到第一个底边 > top 的条目（若第 i 项底边为 next_top，等价于 next_top <= top 时跳过）。
        let (mut lo, mut hi) = (0usize, total);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let item_bottom = tops.get(mid + 1).copied().unwrap_or(total_height);
            if item_bottom <= top {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let start = lo;
        let end = tops
            .partition_point(|&item_top| item_top < bottom)
            .max(start);
        let scroll_offset = if start < end { top - tops[start] } else { 0.0 };
        Self {
            range: start..end,
            scroll_offset,
            total_height,
        }
    }

    /// 根据已按纵向升序排好位置的条目矩形切片 `rects` 与裁剪视口 `clip`，在 $O(\log N)$ 内计算可视窗口。
    pub fn from_rects(rects: &[Rect], clip: Rect) -> Self {
        if rects.is_empty() || clip.h <= 0.0 {
            return Self {
                range: 0..0,
                scroll_offset: 0.0,
                total_height: 0.0,
            };
        }
        let start = rects.partition_point(|r| r.bottom() < clip.y);
        let end = rects.partition_point(|r| r.y <= clip.bottom()).max(start);
        let scroll_offset = if start < end {
            clip.y - rects[start].y
        } else {
            0.0
        };
        let total_height = (rects[rects.len() - 1].bottom() - rects[0].y).max(0.0);
        Self {
            range: start..end,
            scroll_offset,
            total_height,
        }
    }

    /// 返回可视窗口的 `(可见索引区间, 首项纵向偏移, 全部条目总高度)`。
    pub fn bounds(&self) -> (Range<usize>, f32, f32) {
        (self.range.clone(), self.scroll_offset, self.total_height)
    }

    /// 仅对落入可视区间的条目调用 `build_row` 生成节点，并包入带裁剪与首项偏移的纵向容器。
    pub fn build<'a, A>(&self, mut build_row: impl FnMut(usize) -> Node<'a, A>) -> Node<'a, A> {
        vstack()
            .w_full()
            .scroll_y(self.scroll_offset)
            .children(self.range.clone().map(&mut build_row))
    }
}

/// 构造虚拟滚动列表容器：仅对 `window` 可视区间内的条目调用 `build_row` 生成节点。
///
/// 返回的容器宽度占满父容器并已启用裁剪与首行偏移，调用方按需链式追加 `.h(...)` 或 `.flex_1()`。
pub fn virtual_list<'a, A>(
    window: VisibleWindow,
    build_row: impl FnMut(usize) -> Node<'a, A>,
) -> Node<'a, A> {
    window.build(build_row)
}
