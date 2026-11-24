//! 光标、空行和可编辑位置。

use super::*;

#[test]
fn the_caret_paragraph_keeps_its_look() {
    use crate::app::preview::markdown::laid::full_text;
    use winit::keyboard::{Key, NamedKey};

    let (mut app, dir) = live_doc(
        "md-reveal",
        "第一段 **粗** 和 [链接](https://a.dev)。\n\n第二段\n",
    );
    let laid = full_text(&app).unwrap();
    // 光标未接触标记时保持排版后的外观，不显示原文。
    put_caret(&mut app, "第一段", true);
    assert_eq!(full_text(&app).unwrap(), laid);
    // 只显示光标所接触元素的标记。
    put_caret(&mut app, "粗", false);
    let shown = full_text(&app).unwrap();
    assert!(
        shown.contains("**粗**") && !shown.contains("](https"),
        "{shown}"
    );
    put_caret(&mut app, "链接", true);
    let shown = full_text(&app).unwrap();
    assert!(
        shown.contains("[链接](https://a.dev)") && !shown.contains("**"),
        "{shown}"
    );
    // 段落末尾回车：光标落在下方单独的一行上。
    put_caret(&mut app, "第二段", true);
    let line = caret_screen_y(&app);
    assert!(app.markdown_key(&Key::Named(NamedKey::Enter), false));
    settle(&mut app);
    assert!(caret_screen_y(&app) > line + 10.0);
    // 输入法组字排入光标所在的行，其后的文字向后移动，不被组字覆盖。
    put_caret(&mut app, "粗", true);
    app.ime.preedit = "中文".into();
    paint(&mut app);
    let shown = full_text(&app).unwrap();
    assert!(shown.contains("粗中文** 和"), "{shown}");
    // 焦点移到终端后组字文字属于终端：预览不再显示，也不绘制光标、不占用候选窗的位置。
    app.focus = Focus::Terminal;
    app.ime.cursor = None;
    paint(&mut app);
    let shown = full_text(&app).unwrap();
    assert!(!shown.contains("中文"), "{shown}");
    assert!(app.ime.cursor.is_none());
    app.focus = Focus::Editor;
    paint(&mut app);
    assert!(full_text(&app).unwrap().contains("粗中文** 和"));
    assert!(app.ime.cursor.is_some());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn repeated_enter_moves_the_visible_caret_down_one_row_at_a_time() {
    use winit::keyboard::{Key, NamedKey};
    for (name, source) in [
        ("between", "第一段\n\n第二段\n"),
        ("end", "第一段"),
        ("empty", ""),
    ] {
        let (mut app, dir) = live_doc(&format!("md-multiple-enter-{name}"), source);
        if !source.is_empty() {
            put_caret(&mut app, "第一段", true);
        }
        let y = |app: &App| {
            app.preview
                .markdown
                .layout
                .as_ref()
                .unwrap()
                .caret_point(app.live_caret_source().unwrap())
                .map_or(
                    app.preview.markdown.layout.as_ref().unwrap().pad,
                    |(_, y, _)| y,
                )
        };
        let mut above = y(&app);
        for n in 1..=3 {
            assert!(app.markdown_key(&Key::Named(NamedKey::Enter), false));
            settle(&mut app);
            assert!(
                y(&app) > above + 5.0,
                "{name}, Enter {n}: {} <= {above}",
                y(&app)
            );
            above = y(&app);
        }
        let before = doc_text(&app);
        let backspace = Key::Named(NamedKey::Backspace);
        if !app.markdown_key(&backspace, false) {
            app.run(winc_editor::Command::Backspace { word: false });
        }
        settle(&mut app);
        assert_eq!(doc_text(&app).len() + 1, before.len());
        assert!(y(&app) < above - 5.0);
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn intentional_blank_rows_stay_visible_after_leaving_the_gap_and_reopening() {
    use winit::keyboard::{Key, NamedKey};
    let (mut app, dir) = live_doc("md-keep-blank-rows", "第一段\n\n第二段\n");
    let second = |app: &App| app.preview.markdown.layout.as_ref().unwrap().tops[1];
    let original = second(&app);
    put_caret(&mut app, "第一段", true);
    for _ in 0..3 {
        assert!(app.markdown_key(&Key::Named(NamedKey::Enter), false));
        settle(&mut app);
    }
    let expanded = second(&app);
    put_caret(&mut app, "第二段", true);
    let still_expanded = second(&app);
    assert!(
        still_expanded > original + 30.0,
        "extra blank lines disappeared on leaving the gap"
    );
    assert!(
        (expanded - still_expanded).abs() < 1.0,
        "the following paragraph moved when the caret left"
    );
    let id = app.editor.active().unwrap().id;
    assert!(app.save_document(id, None));
    let path = dir.join("LIVE.md");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "第一段\n\n\n\n\n\n第二段\n"
    );
    let mut reopened = app_with(&path, false);
    paint(&mut reopened);
    reopened.set_preview_mode(PreviewMode::Preview);
    settle(&mut reopened);
    assert!((second(&reopened) - still_expanded).abs() < 1.0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn block_separators_are_spacing_in_the_editable_preview() {
    use crate::keymap::Action;
    use winit::keyboard::{Key, ModifiersState, NamedKey};

    for (name, source, first, second, eol) in [
        (
            "paragraph-list",
            "技巧类\n\n- 第一条\n",
            "技巧类",
            "第一条",
            "\n",
        ),
        ("paragraphs", "甲\r\n \t\r\n乙", "甲", "乙", "\r\n"),
    ] {
        let (mut app, dir) = live_doc(&format!("md-block-separator-{name}"), source);
        let first_end = source.find(first).unwrap() + first.len();
        let second_start = source.find(second).unwrap();
        let blank = first_end + eol.len();
        let (x, y) = {
            let laid = app.preview.markdown.layout.as_ref().unwrap();
            let gap_top = laid.tops[0] + laid.blocks[0].1.height;
            let gap_bottom = laid.tops[1];
            (
                laid.left + 15.0,
                app.preview.markdown.area.y + laid.pad - app.preview.markdown.scroll()
                    + (gap_top + gap_bottom) / 2.0,
            )
        };
        super::click(&mut app, (x, y));
        settle(&mut app);
        let clicked = app.live_caret_source().unwrap();
        assert!(
            clicked < blank || clicked >= second_start,
            "{name}: {clicked}"
        );

        put_caret(&mut app, first, true);
        assert!(app.markdown_key(&Key::Named(NamedKey::ArrowRight), false));
        assert_eq!(app.live_caret_source(), Some(second_start));
        settle(&mut app);
        assert!(app.markdown_key(&Key::Named(NamedKey::ArrowLeft), false));
        assert_eq!(app.live_caret_source(), Some(first_end));

        put_caret(&mut app, first, false);
        assert!(app.markdown_key(&Key::Named(NamedKey::ArrowDown), false));
        assert!(app.live_caret_source().unwrap() >= second_start, "{name}");
        settle(&mut app);
        assert!(app.markdown_key(&Key::Named(NamedKey::ArrowUp), false));
        assert!(app.live_caret_source().unwrap() <= first_end, "{name}");

        put_caret(&mut app, first, true);
        app.pointer.modifiers = ModifiersState::SHIFT;
        assert!(app.markdown_key(&Key::Named(NamedKey::ArrowRight), false));
        app.pointer.modifiers = ModifiersState::empty();
        assert!(app.run_action(Action::MarkdownCopy));
        assert_eq!(
            app.clipboard.copied.as_deref(),
            Some(&source[first_end..second_start])
        );
        assert_eq!(doc_text(&app), source);
        let id = app.editor.active().unwrap().id;
        assert!(app.save_document(id, None));
        assert_eq!(
            std::fs::read_to_string(dir.join("LIVE.md")).unwrap(),
            source
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn heading_separators_are_clickable_empty_lines() {
    use winit::keyboard::{Key, NamedKey};

    for marker in ["#", "##"] {
        let source = format!("{marker} 技巧类\n\n- 第一条\n");
        let blank = source.find("\n\n").unwrap() + 1;
        let (mut app, dir) = live_doc("md-heading-separator", &source);
        let (x, y) = {
            let laid = app.preview.markdown.layout.as_ref().unwrap();
            let top = laid.tops[0] + laid.blocks[0].1.height;
            (
                laid.left + 10.0,
                app.preview.markdown.area.y + laid.pad - app.preview.markdown.scroll()
                    + (top + laid.tops[1]) / 2.0,
            )
        };
        super::click(&mut app, (x, y));
        assert_eq!(app.live_caret_source(), Some(blank), "{marker}");
        put_caret(&mut app, "技巧类", false);
        assert!(app.markdown_key(&Key::Named(NamedKey::ArrowDown), false));
        assert_eq!(app.live_caret_source(), Some(blank), "{marker}");
        assert_eq!(doc_text(&app), source);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn a_click_below_the_body_opens_one_empty_paragraph() {
    use winc_editor::Command;

    for (name, source, extra) in [
        ("lf", "正文", "\n\n"),
        ("crlf-list", "# 标题\r\n\r\n- 最后一项\r\n", "\r\n"),
    ] {
        let (mut app, dir) = live_doc(&format!("md-click-below-{name}"), source);
        let below = |app: &App| {
            let laid = app.preview.markdown.layout.as_ref().unwrap();
            (
                laid.left + 10.0,
                app.preview.markdown.area.y + laid.height() - app.preview.markdown.scroll() + 20.0,
            )
        };
        let (x, y) = below(&app);
        app.on_mouse_move(x, y);
        app.on_mouse_down_with_click_count(MouseButton::Left, Some(1));
        assert_eq!(doc_text(&app), source);
        app.on_mouse_up(MouseButton::Left);
        settle(&mut app);
        let expected = format!("{source}{extra}");
        assert_eq!(doc_text(&app), expected, "{name}");
        assert_eq!(app.live_caret_source(), Some(expected.len()));
        let at = below(&app);
        super::click(&mut app, at);
        assert_eq!(doc_text(&app), expected, "{name}");
        app.run(Command::Undo);
        assert_eq!(doc_text(&app), source, "{name}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn a_blank_paragraph_at_the_end_has_a_body_height_caret() {
    let (mut app, dir) = live_doc("md-caret-height-at-end", "最后一段。\n");
    put_caret(&mut app, "最后一段。", true);
    let body_caret = app.ime.cursor.unwrap();
    let area = app.preview.markdown.area;
    let layout = app.preview.markdown.layout.as_ref().unwrap();
    let x = layout.left + 10.0;
    let y = area.y + layout.height() - app.preview.markdown.scroll() + 20.0;
    super::click(&mut app, (x, y));
    settle(&mut app);
    assert_eq!(with_caret(&app), "最后一段。\n\n|");
    let blank_caret = app.ime.cursor.unwrap();
    let _ = std::fs::remove_dir_all(dir);
    assert_eq!(blank_caret.h, body_caret.h);
}

#[test]
fn a_separator_after_properties_places_the_caret_in_the_body() {
    use winit::keyboard::{Key, NamedKey};

    let source = "---\nstatus: draft\n---\n\n## 标题\n";
    let (mut app, dir) = live_doc("md-property-separator", source);
    let body = source.find("##").unwrap();
    let (x, y) = {
        let laid = app.preview.markdown.layout.as_ref().unwrap();
        let gap_top = laid.tops[0] + laid.blocks[0].1.height;
        let gap_bottom = laid.tops[1];
        assert!(gap_bottom > gap_top);
        (
            laid.left + 15.0,
            app.preview.markdown.area.y + laid.pad - app.preview.markdown.scroll()
                + gap_top
                + (gap_bottom - gap_top) * 0.25,
        )
    };
    super::click(&mut app, (x, y));
    settle(&mut app);
    assert!(app.live_caret_source().unwrap() >= body);
    put_caret(&mut app, "标题", false);
    assert!(app.markdown_key(&Key::Named(NamedKey::ArrowUp), false));
    assert!(app.live_caret_source().unwrap() >= body);
    assert_eq!(doc_text(&app), source);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn click_and_vertical_arrows_reach_each_blank_row() {
    use winit::keyboard::{Key, NamedKey};
    for (name, eol) in [("lf", "\n"), ("crlf", "\r\n")] {
        let source = format!("甲{}乙", eol.repeat(4));
        let (mut app, dir) = live_doc(&format!("md-click-blank-{name}"), &source);
        let gap = |app: &App| {
            let laid = app.preview.markdown.layout.as_ref().unwrap();
            (laid.tops[0] + laid.blocks[0].1.height, laid.tops[1])
        };
        let (top, bottom) = gap(&app);
        let first = "甲".len();
        for row in 0..3 {
            put_caret(&mut app, "乙", false);
            let laid = app.preview.markdown.layout.as_ref().unwrap();
            let x = laid.left + 45.0;
            let y = app.preview.markdown.area.y + laid.pad - app.preview.markdown.scroll()
                + top
                + (bottom - top) * (row as f32 + 0.5) / 3.0;
            super::click(&mut app, (x, y));
            settle(&mut app);
            let expected = first + eol.len() * (row + 1);
            assert_eq!(app.live_caret_source(), Some(expected), "{name} row {row}");
            let laid = app.preview.markdown.layout.as_ref().unwrap();
            assert!(laid.caret_point(expected).is_some());
            assert!((gap(&app).1 - bottom).abs() < 1.0, "body jumped");
        }
        put_caret(&mut app, "甲", true);
        for pos in (1..=3)
            .map(|row| first + row * eol.len())
            .chain([source.len()])
        {
            assert!(app.markdown_key(&Key::Named(NamedKey::ArrowDown), false));
            settle(&mut app);
            assert_eq!(app.live_caret_source(), Some(pos), "{name} Down");
        }
        for pos in (1..=3).rev().map(|row| first + row * eol.len()) {
            assert!(app.markdown_key(&Key::Named(NamedKey::ArrowUp), false));
            settle(&mut app);
            assert_eq!(app.live_caret_source(), Some(pos), "{name} Up");
        }
        assert_eq!(doc_text(&app), source);
        app.run(winc_editor::Command::Insert("新".into()));
        let mut expected = source.clone();
        expected.insert(first + eol.len(), '新');
        assert_eq!(doc_text(&app), expected);
        app.run(winc_editor::Command::Undo);
        assert_eq!(doc_text(&app), source);
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn blank_rows_at_document_edges_and_after_properties_are_clickable() {
    const HEADER: &str = "---\nstatus: draft\n---\n";
    for (name, source, offsets) in [
        ("leading", "\n\n\n# 乙".to_owned(), [0, 1, 2]),
        ("trailing", "甲\r\n\r\n\r\n".to_owned(), [5, 7, 9]),
        (
            "properties",
            format!("{HEADER}\n \n\n# 乙"),
            [HEADER.len(), HEADER.len() + 1, HEADER.len() + 3],
        ),
    ] {
        let (mut app, dir) = live_doc(&format!("md-blank-{name}"), &source);
        let mut previous_y = 0.0;
        for offset in offsets {
            let laid = app.preview.markdown.layout.as_ref().unwrap();
            let (_, top, h) = laid
                .caret_point(offset)
                .expect("each blank row has a caret box");
            let y = app.preview.markdown.area.y - app.preview.markdown.scroll() + top + h / 2.0;
            assert!(y > previous_y);
            previous_y = y;
            let x = laid.left + 32.0;
            super::click(&mut app, (x, y));
            settle(&mut app);
            assert_eq!(app.live_caret_source(), Some(offset), "{name}");
        }
        assert_eq!(doc_text(&app), source);
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn the_live_preview_hides_its_caret_until_clicked_and_ignores_the_margins() {
    let (mut app, dir) = readme("md-live-caret", false, PreviewMode::Preview);
    paint(&mut app);
    // 刚打开、仅阅读时：不显示光标。
    assert!(!app.live_caret_shown());
    let before = app.editor.active().unwrap().selection;

    // 点在内容一栏左侧的空白：不放置光标、不开始选择。
    let area = app.preview.markdown.area;
    super::click(&mut app, (area.x + 2.0, area.y + area.h / 2.0));
    paint(&mut app);
    assert!(!app.live_caret_shown());
    assert_eq!(app.editor.active().unwrap().selection, before);

    // 点在文字上：显示光标。
    let at = point_of(&app, 2);
    super::click(&mut app, at);
    paint(&mut app);
    assert!(app.live_caret_shown());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn end_stays_on_the_same_visual_line_after_looking_up_both_sides_of_a_wrap() {
    use winit::keyboard::{Key, NamedKey};

    let source = "甲乙丙丁".repeat(60);
    let (mut app, dir) = live_doc("md-caret-wrap-affinity", &source);
    let first_y = caret_screen_y(&app);
    let end = Key::Named(NamedKey::End);
    assert!(app.markdown_key(&end, false));
    paint(&mut app);
    let at = app.live_caret_source().unwrap();
    assert!((1..source.len()).contains(&at));
    let laid = app.preview.markdown.layout.as_ref().unwrap();
    let above = laid.caret_point_with_affinity(at, true).unwrap();
    let below = laid.caret_point_with_affinity(at, false).unwrap();
    assert!(above.1 < below.1);
    assert_eq!(laid.caret_point(at), Some(above));
    assert!((caret_screen_y(&app) - first_y).abs() < 0.5);
    assert!(app.markdown_key(&end, false));
    assert_eq!(app.live_caret_source(), Some(at));
    std::fs::remove_dir_all(dir).unwrap();
}

/// 回归：单独成行的公式中没有可定位的文字，点击后光标仍应进入公式的原文，公式随之显示原文、
/// 可以编辑（`$$…$$` 和语言为 math 的围栏代码块都是如此）。
#[test]
fn clicking_a_block_formula_puts_the_caret_in_its_source() {
    for (name, formula) in [
        ("md-math-dollars", "$$\nx^2 + y^2 = z^2\n$$"),
        ("md-math-fence", "```math\nx^2 + y^2 = z^2\n```"),
    ] {
        let text = format!("前文\n\n{formula}\n\n后文\n");
        let (mut app, dir) = live_doc(name, &text);
        let shown = crate::app::preview::markdown::laid::full_text(&app).unwrap();
        let at = |c: &str| shown[..shown.find(c).unwrap()].chars().count();
        let (above, below) = (point_of(&app, at("前文")), point_of(&app, at("后文")));
        // 公式在两段之间居中显示：从上往下找第一处落在公式上的点（正文的字不会落到公式的原文处）。
        let area = app.preview.markdown.area;
        let start = text.find(formula).unwrap();
        let x = area.x + area.w / 2.0;
        let middle = (above.1 as usize..below.1 as usize)
            .map(|y| (x, y as f32))
            .find(|&(x, y)| {
                crate::app::preview::markdown::laid::source_at(&app, x, y) == Some(start)
            })
            .expect("点得到公式");
        app.on_mouse_move(middle.0, middle.1);
        app.on_mouse_down(MouseButton::Left);
        app.on_mouse_up(MouseButton::Left);
        settle(&mut app);
        let caret = app.live_caret_source().unwrap();
        assert!(
            (start..start + formula.len()).contains(&caret),
            "{name}: {}",
            with_caret(&app)
        );
        // 光标在公式里：公式显示原文，可以直接编辑。
        let shown = crate::app::preview::markdown::laid::full_text(&app).unwrap();
        assert!(shown.contains("x^2 + y^2 = z^2"), "{name}: {shown}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// 回归：行内公式（如希腊字母、运算符）中没有可定位的字，点击公式时光标仍应进入公式的原文。
#[test]
fn clicking_inline_math_puts_the_caret_in_its_source() {
    let text = "前文\n\n$\\alpha\\beta\\gamma \\nabla\\partial\\infty$\n\n后文\n";
    let (mut app, dir) = live_doc("md-inline-math", text);
    let shown = crate::app::preview::markdown::laid::full_text(&app).unwrap();
    let at = |c: &str| shown[..shown.find(c).unwrap()].chars().count();
    let (above, below) = (point_of(&app, at("前文")), point_of(&app, at("后文")));
    let tex = text.find("\\alpha").unwrap();
    let area = app.preview.markdown.area;
    // 从左到右、从上到下找第一处落在公式原文上的点（正文的字不会落到公式的原文处）。
    let spot = (above.1 as usize..below.1 as usize)
        .flat_map(|y| {
            (area.x as usize..area.right() as usize)
                .step_by(4)
                .map(move |x| (x, y))
        })
        .map(|(x, y)| (x as f32, y as f32))
        .find(|&(x, y)| crate::app::preview::markdown::laid::source_at(&app, x, y) == Some(tex))
        .expect("点得到公式");
    app.on_mouse_move(spot.0, spot.1);
    app.on_mouse_down(MouseButton::Left);
    app.on_mouse_up(MouseButton::Left);
    settle(&mut app);
    let caret = app.live_caret_source().unwrap();
    let end = text.find("$\n\n后文").unwrap();
    assert!((tex..=end).contains(&caret), "{}", with_caret(&app));
    let _ = std::fs::remove_dir_all(&dir);
}

/// 点击分隔线：光标移到它的原文处，分隔线显示原文；列表里的分隔线同样可以点击。
#[test]
fn clicking_a_rule_puts_the_caret_on_it() {
    for (text, expect) in [
        ("前文\n\n---\n\n后文\n", "前文\n\n|---\n\n后文\n"),
        (
            "前文\n\n1. 一\n2. ***\n\n后文\n",
            "前文\n\n1. 一\n2. |***\n\n后文\n",
        ),
    ] {
        let (mut app, dir) = live_doc("md-rule-click", text);
        put_caret(&mut app, "前文", true);
        let area = app.preview.markdown.area;
        let middle = |app: &App, s: &str| {
            let laid = app.preview.markdown.layout.as_ref().unwrap();
            let at = doc_text(app).find(s).unwrap();
            let (_, y, h) = laid.caret_point(at).unwrap();
            area.y + y - app.preview.markdown.scroll() + h / 2.0
        };
        // 分隔线在上一行文字与「后文」之间，取两者正中。
        let above = if text.contains("一") {
            "一"
        } else {
            "前文"
        };
        let y = (middle(&app, above) + middle(&app, "后文")) / 2.0;
        super::click(&mut app, (area.x + area.w / 2.0, y));
        settle(&mut app);
        assert_eq!(with_caret(&app), expect);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn source_lookup_finds_footnotes_in_display_order() {
    let source = "ref[^a]\n\n[^a]: note\n\nnext\n\nlast\n";
    let (app, dir) = live_doc("md-focus-footnote-source", source);
    let laid = app.preview.markdown.layout.as_ref().unwrap();
    let footnote = laid.block_at_source(source.find("note").unwrap());
    assert_eq!(footnote, Some(laid.blocks.len() - 1));
    assert_eq!(laid.block_at_source(source.find("next").unwrap()), Some(1));
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn horizontal_arrows_stop_on_each_blank_line() {
    use winc_editor::Command;
    use winit::keyboard::{Key, NamedKey};

    let (mut app, dir) = live_doc("md-gap", "甲\r\n\r\n\r\n乙\n");
    put_caret(&mut app, "甲", true);
    for (key, cmd, carets) in [
        (
            NamedKey::ArrowRight,
            Command::MoveRight {
                word: false,
                extend: false,
            },
            [5, 7, 9],
        ),
        (
            NamedKey::ArrowLeft,
            Command::MoveLeft {
                word: false,
                extend: false,
            },
            [7, 5, 3],
        ),
    ] {
        for at in carets {
            if !app.markdown_key(&Key::Named(key), false) {
                app.run(cmd.clone());
            }
            settle(&mut app);
            assert_eq!(app.live_caret_source(), Some(at));
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// ←→ 不停留在预览中不显示的列表记号里：从项的开头向左到上一行的末尾，向右到
/// 下一块内容的开头。
#[test]
fn arrows_skip_the_hidden_block_markers() {
    use winit::keyboard::{Key, NamedKey};
    let left = Key::Named(NamedKey::ArrowLeft);
    let right = Key::Named(NamedKey::ArrowRight);
    let (mut app, dir) = live_doc("md-hidden-markers", "- 一\n- 二\n  - 三\n");
    for (needle, before, after) in [
        ("二", "- 一|\n- 二\n", "- 一\n- |二\n"),
        ("三", "- 二|\n  - 三\n", "- 二\n  - |三\n"),
    ] {
        put_caret(&mut app, needle, false);
        assert!(app.markdown_key(&left, false), "{needle}");
        settle(&mut app);
        assert!(with_caret(&app).contains(before), "{}", with_caret(&app));
        assert!(app.markdown_key(&right, false), "{needle}");
        settle(&mut app);
        assert!(with_caret(&app).contains(after), "{}", with_caret(&app));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn arrow_keys_follow_display_lines_and_document_end_scrolls_into_view() {
    use winit::keyboard::{Key, NamedKey};

    let (mut app, dir) = readme("md-live-arrow-navigation", false, PreviewMode::Preview);
    let shown = crate::app::preview::markdown::laid::full_text(&app).unwrap();
    let at = shown[..shown.find("启动快").unwrap()].chars().count();
    let point = point_of(&app, at);
    super::click(&mut app, point);
    app.run(winc_editor::Command::Insert("很".into()));
    settle(&mut app);

    // ↓ 按显示行向下移动，移出该段后到达下一段；↑ 返回。
    let start = app.live_caret_source().unwrap();
    assert!(app.markdown_key(&Key::Named(NamedKey::ArrowDown), false));
    paint(&mut app);
    let down = app.live_caret_source().unwrap();
    assert!(down > start);
    // 继续向下：经过提示块的标题（没有对应的原文，取块的开头），到达其正文的最后一行；保持
    // 原来的横坐标，落在该行末尾。一段折成几行取决于字体，因此逐行移动到该处，每一步都向前。
    let mut last = down;
    for _ in 0..8 {
        if after_caret(&app, 12).starts_with("\n\n## 安装") {
            break;
        }
        assert!(app.markdown_key(&Key::Named(NamedKey::ArrowDown), false));
        paint(&mut app);
        let now = app.live_caret_source().unwrap();
        assert!(now > last);
        last = now;
    }
    assert!(
        after_caret(&app, 12).starts_with("\n\n## 安装"),
        "{}",
        after_caret(&app, 12)
    );
    // 跳过段落间距，一次向下就到标题末尾。
    assert!(app.markdown_key(&Key::Named(NamedKey::ArrowDown), false));
    paint(&mut app);
    assert!(after_caret(&app, 12).starts_with("\n\n1. "));
    // ⇧↑ 扩展选区。
    app.pointer.modifiers = winit::keyboard::ModifiersState::SHIFT;
    assert!(app.markdown_key(&Key::Named(NamedKey::ArrowUp), false));
    app.pointer.modifiers = winit::keyboard::ModifiersState::empty();
    paint(&mut app);
    assert!(!app.editor.active().unwrap().selection.is_empty());
    // 其他键交给编辑区。
    assert!(!app.markdown_key(&Key::Character("x".into()), false));
    assert!(!app.markdown_key(&Key::Named(NamedKey::ArrowLeft), false));

    // 光标到达文末：预览滚动到光标可见。
    app.run(winc_editor::Command::DocEnd { extend: false });
    paint(&mut app);
    paint(&mut app);
    let y = caret_screen_y(&app);
    let area = app.preview.markdown.area;
    assert!(y >= area.y && y < area.bottom(), "{y} {area:?}");

    let _ = std::fs::remove_dir_all(&dir);
}
