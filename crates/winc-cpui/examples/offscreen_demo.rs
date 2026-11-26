//! 独立离屏示例：展示 `winc-cpui` 脱离宿主应用与窗口系统的完整工作流。
//!
//! 运行方式：`cargo run -p winc-cpui --example offscreen_demo`

use std::borrow::Cow;
use std::cell::Cell;

use winc_cpui::widgets::{self, ButtonKind, ButtonSpec};
use winc_cpui::{
    CardFrame, HitMap, PopoverOptions, ScrollMetrics, ScrollbarSpec, SurfaceCache, TextBasis,
    Theme, VisibleWindow, box_, custom, divider, hstack, place_below_or_above, render,
    render_dirty, text, virtual_list, vstack,
};
use winc_render::{Color, FontKind, Rect, Renderer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DemoAction {
    Close,
    Row(usize),
    Cancel,
    Confirm,
}

fn demo_theme() -> Theme {
    Theme {
        text: Color::hex(0x1e1e1e),
        text_muted: Color::hex(0x666666),
        text_dim: Color::hex(0x999999),
        accent: Color::hex(0x2563eb),
        accent_fill: Color::hex(0x2563eb),
        text_on_accent: Color::hex(0xffffff),
        error: Color::hex(0xdc2626),
        hover: Color::hex(0xe5e7eb),
        selected_inactive: Color::hex(0xd1d5db),
        control_height: 28.0,
        icon_button_size: 28.0,
        dialog_button_width: 84.0,
        row_radius: 6.0,
        panel_pad: 20.0,
    }
}

fn main() {
    let viewport = Rect::new(0.0, 0.0, 640.0, 420.0);
    let mut renderer = Renderer::offscreen(
        viewport.w,
        viewport.h,
        1.0,
        Color::hex(0xf3f4f6),
        None,
        None,
    )
    .expect("offscreen renderer initializes");
    let metrics = renderer.metrics(FontKind::Ui, 13.0);
    let basis = TextBasis {
        ui_size: 13.0,
        x_height: metrics.x_height,
        cap_height: metrics.cap_height,
    };
    let theme = demo_theme();

    // 1. 弹出层自动避让视口边界。
    let caret = Rect::new(480.0, 350.0, 2.0, 18.0);
    let popover_rect = place_below_or_above(
        caret,
        viewport,
        (220.0, 140.0),
        PopoverOptions {
            margin: 8.0,
            gap: 4.0,
            ..PopoverOptions::default()
        },
    );

    // 2. 10,000 行虚拟列表 + 对话框首帧渲染与快照捕获。
    let total_rows = 10_000usize;
    let row_height = 24.0;
    let list_height = 160.0;
    let scroll =
        ScrollMetrics::new(list_height, total_rows as f32 * row_height, 0.0).scroll_by(12_010.0);
    let scrollbar = ScrollbarSpec {
        thickness: 6.0,
        inset: 3.0,
        end_margin: 4.0,
        min_thumb: 24.0,
    };
    let thumb = scrollbar.vertical_thumb(
        Rect::new(0.0, 0.0, 420.0, list_height),
        scroll,
        total_rows as f32 * row_height,
    );

    let dialog_rect = Rect::new(110.0, 60.0, 420.0, 280.0);
    let mut cache = SurfaceCache::default();
    let mut built_rows = 0usize;
    let mut hits = HitMap::new();

    renderer.draw(|frame| {
        let window = VisibleWindow::fixed(total_rows, row_height, list_height, scroll);
        let list = virtual_list(window, |i| {
            built_rows += 1;
            hstack()
                .w_full()
                .h(row_height)
                .px(16.0)
                .items_center()
                .flex_shrink(0.0)
                .hit(DemoAction::Row(i))
                .child(
                    text(format!("虚拟列表条目 #{i}"))
                        .text_size(13.0)
                        .color(theme.text),
                )
        })
        .h(list_height);

        let dialog = vstack()
            .w_full()
            .h_full()
            .bg(Color::hex(0xffffff))
            .rounded(10.0)
            .card_frame(CardFrame {
                shadow: Color::hex(0x000000).alpha(0.18),
                border_base: Color::hex(0xffffff),
                border: Color::hex(0xd4d4d4),
            })
            .child(box_().w_full().h(44.0).child(widgets::dialog_header(
                &theme,
                13.0,
                "CPUI 离屏示例",
                false,
                DemoAction::Close,
            )))
            .child(divider(Color::hex(0xe5e7eb)))
            .child(list)
            .child(divider(Color::hex(0xe5e7eb)))
            .child(
                box_()
                    .w_full()
                    .h(44.0)
                    .px(theme.panel_pad)
                    .child(widgets::dialog_footer(
                        &theme,
                        (13.0, 12.0),
                        None,
                        ButtonSpec {
                            label: Cow::Borrowed("取消"),
                            kind: ButtonKind::Normal,
                            enabled: true,
                            hovered: false,
                            action: DemoAction::Cancel,
                        },
                        ButtonSpec {
                            label: Cow::Borrowed("确定"),
                            kind: ButtonKind::Primary,
                            enabled: true,
                            hovered: true,
                            action: DemoAction::Confirm,
                        },
                    )),
            );

        hits = render(frame, basis, dialog_rect, dialog);
        cache.capture(frame, viewport, 1u32);
    });

    // 3. 第二帧命中 SurfaceCache 快照，并用 render_dirty 仅局部重绘右侧徽章。
    let dirty_calls = Cell::new(0usize);
    renderer.draw(|frame| {
        let reused = cache.restore(frame, viewport, &1u32, Color::hex(0).alpha(0.0));
        assert!(reused, "backdrop snapshot should be reused");
        let overlay = hstack::<()>()
            .w(200.0)
            .h(28.0)
            .child(custom((100.0, 28.0), |_, _, _| {
                dirty_calls.set(dirty_calls.get() + 1);
            }))
            .child(custom((100.0, 28.0), |_, _, _| {
                dirty_calls.set(dirty_calls.get() + 1);
            }));
        let _ = render_dirty(
            frame,
            basis,
            Rect::new(20.0, 20.0, 200.0, 28.0),
            Rect::new(130.0, 20.0, 80.0, 28.0),
            overlay,
        );
    });
    cache.clear();

    let confirm_rect = hits
        .rect_of(&DemoAction::Confirm)
        .expect("confirm hit rect");
    println!("popover_rect   = {popover_rect:?}");
    println!("virtual_rows   = {built_rows} / {total_rows} (scroll={scroll:.1}px)");
    println!("scrollbar_thumb= {thumb:?}");
    println!("confirm_hit    = {confirm_rect:?}");
    println!("cache_reuses   = {}", cache.reuses());
    println!("dirty_paints   = {} / 2", dirty_calls.get());
}
