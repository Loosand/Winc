//! 预览的绘制：Markdown 预览的排版和绘制、表格预览、PDF 预览。
//!
//! `markdown` 排版并绘制 Markdown、图片和公式，`table` 绘制 xlsx、csv 工作簿的表格，`pdf` 逐页
//! 绘制渲染好的 PDF 页面；表格的
//! 读取（xlsx 包、数字格式、单元格文字）在 `winc_preview::table`，这一层只管排版、绘制和
//! 命中测试。
//!
//! 图片预览画在编辑区里，见 `editor/picture.rs`。

pub(in crate::view) mod json;
pub(crate) mod markdown;
pub(in crate::view) mod pdf;
pub(crate) mod table;

use crate::theme::{Palette, RADIUS_POPUP};
use winc_render::{Frame, Rect};

/// 绘制浮在预览内容上的工具栏或卡片 `rect`（含细边框）：阴影、细边框和底色，裁剪到 `clip`；
/// 细边框画在卡片底色上，不透出下方的内容。
pub(in crate::view::preview) fn draw_floating_bar(
    frame: &mut Frame<'_>,
    p: &Palette,
    rect: Rect,
    clip: Rect,
) {
    let covered = frame.covered(rect, RADIUS_POPUP);
    frame.shadow(rect, RADIUS_POPUP, 12.0, 4.0, p.shadow, Some(covered));
    frame.rect_clipped(rect, p.popup_bg, RADIUS_POPUP, clip);
    frame.rect_clipped(rect, p.popup_border, RADIUS_POPUP, clip);
    let inner = rect.inset(1.0, 1.0);
    frame.rect_clipped(inner, p.popup_bg, RADIUS_POPUP - 1.0, clip);
}
