//! 可编辑 Markdown 的鼠标选区：平台连击、整段拖选和后台解析期间的行为。

use super::*;

#[test]
fn cross_block_drag_keeps_the_same_layout_and_endpoints_on_release() {
    let source = "### 标题\n\n- [第一项](https://a.example)\n- [第二项](https://b.example)\n";
    for reverse in [false, true] {
        let (mut app, dir) = live_doc("md-select-across-heading", source);
        let shown = crate::app::preview::markdown::laid::full_text(&app).unwrap();
        let point =
            |needle: &str| point_of(&app, shown[..shown.find(needle).unwrap()].chars().count());
        let heading = point("标题");
        let second = point("第二项");
        let end = (second.0 + 100.0, second.1);
        let (from, to) = if reverse {
            (end, heading)
        } else {
            (heading, end)
        };
        let tops = app.preview.markdown.layout.as_ref().unwrap().tops.clone();
        app.on_mouse_move(from.0, from.1);
        app.on_mouse_down_with_click_count(MouseButton::Left, Some(1));
        app.on_mouse_move(to.0, to.1);
        paint(&mut app);
        let selection = app.editor.active().unwrap().selection;
        assert!(!selection.is_empty());
        assert_eq!(selection.anchor > selection.head, reverse);
        assert_eq!(app.preview.markdown.layout.as_ref().unwrap().tops, tops);
        app.on_mouse_up(MouseButton::Left);
        settle(&mut app);
        assert_eq!(app.editor.active().unwrap().selection, selection);
        assert_eq!(app.preview.markdown.layout.as_ref().unwrap().tops, tops);
        assert_eq!(
            crate::app::preview::markdown::laid::full_text(&app).unwrap(),
            shown
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn a_selection_crossing_a_paragraph_paints_to_the_margin_only_on_the_first_row() {
    let source = "前文 **编号** 文字\n\n最后一段";
    let (mut app, dir) = live_doc("md-select-row-bands", source);
    let layout = app.preview.markdown.layout.as_ref().unwrap();
    let screen_y = |needle| {
        let (_, y, h) = layout.caret_point(source.find(needle).unwrap()).unwrap();
        app.preview.markdown.area.y + y + h / 2.0 - app.preview.markdown.scroll()
    };
    let first_y = screen_y("文");
    let last_y = screen_y("最后");
    let x = layout.left + layout.width() - 12.0;
    let pixel = |app: &App, y| {
        let image = app.frame.renderer.as_ref().unwrap().image().unwrap();
        let (x, y) = ((x * SCALE) as usize, (y * SCALE) as usize);
        image.pixels[y * image.width as usize + x]
    };
    let before = (pixel(&app, first_y), pixel(&app, last_y));
    let end = source[..source.find("后").unwrap()].chars().count();
    app.select(1, end);
    settle(&mut app);
    assert_ne!(pixel(&app, first_y), before.0);
    assert_eq!(pixel(&app, last_y), before.1);
    let line_end = source[..source.find('\n').unwrap()].chars().count();
    app.select(line_end, end);
    settle(&mut app);
    assert_ne!(pixel(&app, first_y), before.0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_selection_crossing_paragraphs_paints_each_seam_without_a_gap() {
    let source = "First paragraph.\n\nSecond paragraph.\n\nThird paragraph.";
    let (mut app, dir) = live_doc("md-select-paragraph-seams", source);
    app.select(0, source.len());
    settle(&mut app);
    let laid = app.preview.markdown.layout.as_ref().unwrap();
    assert_eq!(laid.tops.len(), 3);
    let origin_y = app.preview.markdown.area.y + laid.pad - app.preview.markdown.scroll();
    let image = app.frame.renderer.as_ref().unwrap().image().unwrap();
    let x = ((laid.left + 4.0) * SCALE) as usize;
    let pixel = |y: usize| image.pixels[y * image.width as usize + x];
    for &top in &laid.tops[1..] {
        let y = ((origin_y + top) * SCALE).round() as usize;
        let above = pixel(y - 1);
        assert!(
            (y..y + 3).all(|y| pixel(y) == above),
            "selection seam at {y}"
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn keyboard_selection_keeps_an_already_open_link_editable() {
    let source = "- [名字](https://a.example)\n";
    let (mut app, dir) = live_doc("md-select-open-link", source);
    put_caret(&mut app, "https", false);
    let shown = crate::app::preview::markdown::laid::full_text(&app).unwrap();
    assert!(shown.contains("https://a.example"), "{shown}");
    app.run(winc_editor::Command::MoveRight {
        word: false,
        extend: true,
    });
    app.ime.cursor = None;
    settle(&mut app);
    let selected = app.editor.active().unwrap().selection;
    assert_eq!(selected.to(), selected.from() + 1);
    assert_eq!(
        crate::app::preview::markdown::laid::full_text(&app).unwrap(),
        shown
    );
    assert!(app.ime.cursor.is_none());
    app.run(winc_editor::Command::Backspace { word: false });
    assert_eq!(doc_text(&app), "- [名字](ttps://a.example)\n");
    app.run(winc_editor::Command::Undo);
    assert_eq!(app.editor.active().unwrap().selection, selected);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn replacing_a_rendered_link_with_ime_removes_the_whole_link() {
    let source = "前文 [名字](https://a.example) 后文\r\n";
    let (mut app, dir) = live_doc("md-replace-covered-link", source);
    let from = "前文 [".chars().count();
    app.select(from + 2, from);
    settle(&mut app);
    let selected = app.editor.active().unwrap().selection;
    app.on_ime(winit::event::Ime::Preedit("替".into(), Some((3, 3))));
    paint(&mut app);
    assert!(app.ime.cursor.is_some());
    app.on_ime(winit::event::Ime::Commit("替换".into()));
    assert_eq!(doc_text(&app), "前文 替换 后文\r\n");
    app.run(winc_editor::Command::Undo);
    assert_eq!(doc_text(&app), source);
    assert_eq!(app.editor.active().unwrap().selection, selected);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn ordered_list_uses_its_display_numbers_in_selection_and_at_the_caret() {
    use winc_typeset::Item;
    let (mut app, dir) = live_doc("md-order-kept", "3) 第一项\n9) 第二项\n");
    let displays_numbers = |app: &App| {
        let laid = app.preview.markdown.layout.as_ref().unwrap();
        let numbers: Vec<_> = laid.blocks[0]
            .1
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Text {
                    text, span: None, ..
                } => Some(text.as_ref()),
                _ => None,
            })
            .collect();
        assert_eq!(numbers, ["3.", "4."]);
    };
    put_caret(&mut app, "第二项", false);
    displays_numbers(&app);
    app.select(3, "3) 第一项\n9) 第二项".chars().count());
    settle(&mut app);
    displays_numbers(&app);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn quote_markers_are_editable_at_the_start_of_a_quote_row() {
    let source = ">> 引用开头\n>> 续行内容\n";
    let (mut app, dir) = live_doc("md-quote-marker", source);
    let shown = |app: &App| crate::app::preview::markdown::laid::full_text(app).unwrap();
    assert!(!shown(&app).contains('>'));
    put_caret(&mut app, "引用开头", false);
    assert!(shown(&app).contains(">> 引用开头"), "{}", shown(&app));
    put_caret(&mut app, "续行内容", false);
    assert!(shown(&app).contains(">> 续行内容"), "{}", shown(&app));
    app.run(winc_editor::Command::MoveLeft {
        word: false,
        extend: false,
    });
    settle(&mut app);
    assert_eq!(
        app.live_caret_source(),
        Some(source.find("续行内容").unwrap() - 1)
    );
    std::fs::remove_dir_all(dir).unwrap();
}

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
fn copying_and_cutting_a_whole_list_item_include_hidden_syntax() {
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
        let selection = app.editor.active().unwrap().selection;
        app.on_mouse_up(MouseButton::Left);
        settle(&mut app);
        assert_eq!(app.editor.active().unwrap().selection, selection);
        assert!(selection.from() > 0, "{reverse} {count}");
        assert_eq!(selection.to(), first.chars().count(), "{reverse} {count}");
        assert_eq!(
            crate::app::preview::markdown::laid::full_text(&app).unwrap(),
            shown
        );
        assert!(app.run_action(crate::keymap::Action::MarkdownCopy));
        assert_eq!(app.clipboard.copied.as_deref(), Some(first.as_str()));
        assert_eq!(app.editor.active().unwrap().selection, selection);
        app.copy(true);
        assert_eq!(app.clipboard.copied.as_deref(), Some(first.as_str()));
        assert_eq!(doc_text(&app), "\n- 未选择\n");
        app.run(winc_editor::Command::Undo);
        assert_eq!(doc_text(&app), source);
        assert_eq!(app.editor.active().unwrap().selection, selection);
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
    let start = "- 前一项\n- ".chars().count();
    let expected = start..start + item.chars().count();
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
