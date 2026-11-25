//! 可编辑 Markdown 的鼠标选区：平台连击、整段拖选和后台解析期间的行为。

use super::*;

#[test]
fn cross_block_drag_keeps_the_same_layout_and_endpoints_on_release() {
    let source = "### 标题\n\n- [第一项](https://a.example)\n- [第二项](https://b.example)\n";
    for reverse in [false, true] {
        let (mut app, dir) = live_doc("md-select-across-heading", source);
        let shown = app.preview.markdown.full_text().unwrap();
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
        let tops = app
            .preview
            .markdown
            .layout
            .as_ref()
            .unwrap()
            .tops()
            .to_vec();
        app.on_mouse_move(from.0, from.1);
        app.on_mouse_down_with_click_count(MouseButton::Left, Some(1));
        app.on_mouse_move(to.0, to.1);
        paint(&mut app);
        let selection = app.editor.active().unwrap().selection;
        assert!(!selection.is_empty());
        assert_eq!(selection.anchor > selection.head, reverse);
        assert_eq!(app.preview.markdown.layout.as_ref().unwrap().tops(), tops);
        app.on_mouse_up(MouseButton::Left);
        settle(&mut app);
        assert_eq!(app.editor.active().unwrap().selection, selection);
        assert_eq!(app.preview.markdown.layout.as_ref().unwrap().tops(), tops);
        assert_eq!(app.preview.markdown.full_text().unwrap(), shown);
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
    let x = layout.column_bounds().0 + layout.width() - 12.0;
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
    assert_eq!(laid.tops().len(), 3);
    let origin_y = laid.origin_y(app.preview.markdown.area.y, app.preview.markdown.scroll());
    let image = app.frame.renderer.as_ref().unwrap().image().unwrap();
    let x = ((laid.column_bounds().0 + 4.0) * SCALE) as usize;
    let pixel = |y: usize| image.pixels[y * image.width as usize + x];
    for &top in &laid.tops()[1..] {
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
    let shown = app.preview.markdown.full_text().unwrap();
    assert!(shown.contains("https://a.example"), "{shown}");
    app.run(winc_editor::Command::MoveRight {
        word: false,
        extend: true,
    });
    app.ime.cursor = None;
    settle(&mut app);
    let selected = app.editor.active().unwrap().selection;
    assert_eq!(selected.to(), selected.from() + 1);
    assert_eq!(app.preview.markdown.full_text().unwrap(), shown);
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
        let numbers: Vec<_> = laid
            .block_at(0)
            .unwrap()
            .items()
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
    let shown = |app: &App| app.preview.markdown.full_text().unwrap();
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
    let shown = app.preview.markdown.full_text().unwrap();
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
        let shown = app.preview.markdown.full_text().unwrap();
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
        assert_eq!(app.preview.markdown.full_text().unwrap(), shown);
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
    let shown = app.preview.markdown.full_text().unwrap();
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
    let shown = app.preview.markdown.full_text().unwrap();
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
        let shown = app.preview.markdown.full_text().unwrap();
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
    let shown = app.preview.markdown.full_text().unwrap();
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
    let shown = app.preview.markdown.full_text().unwrap();
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

#[test]
fn shift_arrows_cross_a_separator_and_ime_replaces_only_the_selection() {
    use winc_core::Selection;
    use winc_editor::Command;
    use winit::{
        event::Ime,
        keyboard::{Key, ModifiersState, NamedKey},
    };
    let source = "# 标题\n\n前段👩‍💻\n\n后段e\u{301}\n";
    let at = |text| source[..source.find(text).unwrap()].chars().count();
    let (a, b) = (at("前段"), at("后段"));
    for reverse in [false, true] {
        let (mut app, dir) = live_doc(&format!("md-extend-ime-{reverse}"), source);
        let (from, expected, key) = if reverse {
            ("后段", Selection::new(b, a), NamedKey::ArrowUp)
        } else {
            ("前段", Selection::new(a, b), NamedKey::ArrowDown)
        };
        put_caret(&mut app, from, false);
        app.pointer.modifiers = ModifiersState::SHIFT;
        assert!(app.markdown_key(&Key::Named(key), false));
        paint(&mut app);
        app.pointer.modifiers = ModifiersState::empty();
        assert_eq!(app.editor.active().unwrap().selection, expected);
        app.on_ime(Ime::Preedit("ni".into(), Some((2, 2))));
        paint(&mut app);
        assert_eq!(doc_text(&app), source);
        assert_eq!(app.editor.active().unwrap().selection, expected);
        app.on_ime(Ime::Commit("你".into()));
        assert_eq!(doc_text(&app), "# 标题\n\n你后段e\u{301}\n");
        app.run(Command::Undo);
        assert_eq!(doc_text(&app), source);
        assert_eq!(app.editor.active().unwrap().selection, expected);
        // Replay multiple commits without allowing a frame or background parse between them.
        for text in ["你好", "👩‍💻", "e\u{301}"] {
            app.on_ime(Ime::Commit(text.into()));
        }
        settle(&mut app);
        assert_eq!(doc_text(&app), "# 标题\n\n你好👩‍💻e\u{301}后段e\u{301}\n");
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn shift_down_selects_a_wrapped_row_for_paste_and_undo() {
    use winc_core::Selection;
    use winc_editor::Command;
    use winit::keyboard::{Key, ModifiersState, NamedKey};
    let first = "甲乙丙丁".repeat(40);
    let source = format!("{first}\n\n下一段");
    let (mut app, dir) = live_doc("md-select-wrapped-paste", &source);
    let y = caret_screen_y(&app);
    app.pointer.modifiers = ModifiersState::SHIFT;
    assert!(app.markdown_key(&Key::Named(NamedKey::ArrowDown), false));
    app.pointer.modifiers = ModifiersState::empty();
    settle(&mut app);
    let selected = app.editor.active().unwrap().selection;
    assert_eq!(selected.anchor, 0);
    assert!((1..first.chars().count()).contains(&selected.head));
    assert!(caret_screen_y(&app) > y);
    let rest = source.char_indices().nth(selected.head).unwrap().0;
    app.clipboard.copied = Some("替换".into());
    app.paste();
    assert_eq!(doc_text(&app), format!("替换{}", &source[rest..]));
    let doc = app.editor.active().unwrap();
    assert_eq!(doc.selection, Selection::point(2));
    app.run(Command::Undo);
    assert_eq!(doc_text(&app), source);
    assert_eq!(app.editor.active().unwrap().selection, selected);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn replacing_across_blocks_keeps_the_unselected_emphasis_balanced() {
    use winc_core::Selection;
    use winc_editor::Command;
    use winit::event::Ime;
    let source = "开头 **保留 删除**\r\n\r\n选中部分 后文";
    let (mut app, dir) = live_doc("md-replace-bold-boundary", source);
    let at = |word| source[..source.find(word).unwrap()].chars().count();
    let selection = Selection::new(at(" 后文"), at("删除"));
    app.editor.active_mut().unwrap().selection = selection;
    settle(&mut app);
    app.on_ime(Ime::Commit("换掉".into()));
    assert_eq!(with_caret(&app), "开头 **保留** 换掉| 后文");
    app.run(Command::Undo);
    assert_eq!(doc_text(&app), source);
    assert_eq!(app.editor.active().unwrap().selection, selection);
    app.run(Command::Redo);
    assert_eq!(with_caret(&app), "开头 **保留** 换掉| 后文");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deleting_hidden_multiline_emphasis_removes_only_fully_selected_delimiters() {
    use winc_core::Selection;
    use winc_editor::Command;

    let source = "前 **甲\r\n乙** 后";
    let (mut app, dir) = live_doc("md-delete-multiline-bold", source);
    let at = |word| source[..source.find(word).unwrap()].chars().count();

    let selection = Selection::new(at("** 后"), at("甲"));
    app.editor.active_mut().unwrap().selection = selection;
    settle(&mut app);
    app.run(Command::Delete { word: false });
    assert_eq!(doc_text(&app), "前  后");
    app.run(Command::Undo);
    assert_eq!(doc_text(&app), source);
    assert_eq!(app.editor.active().unwrap().selection, selection);

    app.editor.active_mut().unwrap().selection = Selection::new(at("甲"), at("乙"));
    settle(&mut app);
    app.run(Command::Delete { word: false });
    assert_eq!(doc_text(&app), "前 **乙** 后");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn copy_in_a_note_offers_text_and_html_or_explicit_markdown() {
    use crate::keymap::Action;
    use winc_core::Selection;
    let source = "A **重点** 和 [资料](https://example.com)。\n\n```\n**代码原样**\n```\n";
    let (mut app, dir) = live_doc("md-clipboard-copy", source);
    let select = |app: &mut App, needle: &str| {
        let doc = app.editor.active_mut().unwrap();
        let rope = doc.text().rope().unwrap();
        let start = source.find(needle).unwrap();
        doc.selection = Selection::new(
            rope.byte_to_char(start),
            rope.byte_to_char(start + needle.len()),
        );
    };
    select(&mut app, "重点");
    let selection = app.editor.active().unwrap().selection;
    assert!(app.run_action(Action::Copy));
    assert_eq!(app.clipboard.copied.as_deref(), Some("重点"));
    assert!(
        app.clipboard
            .copied_html
            .as_deref()
            .unwrap()
            .contains("<strong>重点</strong>")
    );
    assert!(app.run_action(Action::MarkdownCopy));
    assert_eq!(app.clipboard.copied.as_deref(), Some("**重点**"));
    assert!(app.clipboard.copied_html.is_none());
    assert_eq!(app.editor.active().unwrap().selection, selection);
    assert_eq!(doc_text(&app), source);
    select(&mut app, "点");
    assert!(app.run_action(Action::Copy));
    assert_eq!(app.clipboard.copied.as_deref(), Some("点"));
    assert_eq!(
        app.clipboard.copied_html.as_deref(),
        Some("<strong>点</strong>")
    );
    select(&mut app, "资料");
    assert!(app.run_action(Action::MarkdownCopy));
    assert_eq!(
        app.clipboard.copied.as_deref(),
        Some("[资料](https://example.com)")
    );
    select(&mut app, "**代码原样**");
    assert!(app.run_action(Action::Copy));
    assert_eq!(app.clipboard.copied.as_deref(), Some("**代码原样**"));
    assert!(app.clipboard.copied_html.is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn copying_across_blocks_keeps_only_the_visible_selection() {
    use crate::keymap::Action;
    use winc_core::Selection;
    let source = "前 **甲乙**\n\n## 丙丁\n\n后文";
    let char_at = |word| source[..source.find(word).unwrap()].chars().count();
    let (a, b) = (char_at('乙'), char_at('丁'));
    for reverse in [false, true] {
        let (mut app, dir) = live_doc(&format!("md-copy-across-{reverse}"), source);
        let shown = app.preview.markdown.full_text().unwrap();
        let point = |ch| point_of(&app, shown[..shown.find(ch).unwrap()].chars().count());
        let (from, to) = (point('乙'), point('丁'));
        let ((x, y), (x2, y2), expected) = if reverse {
            (to, from, Selection::new(b, a))
        } else {
            (from, to, Selection::new(a, b))
        };
        app.on_mouse_move(x, y);
        app.on_mouse_down(MouseButton::Left);
        paint(&mut app);
        app.on_mouse_move(x2, y2);
        app.on_mouse_up(MouseButton::Left);
        settle(&mut app);
        assert_eq!(app.editor.active().unwrap().selection, expected);
        assert!(app.run_action(Action::Copy));
        assert_eq!(app.clipboard.copied.as_deref(), Some("乙\n丙"));
        let html = app.clipboard.copied_html.as_deref().unwrap();
        assert!(
            html.contains("<strong>乙</strong>") && html.contains("<h2>丙</h2>"),
            "{html}"
        );
        assert!(!html.contains(['甲', '丁']));
        assert_eq!(doc_text(&app), source);
        assert_eq!(app.editor.active().unwrap().selection, expected);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn dragging_across_blocks_keeps_the_layout_still() {
    let (mut app, dir) = live_doc("md-drag", "第一段文字\n\n第二段文字\n\n第三段文字\n");
    let shown = app.preview.markdown.full_text().unwrap();
    let at = |s: &str| shown[..shown.find(s).unwrap()].chars().count();
    let (x, y) = point_of(&app, at("第一段") + 1);
    let (x2, y2) = point_of(&app, at("第三段") + 1);
    app.on_mouse_move(x, y);
    app.on_mouse_down(MouseButton::Left);
    paint(&mut app);
    // 按住鼠标拖动期间排版不变（尚未开始编辑，全部按排版显示），鼠标下方的内容不移位。
    assert!(!app.preview.markdown.layout.as_ref().unwrap().has_live());
    app.on_mouse_move(x2, y2 + 1.0);
    paint(&mut app);
    assert!(!app.preview.markdown.layout.as_ref().unwrap().has_live());
    assert_eq!(app.preview.markdown.full_text().unwrap(), shown);
    app.on_mouse_up(MouseButton::Left);
    paint(&mut app);
    let doc = app.editor.active().unwrap();
    let text = doc_text(&app);
    let rope = doc.text().rope().unwrap();
    let selected =
        &text[rope.char_to_byte(doc.selection.from())..rope.char_to_byte(doc.selection.to())];
    assert!(
        selected.starts_with("一段") && selected.contains("第二段文字"),
        "{selected}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn backward_drag_over_multiline_emphasis_deletes_hidden_markers_and_restores_selection_on_undo() {
    use winc_core::Selection;
    use winc_editor::Command;

    let source = "前 **甲\r\n乙** 后";
    let (mut app, dir) = live_doc("md-drag-multiline-bold", source);
    let preview = &app.preview.markdown;
    let laid = preview.layout.as_ref().unwrap();
    // Pick a point in each target glyph on the appropriate side of its midpoint.
    let glyph_edge = |needle, fraction| {
        let (top, _, x, baseline, size, span, _, text) = laid
            .spans(0..1)
            .find(|&(.., text)| text.contains(needle))
            .expect("the selected text is visible in the rendered paragraph");
        let glyph = text.chars().position(|c| c == needle).unwrap();
        let leading: f32 = span.advances[..glyph].iter().sum();
        (
            laid.column_bounds().0 + x + leading + fraction * span.advances[glyph],
            laid.origin_y(preview.area.y, preview.scroll()) + top + baseline - size * 0.3,
        )
    };
    let from = glyph_edge('乙', 0.75);
    let to = glyph_edge('甲', 0.25);
    app.on_mouse_move(from.0, from.1);
    app.on_mouse_down_with_click_count(MouseButton::Left, Some(1));
    app.on_mouse_move(to.0, to.1);
    app.on_mouse_up(MouseButton::Left);
    settle(&mut app);

    let at = |needle| source[..source.find(needle).unwrap()].chars().count();
    let expected = Selection::new(at("** 后"), at("甲"));
    assert_eq!(app.editor.active().unwrap().selection, expected);
    app.run(Command::Delete { word: false });
    assert_eq!(doc_text(&app), "前  后");
    app.run(Command::Undo);
    assert_eq!(doc_text(&app), source);
    assert_eq!(app.editor.active().unwrap().selection, expected);
    std::fs::remove_dir_all(dir).unwrap();
}
