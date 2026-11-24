//! 可编辑 Markdown 的鼠标选区：平台连击、整段拖选和后台解析期间的行为。

use super::*;

#[test]
fn markdown_selection_uses_the_click_count_supplied_by_the_platform() {
    let paragraph = "前文 Markdown 后文\n接在同一段";
    let source = format!("{paragraph}\n\n下一段\n");
    let (mut app, dir) = live_doc("md-native-click-count", &source);
    let shown = crate::app::preview::markdown::laid::full_text(&app).unwrap();
    let offset = shown[..shown.find("Markdown").unwrap()].chars().count() + 2;
    let (x, y) = point_of(&app, offset);
    app.on_mouse_move(x, y);

    // 没有前三次应用内按下的记录，也能凭平台事件选段、选词。
    let word = "前文 ".chars().count();
    for (count, expected) in [(3, 0..paragraph.chars().count()), (2, word..word + 8)] {
        app.on_mouse_down_with_click_count(MouseButton::Left, Some(count));
        app.on_mouse_up(MouseButton::Left);
        assert_eq!(app.editor.active().unwrap().selection.range(), expected);
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn selecting_a_whole_list_item_includes_its_wikilink_and_bullet() {
    let first = format!("- [[链接]] {}末尾", "一段折行文字。".repeat(14));
    let source = format!("{first}\n- 未选择\n");
    for (reverse, count) in [(false, 1), (true, 1), (false, 3)] {
        let (mut app, dir) = live_doc("md-select-wiki-item", &source);
        let shown = crate::app::preview::markdown::laid::full_text(&app).unwrap();
        let point =
            |needle: &str| point_of(&app, shown[..shown.find(needle).unwrap()].chars().count());
        let from = point("链接");
        let end = point("末尾");
        assert!(end.1 > from.1);
        let to = (end.0 + 42.0, end.1);
        let (start, end) = if reverse { (to, from) } else { (from, to) };
        app.on_mouse_move(start.0, start.1);
        app.on_mouse_down_with_click_count(MouseButton::Left, Some(count));
        if count == 1 {
            app.on_mouse_move(end.0, end.1);
        }
        app.on_mouse_up(MouseButton::Left);
        settle(&mut app);
        let doc = app.editor.active().unwrap();
        assert_eq!(
            doc.selection.range(),
            0..first.chars().count(),
            "{reverse} {count}"
        );
        let shown = crate::app::preview::markdown::laid::full_text(&app).unwrap();
        assert!(shown.contains("- [[链接]]"), "{shown}");
        assert!(!shown.contains("- 未选择"), "{shown}");
        let laid = app.preview.markdown.layout.as_ref().unwrap();
        assert!(laid.blocks[0].1.items.iter().any(|item| matches!(
            item,
            winc_typeset::Item::Text { text, span: Some(span), .. }
                if text.as_ref() == "- " && span.source == Some(0)
        )));
        assert!(app.run_action(crate::keymap::Action::MarkdownCopy));
        assert_eq!(app.clipboard.copied.as_deref(), Some(first.as_str()));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn dragging_part_of_a_wikilink_keeps_the_selection_inside_the_label() {
    let (mut app, dir) = live_doc("md-select-part-of-link", "- [[链接标题]] 说明\n");
    let shown = crate::app::preview::markdown::laid::full_text(&app).unwrap();
    let at = shown[..shown.find("链接标题").unwrap()].chars().count();
    let (x, y) = point_of(&app, at);
    let (to_x, to_y) = point_of(&app, at + 2);
    app.on_mouse_move(x, y);
    app.on_mouse_down_with_click_count(MouseButton::Left, Some(1));
    app.on_mouse_move(to_x, to_y);
    app.on_mouse_up(MouseButton::Left);
    let doc = app.editor.active().unwrap();
    assert_eq!(doc.selection.range(), 4..6);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn triple_click_keeps_a_wrapped_list_item_selected_during_mouse_movement() {
    let item = format!(
        "[组件库](https://example.com) （图赏）：{}末尾。",
        "这条列表项很长，应该一直选到最后。".repeat(8)
    );
    let source = format!("- 前一项\n- {item}\n- 后一项\n");
    let (mut app, dir) = live_doc("md-triple-wrapped-item", &source);
    let shown = crate::app::preview::markdown::laid::full_text(&app).unwrap();
    let point = |needle: &str| point_of(&app, shown[..shown.find(needle).unwrap()].chars().count());
    let (x, y) = point("图赏");
    assert!(point("末尾").1 > y, "the list item should wrap");
    let start = "- 前一项\n".chars().count();
    let expected = start..start + "- ".chars().count() + item.chars().count();
    super::click(&mut app, (x, y));
    super::click(&mut app, (x, y));
    app.on_mouse_down(MouseButton::Left);
    let before_move = app.editor.active().unwrap().selection.range();
    app.on_mouse_move(x + 2.0, y);
    app.on_mouse_up(MouseButton::Left);
    let after_move = app.editor.active().unwrap().selection.range();
    assert_eq!(after_move, before_move);
    assert_eq!(after_move, expected);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn triple_click_drag_extends_by_markdown_paragraphs_in_either_direction() {
    let first = "前段的开头\r\n前段的结尾";
    let middle = "中段的开头\r\n中段的结尾";
    let last = "后段的开头\r\n后段的结尾";
    let source = format!("{first}\r\n\r\n{middle}\r\n\r\n{last}\r\n");
    let start = first.chars().count() + 4;
    let end = start + middle.chars().count();
    let source_end = source.chars().count() - 2;
    for (target, expected) in [
        ("前段的结尾", winc_core::Selection::new(end, 0)),
        ("后段的开头", winc_core::Selection::new(start, source_end)),
    ] {
        let (mut app, dir) = live_doc("md-triple-source-lines", &source);
        let shown = crate::app::preview::markdown::laid::full_text(&app).unwrap();
        let point = |needle: &str| {
            point_of(
                &app,
                shown[..shown.find(needle).unwrap()].chars().count() + 1,
            )
        };
        let origin = point("中段的结尾");
        let (x, y) = point(target);
        super::click(&mut app, origin);
        super::click(&mut app, origin);
        app.on_mouse_down(MouseButton::Left);
        assert_eq!(app.editor.active().unwrap().selection.range(), start..end);
        app.on_mouse_move(x, y);
        app.on_mouse_up(MouseButton::Left);
        assert_eq!(app.editor.active().unwrap().selection, expected);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn triple_click_uses_unchanged_paragraphs_while_the_current_revision_is_parsing() {
    let first = "第一行文字\n第二行文字";
    let (mut app, dir) = live_doc(
        "md-triple-unchanged",
        &format!("{first}\n\n中间段\n\n末段\n"),
    );
    put_caret(&mut app, "末段", true);
    let shown = crate::app::preview::markdown::laid::full_text(&app).unwrap();
    let at = shown[..shown.find("第二行").unwrap()].chars().count() + 1;
    let (x, y) = point_of(&app, at);
    app.preview.markdown.cost = Some(std::time::Duration::from_secs(1));
    app.run(winc_editor::Command::Insert("改".into()));
    app.on_mouse_move(x, y);
    app.on_mouse_down_with_click_count(MouseButton::Left, Some(3));
    let expected = 0..first.chars().count();
    assert_eq!(app.editor.active().unwrap().selection.range(), expected);
    settle(&mut app);
    app.on_mouse_move(x + 1.0, y);
    app.on_mouse_up(MouseButton::Left);
    assert_eq!(app.editor.active().unwrap().selection.range(), expected);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn triple_click_drag_keeps_its_initial_range_when_parsing_catches_up() {
    let (mut app, dir) = live_doc("md-triple-parsing", "第一行文字\n第二行文字\n\n下一段\n");
    put_caret(&mut app, "第二行文字", true);
    let shown = crate::app::preview::markdown::laid::full_text(&app).unwrap();
    let at = shown[..shown.find("第二行").unwrap()].chars().count() + 1;
    let (x, y) = point_of(&app, at);
    app.preview.markdown.cost = Some(std::time::Duration::from_secs(1));
    app.run(winc_editor::Command::Insert("改".into()));
    app.on_mouse_move(x, y);
    app.on_mouse_down_with_click_count(MouseButton::Left, Some(3));
    let before = app.editor.active().unwrap().selection;
    let first_row = "第一行文字\n".chars().count();
    let paragraph = "第一行文字\n第二行文字改".chars().count();
    assert_eq!(before.range(), first_row..paragraph + 1);
    settle(&mut app);
    app.on_mouse_move(x + 1.0, y);
    app.on_mouse_up(MouseButton::Left);
    assert_eq!(app.editor.active().unwrap().selection, before);
    app.on_mouse_down_with_click_count(MouseButton::Left, Some(3));
    app.on_mouse_up(MouseButton::Left);
    assert_eq!(app.editor.active().unwrap().selection.range(), 0..paragraph);
    std::fs::remove_dir_all(dir).unwrap();
}
