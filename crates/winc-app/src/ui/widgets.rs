//! 声明式通用基础控件：按钮、勾选框、彩色标签徽章、方向箭头、对话框头尾栏与自定义输入框槽位。
//!
//! 这些控件统一使用 `crate::ui` 原语组合，供弹窗、对话框、属性面板以及后续新面板直接复用，
//! 避免在各视图模块里重复拼接样式与坐标。

use std::borrow::Cow;

use winc_render::{Color, Frame, Rect, Stroke};

use super::{Node, TextAlign, custom, custom_measured, hstack, spacer, text, vstack};
use crate::icons;
use crate::theme::{
    BUTTON, DIALOG_BUTTON_W, Palette, RADIUS_ROW, SETTINGS_CONTROL_H, SETTINGS_PAD,
};

/// 对话框与提示框底部操作按钮的角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ButtonKind {
    /// 主按钮：强调色实底、高对比度文字。
    Primary,
    /// 普通按钮：浅灰底色。
    Normal,
    /// 危险操作按钮：浅灰底色配错误色文字。
    Destructive,
}

/// 底部操作按钮的状态配置（供 [`dialog_footer`] 组合使用）。
pub(crate) struct ButtonSpec<'a, A> {
    /// 按钮文字。
    pub(crate) label: Cow<'a, str>,
    /// 按钮角色。
    pub(crate) kind: ButtonKind,
    /// 是否可点击；禁用时灰底淡字且不响应悬停。
    pub(crate) enabled: bool,
    /// 鼠标是否悬停在按钮上。
    pub(crate) hovered: bool,
    /// 点击或命中时对应的动作标识。
    pub(crate) action: A,
}

/// 构造标准圆角操作按钮（高度与最小宽对齐弹窗规范，文字水平垂直居中）。
pub(crate) fn action_button<'a, A>(
    p: &Palette,
    size: f32,
    title: impl Into<Cow<'a, str>>,
    kind: ButtonKind,
    hovered: bool,
    action: A,
) -> Node<'a, A> {
    action_button_with(p, size, title, kind, true, hovered, None, action)
}

/// 构造支持启用/禁用状态和可选固定宽度的圆角操作按钮（与 `view::draw_action_button` 样式一致）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn action_button_with<'a, A>(
    p: &Palette,
    size: f32,
    title: impl Into<Cow<'a, str>>,
    kind: ButtonKind,
    enabled: bool,
    hovered: bool,
    fixed_w: Option<f32>,
    action: A,
) -> Node<'a, A> {
    let hovered = enabled && hovered;
    let ink = match kind {
        ButtonKind::Destructive => p.error,
        _ => p.text,
    };
    let (fill, color) = match (kind == ButtonKind::Primary, enabled, hovered) {
        (true, false, _) => (p.hover, p.text_dim),
        (true, true, false) => (p.accent_fill, p.text_on_accent),
        (true, true, true) => (p.accent_fill.alpha(0.85), p.text_on_accent),
        (false, false, _) => (p.hover, p.text_dim),
        (false, _, false) => (p.hover, ink),
        (false, _, true) => (p.selected_inactive, ink),
    };

    let node = hstack()
        .h(SETTINGS_CONTROL_H)
        .items_center()
        .justify_center()
        .flex_shrink(0.0)
        .bg(fill)
        .rounded(RADIUS_ROW)
        .hit(action)
        .child(
            text(title)
                .text_size(size)
                .line_height(SETTINGS_CONTROL_H)
                .color(color)
                .text_align(TextAlign::Center)
                .flex_shrink(0.0),
        );

    match fixed_w {
        Some(w) => node.w(w),
        None => node.min_w(DIALOG_BUTTON_W).px(14.0),
    }
}

/// 构造对话框标准标题栏：左侧标题文字，右侧悬停高亮的关闭按钮（几何与 `DialogLayout` 对齐）。
pub(crate) fn dialog_header<'a, A>(
    p: &Palette,
    size: f32,
    title: impl Into<Cow<'a, str>>,
    close_hovered: bool,
    close_action: A,
) -> Node<'a, A> {
    let close_icon_color = p.text_muted;
    let close_btn = custom((BUTTON, BUTTON), move |frame, rect, clip| {
        let center = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
        icons::close(frame, center, 7.0, close_icon_color, clip);
    })
    .rounded(RADIUS_ROW)
    .when(close_hovered, |n| n.bg(p.hover))
    .hit(close_action);

    hstack()
        .w_full()
        .h_full()
        .pl(SETTINGS_PAD)
        .pr(12.0)
        .items_center()
        .justify_between()
        .gap(8.0)
        .child(text(title).min_w(0.0).text_size(size).color(p.text))
        .child(close_btn)
}

