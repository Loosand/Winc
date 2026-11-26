//! 声明式通用基础控件：按钮、勾选框、彩色标签徽章、方向箭头、对话框头尾栏与输入框占位槽。
//!
//! 全部控件仅依赖注入的语义化 [`Theme`] 与底层 CPU 渲染器，不内置具体主题色，也不依赖宿主图标表。
//!
//! # 示例
//!
//! ```
//! use winc_cpui::widgets::{ButtonKind, Theme, action_button};
//! use winc_cpui::{TextBasis, render};
//! use winc_render::{Color, FontKind, Rect, Renderer};
//!
//! let theme = Theme {
//!     text: Color::hex(0x1e1e1e),
//!     text_muted: Color::hex(0x666666),
//!     text_dim: Color::hex(0x999999),
//!     accent: Color::hex(0x2563eb),
//!     accent_fill: Color::hex(0x2563eb),
//!     text_on_accent: Color::hex(0xffffff),
//!     error: Color::hex(0xdc2626),
//!     hover: Color::hex(0xe5e7eb),
//!     selected_inactive: Color::hex(0xd1d5db),
//!     control_height: 28.0,
//!     icon_button_size: 28.0,
//!     dialog_button_width: 84.0,
//!     row_radius: 6.0,
//!     panel_pad: 20.0,
//! };
//!
//! let mut renderer = Renderer::offscreen(200.0, 60.0, 1.0, Color::hex(0xffffff), None, None)
//!     .expect("offscreen renderer initializes");
//! let m = renderer.metrics(FontKind::Ui, 13.0);
//! let basis = TextBasis { ui_size: 13.0, x_height: m.x_height, cap_height: m.cap_height };
//! let mut hits = winc_cpui::HitMap::new();
//! renderer.draw(|frame| {
//!     let btn = action_button(&theme, 13.0, "保存", ButtonKind::Primary, false, "save");
//!     hits = render(frame, basis, Rect::new(0.0, 0.0, 200.0, 60.0), btn);
//! });
//! assert!(hits.rect_of(&"save").is_some());
//! ```

use std::borrow::Cow;

use winc_render::{Color, Frame, Rect, Stroke, rounded_outline};

use crate::node::{Node, TextAlign, custom, custom_measured, hstack, spacer, text, vstack};

/// 图标描边宽度（逻辑像素）。
const ICON_STROKE: f32 = 1.25;
/// 勾选记号描边宽度（逻辑像素）。
const CHECK_MARK_STROKE: f32 = 1.5;

/// CPUI 组件层所需的语义化视觉设计变量（由宿主应用按当前主题注入）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    /// 正文文字色。
    pub text: Color,
    /// 次要弱化文字色（用于图标与辅助标签）。
    pub text_muted: Color,
    /// 禁用文字与未勾选边框色。
    pub text_dim: Color,
    /// 强调色（用于勾选框实底等）。
    pub accent: Color,
    /// 主操作按钮实底强调色。
    pub accent_fill: Color,
    /// 强调色实底上的高对比度文字与勾选记号色。
    pub text_on_accent: Color,
    /// 错误与危险操作文字色。
    pub error: Color,
    /// 普通按钮底色与悬停背景色。
    pub hover: Color,
    /// 普通按钮悬停态背景色。
    pub selected_inactive: Color,
    /// 标准操作控件与按钮高度（逻辑像素）。
    pub control_height: f32,
    /// 标准正方形图标按钮边长（逻辑像素）。
    pub icon_button_size: f32,
    /// 对话框底部标准操作按钮最小/固定宽度（逻辑像素）。
    pub dialog_button_width: f32,
    /// 行与按钮圆角半径（逻辑像素）。
    pub row_radius: f32,
    /// 对话框与面板标准水平内边距（逻辑像素）。
    pub panel_pad: f32,
}

/// 对话框与提示框底部操作按钮的角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// 主按钮：强调色实底、高对比度文字。
    Primary,
    /// 普通按钮：浅灰底色。
    Normal,
    /// 危险操作按钮：浅灰底色配错误色文字。
    Destructive,
}

/// 底部操作按钮的状态配置（供 [`dialog_footer`] 组合使用）。
#[derive(Debug, Clone)]
pub struct ButtonSpec<'a, A> {
    /// 按钮文字。
    pub label: Cow<'a, str>,
    /// 按钮角色。
    pub kind: ButtonKind,
    /// 是否可点击；禁用时灰底淡字且不响应悬停。
    pub enabled: bool,
    /// 鼠标是否悬停在按钮上。
    pub hovered: bool,
    /// 点击或命中时对应的动作标识。
    pub action: A,
}

/// 绘制居中关闭叉号图标（×），`size` 为叉的边长（逻辑像素）。
fn draw_close_mark(
    frame: &mut Frame<'_>,
    (cx, cy): (f32, f32),
    size: f32,
    color: Color,
    clip: Rect,
) {
    let h = size / 2.0;
    let lines = [
        Stroke::Line(cx - h, cy - h, cx + h, cy + h),
        Stroke::Line(cx - h, cy + h, cx + h, cy - h),
    ];
    frame.stroke(&lines, ICON_STROKE, color, clip);
}

