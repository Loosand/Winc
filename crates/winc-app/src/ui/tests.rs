//! 声明式布局与命中表（`crate::ui`）单元测试。

use winc_render::{Color, FontKind, Rect, Renderer};

use super::*;

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

    // item0 完全滚出顶部（y = -30..0），被裁剪掉不应产生命中区域。
    assert_eq!(hits.rect_of(&"item0"), None);
    // item1 滚到顶部可见区（y = 0..30）。
    assert_eq!(hits.at(10.0, 15.0), Some(&"item1"));
    // item2 部分可见（y = 30..50），超出容器底部 y = 55 处不能命中。
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
    // 即使左侧徽章文本很长，右侧 24px 勾选标记仍应稳稳停在卡片右侧内部。
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

    // 同一区域、同一缩放、同一键：直接复用快照。
    renderer.draw(|frame| {
        assert!(cache.restore(frame, area, &1u32, tint));
        // 不同键或不同尺寸不能命中。
        assert!(!cache.restore(frame, area, &2u32, tint));
        assert!(!cache.restore(frame, Rect::new(0.0, 0.0, 180.0, 120.0), &1u32, tint));
    });
    assert_eq!(cache.reuses(), 1);

    // invalidate 失效后不再命中；clear 释放缓冲。
    cache.invalidate();
    assert!(!cache.is_valid());
    renderer.draw(|frame| {
        assert!(!cache.restore(frame, area, &1u32, tint));
    });
    cache.clear();
    assert!(!cache.is_valid());
}

#[test]
fn dialog_header_and_footer_align_with_dialog_layout() {
    use std::borrow::Cow;

    use crate::layout::{DialogLayout, DialogPart};
    use crate::theme::LIGHT;
    use crate::ui::widgets::{ButtonKind, ButtonSpec};

    let mut renderer = offscreen_renderer();
    let basis = basis_for(&renderer);
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
    assert!((cancel.x - layout.cancel.x).abs() < 1.0 && (cancel.w - layout.cancel.w).abs() < 1.0);
    assert!(
        (confirm.x - layout.confirm.x).abs() < 1.0 && (confirm.w - layout.confirm.w).abs() < 1.0
    );
}
