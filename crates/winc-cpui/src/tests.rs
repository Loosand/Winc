//! `winc-cpui` 离屏布局、绘制、命中测试与快照缓存单元测试。

use std::borrow::Cow;

use winc_render::{Color, FontKind, Rect, Renderer};

use crate::widgets::{ButtonKind, ButtonSpec, Theme};
use crate::*;

fn offscreen_renderer() -> Renderer {
    Renderer::offscreen(600.0, 400.0, 1.0, Color::hex(0x1e1e1e), None, None)
        .expect("offscreen renderer initializes")
}

fn basis_for(renderer: &Renderer) -> TextBasis {
    let m = renderer.metrics(FontKind::Ui, 13.0);
    TextBasis {
        ui_size: 13.0,
        x_height: m.x_height,
        cap_height: m.cap_height,
    }
}

fn sample_theme() -> Theme {
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

#[test]
fn hit_map_prefers_topmost_entry() {
    let mut hits = HitMap::new();
    hits.push(Rect::new(0.0, 0.0, 100.0, 100.0), "outer");
    hits.push(Rect::new(20.0, 20.0, 40.0, 40.0), "inner");

    assert_eq!(hits.at(10.0, 10.0), Some(&"outer"));
    assert_eq!(hits.at(30.0, 30.0), Some(&"inner"));
    assert_eq!(hits.at(150.0, 10.0), None);
    assert_eq!(
        hits.rect_of(&"inner"),
        Some(Rect::new(20.0, 20.0, 40.0, 40.0))
    );
}

#[test]
fn flexbox_hstack_rev_orders_first_child_rightmost() {
    let mut renderer = offscreen_renderer();
    let basis = basis_for(&renderer);
    let mut hits = HitMap::new();

    renderer.draw(|frame| {
        let tree = hstack_rev()
            .w(240.0)
            .h(32.0)
            .gap(8.0)
            .child(box_().size(60.0, 28.0).hit(0usize))
            .child(box_().size(60.0, 28.0).hit(1usize))
            .child(box_().size(60.0, 28.0).hit(2usize));
        hits = render(frame, basis, Rect::new(10.0, 20.0, 240.0, 32.0), tree);
    });

    let r0 = hits.rect_of(&0).unwrap();
    let r1 = hits.rect_of(&1).unwrap();
    let r2 = hits.rect_of(&2).unwrap();
    assert!(r0.x > r1.x && r1.x > r2.x);
    assert!((r0.x - (r1.right() + 8.0)).abs() < 0.01);
    assert!((r1.x - (r2.right() + 8.0)).abs() < 0.01);
}

#[test]
fn wrapped_text_grows_height_under_max_width() {
    let mut renderer = offscreen_renderer();
    let basis = basis_for(&renderer);
    let mut narrow = (0.0, 0.0);
    let mut wide = (0.0, 0.0);
    let mut short = (0.0, 0.0);

    let sample =
        "这是一段足够长的提示文字，用来验证在最大宽度约束下声明式文本节点会自动折行并增加高度。";
    renderer.draw(|frame| {
        let node = vstack()
            .max_w(160.0)
            .p(12.0)
            .child(text::<()>(sample).text_size(13.0).line_height(20.0).wrap());
        narrow = layout(frame, basis, (None, None), node).root_size();

        let node = vstack()
            .max_w(560.0)
            .p(12.0)
            .child(text::<()>(sample).text_size(13.0).line_height(20.0).wrap());
        wide = layout(frame, basis, (None, None), node).root_size();

        let node = vstack()
            .max_w(560.0)
            .p(12.0)
            .child(text::<()>("短句").text_size(13.0).line_height(20.0).wrap());
        short = layout(frame, basis, (None, None), node).root_size();
    });

    assert!(narrow.0 <= 160.0 + 0.5, "narrow width={}", narrow.0);
    assert!(narrow.1 > wide.1 + 19.0, "narrow={narrow:?}, wide={wide:?}");
    assert!(
        short.0 < 120.0,
        "short text should shrink-to-fit, got {}",
        short.0
    );
}

#[test]
fn overflow_hidden_and_scroll_y_clip_hit_rects() {
    let mut renderer = offscreen_renderer();
    let basis = basis_for(&renderer);
    let mut hits = HitMap::new();

    renderer.draw(|frame| {
        let list = vstack()
            .w(120.0)
            .h(50.0)
            .scroll_y(30.0)
            .child(box_().size(120.0, 30.0).flex_shrink(0.0).hit("item0"))
            .child(box_().size(120.0, 30.0).flex_shrink(0.0).hit("item1"))
            .child(box_().size(120.0, 30.0).flex_shrink(0.0).hit("item2"));
        hits = render(frame, basis, Rect::new(0.0, 0.0, 120.0, 50.0), list);
    });

    assert_eq!(hits.rect_of(&"item0"), None);
    assert_eq!(hits.at(10.0, 15.0), Some(&"item1"));
    assert_eq!(hits.at(10.0, 40.0), Some(&"item2"));
    assert_eq!(hits.at(10.0, 55.0), None);
}

#[test]
fn flex_weight_distributes_columns_proportionally() {
    let mut renderer = offscreen_renderer();
    let basis = basis_for(&renderer);
    let mut hits = HitMap::new();

    renderer.draw(|frame| {
        let row = hstack()
            .w_full()
            .h(30.0)
            .child(box_().flex_weight(4.0).h_full().hit("month"))
            .child(box_().flex_weight(1.0).h_full().hit("today"))
            .child(box_().flex_weight(1.0).h_full().hit("prev"))
            .child(box_().flex_weight(1.0).h_full().hit("next"));
        hits = render(frame, basis, Rect::new(0.0, 0.0, 280.0, 30.0), row);
    });

    let month = hits.rect_of(&"month").unwrap();
    let today = hits.rect_of(&"today").unwrap();
    let prev = hits.rect_of(&"prev").unwrap();
    let next = hits.rect_of(&"next").unwrap();
    assert!((month.w - 160.0).abs() < 0.5, "month.w={}", month.w);
    assert!((today.w - 40.0).abs() < 0.5, "today.w={}", today.w);
    assert!((prev.w - 40.0).abs() < 0.5, "prev.w={}", prev.w);
    assert!((next.w - 40.0).abs() < 0.5, "next.w={}", next.w);
    assert!((next.right() - 280.0).abs() < 0.5);
}

#[test]
fn divider_chip_and_field_slot_compose_cleanly() {
    let mut renderer = offscreen_renderer();
    let basis = basis_for(&renderer);
    let mut field_rect = Rect::default();
    let mut hits = HitMap::new();

    renderer.draw(|frame| {
        let tree = vstack()
            .w_full()
            .h_full()
            .child(widgets::field_slot(32.0, |_, rect, _| {
                field_rect = rect;
            }))
            .child(divider(Color::hex(0x444444)))
            .child(
                hstack()
                    .w_full()
                    .h(28.0)
                    .px(6.0)
                    .items_center()
                    .justify_between()
                    .child(widgets::chip(
                        "超长标签内容用于验证徽章不会撑破父行宽度",
                        Color::hex(0xffffff),
                        Color::hex(0x336699),
                        13.0,
                        21.0,
                    ))
                    .child(box_().size(24.0, 24.0).flex_shrink(0.0).hit("check")),
            );
        hits = render(frame, basis, Rect::new(10.0, 20.0, 180.0, 100.0), tree);
    });

    assert_eq!(field_rect, Rect::new(10.0, 20.0, 180.0, 32.0));
    let check = hits.rect_of(&"check").unwrap();
    assert!((check.right() - 184.0).abs() < 1.0, "check={check:?}");
    assert!(
        check.y >= 53.0,
        "check should sit below field + 1px divider"
    );
}

#[test]
fn surface_cache_captures_restores_and_invalidates_cleanly() {
    let mut renderer = offscreen_renderer();
    let mut cache = SurfaceCache::default();
    let area = Rect::new(0.0, 0.0, 200.0, 120.0);
    let tint = Color::hex(0).alpha(0.0);

    renderer.draw(|frame| {
        assert!(!cache.restore(frame, area, &1u32, tint));
        frame.rect(area, Color::hex(0x224466));
        cache.capture(frame, area, 1u32);
    });
    assert!(cache.is_valid());
    assert_eq!(cache.reuses(), 0);

    renderer.draw(|frame| {
        assert!(cache.restore(frame, area, &1u32, tint));
        assert!(!cache.restore(frame, area, &2u32, tint));
        assert!(!cache.restore(frame, Rect::new(0.0, 0.0, 180.0, 120.0), &1u32, tint));
    });
    assert_eq!(cache.reuses(), 1);

    cache.invalidate();
    assert!(!cache.is_valid());
    renderer.draw(|frame| {
        assert!(!cache.restore(frame, area, &1u32, tint));
    });
    cache.clear();
    assert!(!cache.is_valid());
}

#[test]
fn themed_widgets_render_and_collect_hits_without_host_dependencies() {
    let mut renderer = offscreen_renderer();
    let basis = basis_for(&renderer);
    let theme = sample_theme();
    let mut hits = HitMap::new();

    renderer.draw(|frame| {
        let dialog = vstack()
            .w_full()
            .h_full()
            .child(
                box_()
                    .w_full()
                    .h(44.0)
                    .child(widgets::dialog_header(&theme, 13.0, "标题", true, "close")),
            )
            .child(box_().w_full().px(theme.panel_pad).child(widgets::checkbox(
                &theme,
                13.0,
                "记住选择",
                true,
                "remember",
            )))
            .child(spacer())
            .child(
                box_()
                    .w_full()
                    .h(44.0)
                    .px(theme.panel_pad)
                    .child(widgets::dialog_footer(
                        &theme,
                        (13.0, 12.0),
                        Some(Cow::Borrowed("校验提示")),
                        ButtonSpec {
                            label: Cow::Borrowed("取消"),
                            kind: ButtonKind::Normal,
                            enabled: true,
                            hovered: false,
                            action: "cancel",
                        },
                        ButtonSpec {
                            label: Cow::Borrowed("确定"),
                            kind: ButtonKind::Primary,
                            enabled: true,
                            hovered: true,
                            action: "confirm",
                        },
                    )),
            );
        hits = render(frame, basis, Rect::new(0.0, 0.0, 420.0, 180.0), dialog);
    });

    assert!(hits.rect_of(&"close").is_some());
    assert!(hits.rect_of(&"remember").is_some());
    let cancel = hits.rect_of(&"cancel").unwrap();
    let confirm = hits.rect_of(&"confirm").unwrap();
    assert!(confirm.x > cancel.right());
    assert!((confirm.w - theme.dialog_button_width).abs() < 0.5);
}

#[test]
fn popover_places_below_and_flips_above_at_viewport_edges() {
    let viewport = Rect::new(0.0, 0.0, 400.0, 300.0);
    let opts = PopoverOptions {
        margin: 8.0,
        gap: 4.0,
        offset_x: -20.0,
        prefer_larger_side: false,
        min_height: None,
    };

    // 光标在上方且空间充足：紧贴光标下方，并向左按 offset_x 对齐且不越过左边界。
    let top_anchor = Rect::new(10.0, 20.0, 2.0, 18.0);
    let placed = place_below_or_above(top_anchor, viewport, (180.0, 120.0), opts);
    assert_eq!(placed, Rect::new(8.0, 42.0, 180.0, 120.0));

    // 光标接近底边且靠右：自动翻到光标上方，同时夹紧到右边界留白内。
    let bottom_anchor = Rect::new(350.0, 230.0, 2.0, 18.0);
    let flipped = place_below_or_above(bottom_anchor, viewport, (180.0, 120.0), opts);
    assert_eq!(flipped, Rect::new(212.0, 106.0, 180.0, 120.0));

    // prefer_larger_side + min_height：下方空间比上方大时留在下方并按剩余高度裁剪。
    let mid_anchor = Rect::new(40.0, 60.0, 120.0, 24.0);
    let cropped = place_below_or_above(
        mid_anchor,
        viewport,
        (240.0, 260.0),
        PopoverOptions {
            margin: 8.0,
            gap: 4.0,
            offset_x: 0.0,
            prefer_larger_side: true,
            min_height: Some(36.0),
        },
    );
    assert_eq!(cropped, Rect::new(40.0, 88.0, 240.0, 204.0));
}

#[test]
fn point_menu_and_submenu_respect_alignment_and_flip_left() {
    let viewport = Rect::new(0.0, 0.0, 400.0, 300.0);

    let level0 = place_at_point(
        viewport,
        (160.0, 100.0),
        PointPlacement {
            point: (380.0, 80.0),
            margin: 8.0,
            right_aligned: true,
            anchor_y_offset: 30.0,
        },
    );
    assert_eq!(level0, Rect::new(220.0, 50.0, 160.0, 100.0));

    // 父菜单靠右时，子菜单在右侧放不下，应自动翻转到父菜单左侧并保留 2px 重叠。
    let trigger = Rect::new(level0.x, level0.y + 28.0, level0.w, 24.0);
    let sub = place_submenu(
        viewport,
        (140.0, 90.0),
        SubmenuPlacement {
            parent_card: level0,
            trigger_row: trigger,
            margin: 8.0,
            overlap: 2.0,
            pad_y: 4.0,
        },
    );
    assert_eq!(sub, Rect::new(82.0, 74.0, 140.0, 90.0));

    // 极窄视口时 clamp_rect 保持宽度归零或缩至可用区，贴靠左上留白。
    let clamped = clamp_rect(
        Rect::new(50.0, 50.0, 200.0, 100.0),
        Rect::new(0.0, 0.0, 10.0, 10.0),
        8.0,
    );
    assert_eq!(clamped, Rect::new(8.0, 8.0, 0.0, 0.0));
}

#[test]
fn scroll_metrics_and_scrollbar_spec_round_trip() {
    let m = ScrollMetrics::new(100.0, 350.0, 40.0);
    assert!(m.can_scroll());
    assert_eq!(m.max_offset(), 250.0);
    assert_eq!(m.scroll_by(300.0), 250.0);
    assert_eq!(m.scroll_by(-80.0), 0.0);
    assert_eq!(m.ensure_visible(20.0, 24.0), 20.0);
    assert_eq!(m.ensure_visible(60.0, 24.0), 40.0);
    assert_eq!(m.ensure_visible(130.0, 24.0), 54.0);

    let spec = ScrollbarSpec {
        thickness: 6.0,
        inset: 3.0,
        end_margin: 4.0,
        min_thumb: 24.0,
    };
    let track = Rect::new(10.0, 20.0, 120.0, 200.0);
    assert_eq!(spec.vertical_thumb(track, 0.0, 200.0), None);
    let top = spec.vertical_thumb(track, 0.0, 400.0).unwrap();
    let mid = spec.vertical_thumb(track, 100.0, 400.0).unwrap();
    let bottom = spec.vertical_thumb(track, 200.0, 400.0).unwrap();
    assert_eq!(top.x, track.right() - 9.0);
    assert_eq!(top.y, track.y + 4.0);
    assert_eq!(bottom.bottom(), track.bottom() - 4.0);

    let dragged = spec.offset_for_thumb(track.y, track.h, mid.h, mid.y + 10.0, 10.0, 200.0);
    assert!((dragged - 100.0).abs() < 0.01);
}
