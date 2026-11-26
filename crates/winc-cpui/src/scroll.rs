//! 滚动区域与滚动条的无状态几何计算原语。
//!
//! CPUI 不托管滚动偏移状态：应用以普通数值持有当前偏移，通过 [`ScrollMetrics`] 计算边界、
//! 滚轮步进与条目可见性，并通过 [`ScrollbarSpec`] 换算滑块矩形与拖动偏移。

use winc_render::Rect;

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
