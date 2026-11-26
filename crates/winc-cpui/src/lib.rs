//! Minc 基于 CPU 渲染的声明式桌面界面层（CPUI）。
//!
//! 自下而上分为四个单向依赖层次，不依赖窗口系统与应用层，仅共享 [`winc_render`]：
//! - **描述层**（[`Node`]）：构建界面描述树的原语与链式布局/样式构建器；
//! - **布局层**（[`layout`]、[`render`]、[`render_dirty`]、[`wrap_text`]）：基于 `taffy` 完成
//!   Flexbox 测量、折行、绘制、视口裁剪与局部脏区重绘；
//! - **交互层**（[`HitMap`]、[`SurfaceCache`]、[`popover`]、[`scroll`]）：收集交互命中表、
//!   帧间像素快照复用、弹出层锚定定位与虚拟滚动；
//! - **组件层**（[`widgets`]）：由语义化 [`Theme`] 驱动的通用界面组件（按钮、勾选框、徽章、
//!   对话框栏、输入框槽位）。
//!
//! # 快速示例（离屏布局、绘制与命中测试）
//!
//! ```
//! use winc_cpui::{HitMap, TextBasis, box_, hstack, render, text, vstack};
//! use winc_render::{Color, FontKind, Rect, Renderer};
//!
//! let mut renderer = Renderer::offscreen(320.0, 120.0, 1.0, Color::hex(0x1e1e1e), None, None)
//!     .expect("offscreen renderer initializes");
//! let metrics = renderer.metrics(FontKind::Ui, 13.0);
//! let basis = TextBasis {
//!     ui_size: 13.0,
//!     x_height: metrics.x_height,
//!     cap_height: metrics.cap_height,
//! };
//!
//! let mut hits = HitMap::new();
//! renderer.draw(|frame| {
//!     let tree = vstack()
//!         .w_full()
//!         .h_full()
//!         .p(12.0)
//!         .gap(8.0)
//!         .child(text("确认操作").text_size(13.0).color(Color::hex(0xffffff)))
//!         .child(
//!             hstack()
//!                 .gap(8.0)
//!                 .child(box_().size(80.0, 28.0).bg(Color::hex(0x2563eb)).hit("ok"))
//!                 .child(box_().size(80.0, 28.0).bg(Color::hex(0x444444)).hit("cancel")),
//!         );
//!     hits = render(frame, basis, Rect::new(0.0, 0.0, 320.0, 120.0), tree);
//! });
//!
//! let ok = hits.rect_of(&"ok").expect("ok button laid out");
//! assert_eq!(hits.at(ok.x + 5.0, ok.y + 5.0), Some(&"ok"));
//! ```

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
pub use layout::{LaidOut, TextBasis, layout, render, render_dirty};
pub use node::{
    CardFrame, MeasureFn, Node, PaintFn, TextAlign, box_, custom, custom_measured, divider, hstack,
    hstack_rev, spacer, text, vdivider, vstack,
};
pub use popover::{
    PointPlacement, PopoverOptions, SubmenuPlacement, clamp_rect, place_at_point,
    place_below_or_above, place_submenu,
};
pub use scroll::{ScrollMetrics, ScrollbarSpec, VisibleWindow, virtual_list};
pub use widgets::Theme;
pub use wrap::wrap_text;
