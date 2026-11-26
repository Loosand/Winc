//! 应用层对独立 [`winc_cpui`] 声明式界面层的薄适配与主题注入入口。

#[allow(unused_imports)]
pub(crate) use winc_cpui::{
    CardFrame, HitMap, LaidOut, Node, PointPlacement, PopoverOptions, ScrollMetrics, ScrollbarSpec,
    SubmenuPlacement, SurfaceCache, TextAlign, TextBasis, VisibleWindow, box_, clamp_rect, custom,
    custom_measured, divider, hstack, hstack_rev, layout, place_at_point, place_below_or_above,
    place_submenu, render, render_dirty, spacer, text, vdivider, virtual_list, vstack,
};

/// 将应用主题 [`crate::theme::Palette`] 注入到 `winc-cpui` 通用组件的薄适配封装。
pub(crate) mod widgets {
    use std::borrow::Cow;

    use crate::theme::Palette;
    use winc_cpui::Node;
    pub(crate) use winc_cpui::widgets::{ButtonKind, ButtonSpec, chevron, chip, field_slot};

    /// 构造标准圆角操作按钮。
    pub(crate) fn action_button<'a, A>(
        p: &Palette,
        size: f32,
        title: impl Into<Cow<'a, str>>,
        kind: ButtonKind,
        hovered: bool,
        action: A,
    ) -> Node<'a, A> {
        winc_cpui::widgets::action_button(&p.cpui_theme(), size, title, kind, hovered, action)
    }

    /// 构造支持启用/禁用状态和可选固定宽度的圆角操作按钮。
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
        winc_cpui::widgets::action_button_with(
            &p.cpui_theme(),
            size,
            title,
            kind,
            enabled,
            hovered,
            fixed_w,
            action,
        )
    }

    /// 构造对话框标准标题栏。
    pub(crate) fn dialog_header<'a, A>(
        p: &Palette,
        size: f32,
        title: impl Into<Cow<'a, str>>,
        close_hovered: bool,
        close_action: A,
    ) -> Node<'a, A> {
        winc_cpui::widgets::dialog_header(&p.cpui_theme(), size, title, close_hovered, close_action)
    }

    /// 构造对话框标准底部操作栏。
    pub(crate) fn dialog_footer<'a, A>(
        p: &Palette,
        sizes: (f32, f32),
        error: Option<Cow<'a, str>>,
        cancel: ButtonSpec<'a, A>,
        confirm: ButtonSpec<'a, A>,
    ) -> Node<'a, A> {
        winc_cpui::widgets::dialog_footer(&p.cpui_theme(), sizes, error, cancel, confirm)
    }

    /// 构造带左侧复选框图标与右侧标签的勾选项节点。
    pub(crate) fn checkbox<'a, A>(
        p: &Palette,
        size: f32,
        label: impl Into<Cow<'a, str>>,
        checked: bool,
        action: A,
    ) -> Node<'a, A> {
        winc_cpui::widgets::checkbox(&p.cpui_theme(), size, label, checked, action)
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use winc_render::{Color, FontKind, Rect, Renderer};

    use super::widgets::{self, ButtonKind, ButtonSpec};
    use super::{HitMap, TextBasis, render};
    use crate::layout::{DialogLayout, DialogPart};
    use crate::theme::LIGHT;

    #[test]
    fn dialog_header_and_footer_align_with_dialog_layout() {
        let mut renderer = Renderer::offscreen(600.0, 400.0, 1.0, Color::hex(0x1e1e1e), None, None)
            .expect("offscreen renderer initializes");
        let m = renderer.metrics(FontKind::Ui, 13.0);
        let basis = TextBasis {
            ui_size: 13.0,
            x_height: m.x_height,
            cap_height: m.cap_height,
        };
        let layout = DialogLayout::compute(Rect::new(0.0, 0.0, 600.0, 400.0), false, &[]);
        let p = &LIGHT;
        let mut header_hits = HitMap::new();
        let mut footer_hits = HitMap::new();

        renderer.draw(|frame| {
            let header = widgets::dialog_header(p, 13.0, "新建分支", false, DialogPart::Close);
            header_hits = render(frame, basis, layout.header, header);

            let footer_rect = Rect::new(
                layout.card.x + crate::theme::SETTINGS_PAD,
                layout.cancel.y,
                layout.confirm.right() - (layout.card.x + crate::theme::SETTINGS_PAD),
                layout.cancel.h,
            );
            let footer = widgets::dialog_footer(
                p,
                (13.0, 12.0),
                Some(Cow::Borrowed("分支名已存在")),
                ButtonSpec {
                    label: Cow::Borrowed("取消"),
                    kind: ButtonKind::Normal,
                    enabled: true,
                    hovered: false,
                    action: DialogPart::Cancel,
                },
                ButtonSpec {
                    label: Cow::Borrowed("创建"),
                    kind: ButtonKind::Primary,
                    enabled: true,
                    hovered: false,
                    action: DialogPart::Confirm,
                },
            );
            footer_hits = render(frame, basis, footer_rect, footer);
        });

        let close = header_hits.rect_of(&DialogPart::Close).unwrap();
        let cancel = footer_hits.rect_of(&DialogPart::Cancel).unwrap();
        let confirm = footer_hits.rect_of(&DialogPart::Confirm).unwrap();
        assert!((close.x - layout.close.x).abs() < 1.0 && (close.y - layout.close.y).abs() < 1.0);
        assert!(
            (cancel.x - layout.cancel.x).abs() < 1.0 && (cancel.w - layout.cancel.w).abs() < 1.0
        );
        assert!(
            (confirm.x - layout.confirm.x).abs() < 1.0
                && (confirm.w - layout.confirm.w).abs() < 1.0
        );
    }
}
