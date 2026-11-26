//! Minc 基于 CPU 渲染的声明式桌面界面层（CPUI）。
//!
//! 自下而上分为四个单向依赖层次，不依赖窗口系统与应用层，仅共享 [`winc_render`]：
//! - **描述层**（[`Node`]）：构建界面描述树的原语与链式布局/样式构建器；
//! - **布局层**（[`layout`]、[`wrap_text`]）：基于 `taffy` 完成 Flexbox 测量、折行、绘制与裁剪；
//! - **交互层**（[`HitMap`]、[`SurfaceCache`]、[`popover`]、[`scroll`]）：收集交互命中表、帧间像素快照复用、弹出层锚定定位与滚动几何；
//! - **组件层**（[`widgets`]）：由语义化 [`Theme`] 驱动的通用界面组件（按钮、勾选框、徽章、对话框栏、输入框槽位）。

mod cache;
mod hit;
mod layout;
mod node;
pub mod popover;
pub mod scroll;
#[cfg(test)]
mod tests;
pub mod widgets;
mod wrap;

pub use cache::SurfaceCache;
pub use hit::HitMap;
pub use layout::{LaidOut, TextBasis, layout, render};
pub use node::{
    CardFrame, MeasureFn, Node, PaintFn, TextAlign, box_, custom, custom_measured, divider, hstack,
    hstack_rev, spacer, text, vdivider, vstack,
};
pub use popover::{
    PointPlacement, PopoverOptions, SubmenuPlacement, clamp_rect, place_at_point,
    place_below_or_above, place_submenu,
};
pub use scroll::{ScrollMetrics, ScrollbarSpec};
pub use widgets::Theme;
pub use wrap::wrap_text;