/// 构造对话框标准底部操作栏：左侧可选自动折行的错误提示，右侧定宽「取消 + 确认」按钮组。
pub(crate) fn dialog_footer<'a, A>(
    p: &Palette,
    (size, small): (f32, f32),
    error: Option<Cow<'a, str>>,
    cancel: ButtonSpec<'a, A>,
    confirm: ButtonSpec<'a, A>,
) -> Node<'a, A> {
    let left = match error {
        Some(message) => vstack()
            .flex_1()
            .min_w(0.0)
            .h_full()
            .justify_center()
            .overflow_hidden()
            .child(
                text(message)
                    .w_full()
                    .text_size(small)
                    .line_height(16.0)
                    .color(p.error)
                    .wrap(),
            ),
        None => spacer(),
    };

    let buttons = hstack()
        .gap(8.0)
        .flex_shrink(0.0)
        .items_center()
        .child(action_button_with(
            p,
            size,
            cancel.label,
            cancel.kind,
            cancel.enabled,
            cancel.hovered,
            Some(DIALOG_BUTTON_W),
            cancel.action,
        ))
        .child(action_button_with(
            p,
            size,
            confirm.label,
            confirm.kind,
            confirm.enabled,
            confirm.hovered,
            Some(DIALOG_BUTTON_W),
            confirm.action,
        ));

    hstack()
        .w_full()
        .h_full()
        .items_center()
        .justify_between()
        .gap(16.0)
        .child(left)
        .child(buttons)
}

/// 构造带左侧复选框图标与右侧标签的勾选项节点；空间受限时文字自动收缩并裁剪，不挤压兄弟节点。
pub(crate) fn checkbox<'a, A>(
    p: &Palette,
    size: f32,
    label: impl Into<Cow<'a, str>>,
    checked: bool,
    action: A,
) -> Node<'a, A> {
    let (accent, text_dim, text_on_accent, text_color) =
        (p.accent, p.text_dim, p.text_on_accent, p.text);
    let icon = custom((20.0, SETTINGS_CONTROL_H), move |frame, rect, clip| {
        let at = (rect.x + 6.0, rect.y + rect.h / 2.0);
        icons::checkbox(
            frame,
            at,
            checked.then_some(true),
            accent,
            text_dim,
            text_on_accent,
            clip,
        );
    });

    hstack()
        .h(SETTINGS_CONTROL_H)
        .min_w(0.0)
        .flex_shrink(1.0)
        .items_center()
        .overflow_hidden()
        .hit(action)
        .child(icon)
        .child(
            text(label)
                .min_w(0.0)
                .text_size(size)
                .line_height(SETTINGS_CONTROL_H)
                .color(text_color),
        )
}

/// 构造圆角彩色标签徽章（Chip），常用于属性选项、状态标记和工具标签。
///
/// 内部文本超出可用宽度时自动裁剪，不会撑破所在行或遮挡右侧操作图标。
pub(crate) fn chip<'a, A>(
    label: impl Into<Cow<'a, str>>,
    fg: Color,
    bg: Color,
    size: f32,
    height: f32,
) -> Node<'a, A> {
    hstack()
        .h(height)
        .min_w(0.0)
        .flex_shrink(1.0)
        .px(7.0)
        .items_center()
        .bg(bg)
        .rounded(4.0)
        .overflow_hidden()
        .child(
            text(label)
                .min_w(0.0)
                .text_size(size)
                .line_height(height)
                .color(fg),
        )
}

/// 构造居中绘制的左右折线箭头（`left` 为真时指向左，否则指向右）。
pub(crate) fn chevron<'a, A>(left: bool, color: Color) -> Node<'a, A> {
    let dx = if left { -2.0 } else { 2.0 };
    custom_measured(
        |known_w, avail_w, _| (known_w.or(avail_w).unwrap_or(16.0), 16.0),
        move |frame, rect, clip| {
            let x = (rect.x + rect.w * 0.5).round();
            let y = (rect.y + rect.h * 0.5).round();
            frame.stroke(
                &[
                    Stroke::Line(x - dx, y - 4.0, x + dx, y),
                    Stroke::Line(x + dx, y, x - dx, y + 4.0),
                ],
                1.35,
                color,
                clip,
            );
        },
    )
    .w_full()
    .h_full()
}

/// 构造单行输入框占位槽：由 Flexbox 计算槽位矩形，再交给调用方调用既有 `TextField` 绘制逻辑。
pub(crate) fn field_slot<'a, A>(
    height: f32,
    paint: impl FnMut(&mut Frame<'_>, Rect, Rect) + 'a,
) -> Node<'a, A> {
    custom_measured(
        move |known_w, avail_w, _| (known_w.or(avail_w).unwrap_or(0.0), height),
        paint,
    )
    .w_full()
    .h(height)
    .flex_shrink(0.0)
}
