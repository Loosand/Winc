//! 弹出层与子菜单的锚定定位及视口边界避让计算。
//!
//! 所有函数均为纯几何计算，不持有窗口或组件状态，可供补全列表、右键菜单、子菜单、属性浮层与
//! 工具提示共用。
//!
//! # 示例
//!
//! ```
//! use winc_cpui::popover::{PopoverOptions, place_below_or_above};
//! use winc_render::Rect;
//!
//! let viewport = Rect::new(0.0, 0.0, 400.0, 300.0);
//! let opts = PopoverOptions {
//!     margin: 8.0,
//!     gap: 4.0,
//!     ..PopoverOptions::default()
//! };
//!
//! // 光标靠近底边时自动翻转至上方。
//! let caret = Rect::new(120.0, 240.0, 2.0, 18.0);
//! let card = place_below_or_above(caret, viewport, (180.0, 100.0), opts);
//! assert_eq!(card, Rect::new(120.0, 136.0, 180.0, 100.0));
//! ```

use winc_render::Rect;

/// 锚定在参考矩形上方或下方的定位选项。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PopoverOptions {
    /// 浮层与视口四边保持的最小留白（逻辑像素）。
    pub margin: f32,
    /// 浮层与锚点矩形之间的垂直间距（逻辑像素）。
    pub gap: f32,
    /// 浮层左边缘相对锚点左边缘的水平偏移（逻辑像素）。
    pub offset_x: f32,
    /// 当锚点下方空间不足时，是否仅在上方空间大于下方空间时才向上翻转。
    ///
    /// 为 `false` 时只要下方放不下就向上翻转；为 `true` 时在上下两侧中选择更宽敞的一侧。
    pub prefer_larger_side: bool,
    /// 是否按所选一侧的可用高度裁剪浮层高度，并保证不低于该下限（逻辑像素）。
    ///
    /// 为 `None` 时保留完整高度（仅不超过视口可用高度），必要时允许覆盖锚点区域。
    pub min_height: Option<f32>,
}

/// 锚定在屏幕坐标点处的弹出层定位参数。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PointPlacement {
    /// 触发弹出层的基准点 `(x, y)`（逻辑像素）。
    pub point: (f32, f32),
    /// 浮层与视口四边保持的最小留白（逻辑像素）。
    pub margin: f32,
    /// 为 `true` 时将浮层右边缘对齐到 `point.0`，否则左边缘对齐到 `point.0`。
    pub right_aligned: bool,
    /// 浮层顶边相对 `point.1` 向上回退的距离（逻辑像素，例如让勾选行居中对准基准点）。
    pub anchor_y_offset: f32,
}

/// 相对父菜单展开的级联子菜单定位参数。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SubmenuPlacement {
    /// 父级菜单卡片的矩形区域。
    pub parent_card: Rect,
    /// 展开子菜单的触发行矩形区域。
    pub trigger_row: Rect,
    /// 子菜单与视口四边保持的最小留白（逻辑像素）。
    pub margin: f32,
    /// 子菜单与父级菜单水平方向的重叠像素（避免鼠标移入时产生空隙）。
    pub overlap: f32,
    /// 子菜单顶部内边距，用于让子菜单首行与触发行垂直对齐。
    pub pad_y: f32,
}

/// 将尺寸为 `size` 的弹出层锚定在 `anchor` 的下方或上方，并限制在 `viewport` 的留白范围内。
///
/// 优先放置在 `anchor` 下方；下方空间不足时按 [`PopoverOptions::prefer_larger_side`] 决定是否
/// 翻转到上方，最后在视口有效范围内完成水平与垂直夹紧。
pub fn place_below_or_above(
    anchor: Rect,
    viewport: Rect,
    size: (f32, f32),
    options: PopoverOptions,
) -> Rect {
    let bounds = inset_bounds(viewport, options.margin);
    let w = size.0.min(bounds.w).max(0.0);
    let below_space = bounds.bottom() - (anchor.bottom() + options.gap);
    let above_space = (anchor.y - options.gap) - bounds.y;
    let up = if options.prefer_larger_side {
        below_space < size.1 && above_space > below_space
    } else {
        below_space < size.1
    };
    let h = match options.min_height {
        Some(min_h) => {
            let space = if up { above_space } else { below_space };
            size.1.min(space.max(min_h)).min(bounds.h).max(0.0)
        }
        None => size.1.min(bounds.h).max(0.0),
    };
    let raw_x = anchor.x + options.offset_x;
    let raw_y = if up {
        anchor.y - options.gap - h
    } else {
        anchor.bottom() + options.gap
    };
    clamp_into_bounds(Rect::new(raw_x, raw_y, w, h), bounds)
}

/// 将尺寸为 `size` 的弹出层锚定在 [`PointPlacement::point`] 附近，并限制在 `viewport` 内。
pub fn place_at_point(viewport: Rect, size: (f32, f32), placement: PointPlacement) -> Rect {
    let bounds = inset_bounds(viewport, placement.margin);
    let w = size.0.min(bounds.w).max(0.0);
    let h = size.1.min(bounds.h).max(0.0);
    let raw_x = if placement.right_aligned {
        placement.point.0 - w
    } else {
        placement.point.0
    };
    let raw_y = placement.point.1 - placement.anchor_y_offset;
    clamp_into_bounds(Rect::new(raw_x, raw_y, w, h), bounds)
}

/// 将尺寸为 `size` 的子菜单放置在父菜单右侧；右侧放不下时翻转至父菜单左侧，并限制在 `viewport` 内。
pub fn place_submenu(viewport: Rect, size: (f32, f32), placement: SubmenuPlacement) -> Rect {
    let bounds = inset_bounds(viewport, placement.margin);
    let w = size.0.min(bounds.w).max(0.0);
    let h = size.1.min(bounds.h).max(0.0);
    let right = placement.parent_card.right() - placement.overlap;
    let raw_x = if right + w <= bounds.right() {
        right
    } else {
        placement.parent_card.x - w + placement.overlap
    };
    let raw_y = placement.trigger_row.y - placement.pad_y;
    clamp_into_bounds(Rect::new(raw_x, raw_y, w, h), bounds)
}

/// 将矩形 `rect` 的宽高与位置限制在 `viewport` 四边各内缩 `margin` 后的区域内。
pub fn clamp_rect(rect: Rect, viewport: Rect, margin: f32) -> Rect {
    let bounds = inset_bounds(viewport, margin);
    let w = rect.w.min(bounds.w).max(0.0);
    let h = rect.h.min(bounds.h).max(0.0);
    clamp_into_bounds(Rect::new(rect.x, rect.y, w, h), bounds)
}

/// 计算视口四边各缩进 `margin` 后的可用边界矩形（宽、高不小于 0）。
fn inset_bounds(viewport: Rect, margin: f32) -> Rect {
    Rect::new(
        viewport.x + margin,
        viewport.y + margin,
        (viewport.w - 2.0 * margin).max(0.0),
        (viewport.h - 2.0 * margin).max(0.0),
    )
}

/// 在有效边界 `bounds` 内夹紧矩形的左上角；当边界空间不足时贴紧左上角。
fn clamp_into_bounds(rect: Rect, bounds: Rect) -> Rect {
    let x = rect
        .x
        .clamp(bounds.x, (bounds.right() - rect.w).max(bounds.x));
    let y = rect
        .y
        .clamp(bounds.y, (bounds.bottom() - rect.h).max(bounds.y));
    Rect::new(x, y, rect.w, rect.h)
}
