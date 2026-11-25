//! 声明式界面布局与绘制辅助：基于 `taffy` 计算 Flexbox 布局，直接绘制到 [`winc_render::Frame`]，
//! 并自动收集各交互区域的 [`HitMap`]。
//!
//! 本模块不接管应用状态，也不替换底层 CPU 渲染器；它以局部子树的方式嵌入任意逻辑像素矩形，
//! 适用于弹窗、设置页、属性浮层以及侧栏卡片流等需要自动排版的界面。

mod cache;
mod hit;
mod layout;
mod node;
#[cfg(test)]
mod tests;
pub(crate) mod widgets;

pub(crate) use cache::SurfaceCache;
pub(crate) use hit::HitMap;
#[allow(unused_imports)]
pub(crate) use layout::{LaidOut, TextBasis, layout, render};
#[allow(unused_imports)]
pub(crate) use node::{
    CardFrame, Node, TextAlign, box_, custom, custom_measured, divider, hstack, hstack_rev, spacer,
    text, vdivider, vstack,
};