/// 绘制 12×12 圆角勾选框（勾选态铺强调色并绘制对勾，未勾选态绘制 1px 细框）。
fn draw_checkbox_mark(
    frame: &mut Frame<'_>,
    (cx, cy): (f32, f32),
    checked: bool,
    accent: Color,
    border: Color,
    on_accent: Color,
    clip: Rect,
) {
    let size = 12.0;
    let rect = Rect::new(
        (cx - size / 2.0).round(),
        (cy - size / 2.0).round(),
        size,
        size,
    );
    if checked {
        frame.rect_clipped(rect, accent, 3.0, clip);
        let (x, y) = (rect.x, rect.y);
        let mark = [
            Stroke::Line(x + 3.0, y + 6.2, x + 5.2, y + 8.4),
            Stroke::Line(x + 5.2, y + 8.4, x + 9.2, y + 3.8),
        ];
        frame.stroke(&mark, CHECK_MARK_STROKE, on_accent, clip);
    } else {
        let outline = rounded_outline(rect.x + 0.5, rect.y + 0.5, rect.w - 1.0, rect.h - 1.0, 3.0);
        frame.stroke(&outline, 1.0, border, clip);
    }
}

/// 构造标准圆角操作按钮（高度与最小宽对齐弹窗规范，文字水平垂直居中）。
pub fn action_button<'a, A>(
    theme: &Theme,
    size: f32,
    title: impl Into<Cow<'a, str>>,
    kind: ButtonKind,
    hovered: bool,
    action: A,
) -> Node<'a, A> {
    action_button_with(theme, size, title, kind, true, hovered, None, action)
}

/// 构造支持启用/禁用状态和可选固定宽度的圆角操作按钮。
#[allow(clippy::too_many_arguments)]
pub fn action_button_with<'a, A>(
    theme: &Theme,
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
        ButtonKind::Destructive => theme.error,
        _ => theme.text,
    };
    let (fill, color) = match (kind == ButtonKind::Primary, enabled, hovered) {
        (true, false, _) => (theme.hover, theme.text_dim),
        (true, true, false) => (theme.accent_fill, theme.text_on_accent),
        (true, true, true) => (theme.accent_fill.alpha(0.85), theme.text_on_accent),
        (false, false, _) => (theme.hover, theme.text_dim),
        (false, _, false) => (theme.hover, ink),
        (false, _, true) => (theme.selected_inactive, ink),
    };

    let node = hstack()
        .h(theme.control_height)
        .items_center()
        .justify_center()
        .flex_shrink(0.0)
        .bg(fill)
        .rounded(theme.row_radius)
        .hit(action)
        .child(
            text(title)
                .text_size(size)
                .line_height(theme.control_height)
                .color(color)
                .text_align(TextAlign::Center)
                .flex_shrink(0.0),
        );

    match fixed_w {
        Some(w) => node.w(w),
        None => node.min_w(theme.dialog_button_width).px(14.0),
    }
}

/// 构造对话框标准标题栏：左侧标题文字，右侧悬停高亮的关闭按钮。
pub fn dialog_header<'a, A>(
    theme: &Theme,
    size: f32,
    title: impl Into<Cow<'a, str>>,
    close_hovered: bool,
    close_action: A,
) -> Node<'a, A> {
    let close_icon_color = theme.text_muted;
    let btn_size = theme.icon_button_size;
    let close_btn = custom((btn_size, btn_size), move |frame, rect, clip| {
        let center = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
        draw_close_mark(frame, center, 7.0, close_icon_color, clip);
    })
    .rounded(theme.row_radius)
    .when(close_hovered, |n| n.bg(theme.hover))
    .hit(close_action);

    hstack()
        .w_full()
        .h_full()
        .pl(theme.panel_pad)
        .pr(12.0)
        .items_center()
        .justify_between()
        .gap(8.0)
        .child(text(title).min_w(0.0).text_size(size).color(theme.text))
        .child(close_btn)
}

/// 构造对话框标准底部操作栏：左侧可选自动折行的错误提示，右侧定宽「取消 + 确认」按钮组。
pub fn dialog_footer<'a, A>(
    theme: &Theme,
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
                    .color(theme.error)
                    .wrap(),
            ),
        None => spacer(),
    };

    let buttons = hstack()
        .gap(8.0)
        .flex_shrink(0.0)
        .items_center()
        .child(action_button_with(
            theme,
            size,
            cancel.label,
            cancel.kind,
            cancel.enabled,
            cancel.hovered,
            Some(theme.dialog_button_width),
            cancel.action,
        ))
        .child(action_button_with(
            theme,
            size,
            confirm.label,
            confirm.kind,
            confirm.enabled,
            confirm.hovered,
            Some(theme.dialog_button_width),
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
pub fn checkbox<'a, A>(
    theme: &Theme,
    size: f32,
    label: impl Into<Cow<'a, str>>,
    checked: bool,
    action: A,
) -> Node<'a, A> {
    let (accent, text_dim, text_on_accent, text_color) = (
        theme.accent,
        theme.text_dim,
        theme.text_on_accent,
        theme.text,
    );
    let icon = custom((20.0, theme.control_height), move |frame, rect, clip| {
        let at = (rect.x + 6.0, rect.y + rect.h / 2.0);
        draw_checkbox_mark(frame, at, checked, accent, text_dim, text_on_accent, clip);
    });

    hstack()
        .h(theme.control_height)
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
                .line_height(theme.control_height)
                .color(text_color),
        )
}

/// 构造圆角彩色标签徽章（Chip），常用于属性选项、状态标记和工具标签。
///
/// 内部文本超出可用宽度时自动裁剪，不会撑破所在行或遮挡右侧操作图标。
pub fn chip<'a, A>(
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
pub fn chevron<'a, A>(left: bool, color: Color) -> Node<'a, A> {
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

/// 构造单行输入框占位槽：由 Flexbox 计算槽位矩形，再交给调用方调用既有输入框绘制逻辑。
pub fn field_slot<'a, A>(
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
