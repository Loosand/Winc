//! 以外部消费方视角验证 `winc-cpui` 全部公开 API 契约的离屏集成测试。

use std::borrow::Cow;
use std::cell::Cell;

use winc_cpui::widgets::{self, ButtonKind, ButtonSpec};
use winc_cpui::{
    CardFrame, HitMap, PointPlacement, PopoverOptions, ScrollMetrics, ScrollbarSpec,
    SubmenuPlacement, SurfaceCache, TextBasis, Theme, VisibleWindow, box_, custom, divider, hstack,
    place_at_point, place_below_or_above, place_submenu, render, render_dirty, text, virtual_list,
    vstack, wrap_text,
};
use winc_render::{Color, FontKind, Rect, Renderer};

fn test_theme() -> Theme {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Close,
    Row(usize),
    Cancel,
    Confirm,
}

#[test]
fn external_consumer_end_to_end_dialog_virtual_list_cache_and_dirty_paint() {
    let mut renderer = Renderer::offscreen(480.0, 320.0, 1.0, Color::hex(0xf5f5f5), None, None)
        .expect("offscreen renderer initializes");
    let metrics = renderer.metrics(FontKind::Ui, 13.0);
    let basis = TextBasis {
        ui_size: 13.0,
        x_height: metrics.x_height,
        cap_height: metrics.cap_height,
    };
    let theme = test_theme();

    // 1. 弹出层与子菜单定位原语。
    let viewport = Rect::new(0.0, 0.0, 480.0, 320.0);
    let popover = place_below_or_above(
        Rect::new(20.0, 260.0, 2.0, 18.0),
        viewport,
        (220.0, 120.0),
        PopoverOptions {
            margin: 8.0,
            gap: 4.0,
            ..PopoverOptions::default()
        },
    );
    assert_eq!(popover.y, 136.0);

    let menu = place_at_point(
        viewport,
        (160.0, 100.0),
        PointPlacement {
            point: (450.0, 40.0),
            margin: 8.0,
            right_aligned: true,
            anchor_y_offset: 0.0,
        },
    );
    let sub = place_submenu(
        viewport,
        (140.0, 80.0),
        SubmenuPlacement {
            parent_card: menu,
            trigger_row: Rect::new(menu.x, menu.y + 24.0, menu.w, 24.0),
            margin: 8.0,
            overlap: 2.0,
            pad_y: 4.0,
        },
    );
    assert!(sub.x < menu.x);

    // 2. 折行工具与滚动几何原语。
    let wrapped = wrap_text("hello world cpui", 60.0, |s| s.len() as f32 * 8.0);
    assert!(wrapped.len() >= 2);
    let scroll = ScrollMetrics::new(140.0, 240_000.0, 2_400.0);
    assert_eq!(scroll.ensure_visible(2_520.0, 24.0), 2_404.0);
    let spec = ScrollbarSpec {
        thickness: 6.0,
        inset: 3.0,
        end_margin: 4.0,
        min_thumb: 24.0,
    };
    assert!(
        spec.vertical_thumb(Rect::new(0.0, 0.0, 300.0, 140.0), 2_400.0, 240_000.0)
            .is_some()
    );

    // 3. 声明式弹窗 + 10,000 行虚拟列表 + 帧间快照缓存 + 局部脏区重绘。
    let dialog_bounds = Rect::new(40.0, 20.0, 400.0, 260.0);
    let mut cache = SurfaceCache::default();
    let mut built_rows = 0usize;
    let mut hits = HitMap::new();

    renderer.draw(|frame| {
        let window = VisibleWindow::fixed(10_000, 24.0, 140.0, 2_400.0);
        let list = virtual_list(window, |i| {
            built_rows += 1;
            hstack()
                .w_full()
                .h(24.0)
                .px(12.0)
                .items_center()
                .flex_shrink(0.0)
                .hit(Action::Row(i))
                .child(text(format!("条目 #{i}")).text_size(13.0).color(theme.text))
        })
        .h(140.0);

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
                "万行列表",
                false,
                Action::Close,
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
                            action: Action::Cancel,
                        },
                        ButtonSpec {
                            label: Cow::Borrowed("确定"),
                            kind: ButtonKind::Primary,
                            enabled: true,
                            hovered: false,
                            action: Action::Confirm,
                        },
                    )),
            );

        hits = render(frame, basis, dialog_bounds, dialog);
        cache.capture(frame, viewport, 1u32);
    });

    assert!(built_rows <= 7, "only visible rows built, got {built_rows}");
    assert!(hits.rect_of(&Action::Row(100)).is_some());
    assert!(hits.rect_of(&Action::Confirm).is_some());

    // 下一帧命中快照缓存，并只重绘局部脏区。
    let left_painted = Cell::new(0usize);
    let right_painted = Cell::new(0usize);
    renderer.draw(|frame| {
        assert!(cache.restore(frame, viewport, &1u32, Color::hex(0).alpha(0.0)));
        let strip = hstack::<()>()
            .w(200.0)
            .h(32.0)
            .child(custom((100.0, 32.0), |_, _, _| {
                left_painted.set(left_painted.get() + 1);
            }))
            .child(custom((100.0, 32.0), |_, _, _| {
                right_painted.set(right_painted.get() + 1);
            }));
        let _ = render_dirty(
            frame,
            basis,
            Rect::new(0.0, 0.0, 200.0, 32.0),
            Rect::new(110.0, 0.0, 80.0, 32.0),
            strip,
        );
    });
    assert_eq!(cache.reuses(), 1);
    assert_eq!(left_painted.get(), 0);
    assert_eq!(right_painted.get(), 1);
    cache.clear();
    assert!(!cache.is_valid());
}
