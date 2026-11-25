//! 光标、空行和可编辑位置。

use super::*;

#[test]
fn the_caret_paragraph_keeps_its_look() {
    use winit::keyboard::{Key, NamedKey};

    let (mut app, dir) = live_doc(
        "md-reveal",
        "第一段 **粗** 和 [链接](https://a.dev)。\n\n第二段\n",
    );
    let laid = app.preview.markdown.full_text().unwrap();
    // 光标未接触标记时保持排版后的外观，不显示原文。
    put_caret(&mut app, "第一段", true);
    assert_eq!(app.preview.markdown.full_text().unwrap(), laid);
    // 只显示光标所接触元素的标记。
    put_caret(&mut app, "粗", false);
    let shown = app.preview.markdown.full_text().unwrap();
    assert!(
        shown.contains("**粗**") && !shown.contains("](https"),
        "{shown}"
    );
    put_caret(&mut app, "链接", true);
    let shown = app.preview.markdown.full_text().unwrap();
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
    let shown = app.preview.markdown.full_text().unwrap();
    assert!(shown.contains("粗中文** 和"), "{shown}");
    // 焦点移到终端后组字文字属于终端：预览不再显示，也不绘制光标、不占用候选窗的位置。
    app.focus = Focus::Terminal;
    app.ime.cursor = None;
    paint(&mut app);
    let shown = app.preview.markdown.full_text().unwrap();
    assert!(!shown.contains("中文"), "{shown}");
    assert!(app.ime.cursor.is_none());
    app.focus = Focus::Editor;
    paint(&mut app);
    assert!(
        app.preview
            .markdown
            .full_text()
            .unwrap()
            .contains("粗中文** 和")
    );
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
                    {
                        let l = app.preview.markdown.layout.as_ref().unwrap();
                        l.body_bottom() - l.height
                    },
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
    let second = |app: &App| app.preview.markdown.layout.as_ref().unwrap().tops()[1];
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
            let gap_top = laid.tops()[0] + laid.block_at(0).unwrap().height;
            let gap_bottom = laid.tops()[1];
            (
                laid.column_bounds().0 + 15.0,
                laid.origin_y(app.preview.markdown.area.y, app.preview.markdown.scroll())
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
            let top = laid.tops()[0] + laid.block_at(0).unwrap().height;
            (
                laid.column_bounds().0 + 10.0,
                laid.origin_y(app.preview.markdown.area.y, app.preview.markdown.scroll())
                    + (top + laid.tops()[1]) / 2.0,
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
                laid.column_bounds().0 + 10.0,
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
    let x = layout.column_bounds().0 + 10.0;
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
        let gap_top = laid.tops()[0] + laid.block_at(0).unwrap().height;
        let gap_bottom = laid.tops()[1];
        assert!(gap_bottom > gap_top);
        (
            laid.column_bounds().0 + 15.0,
            laid.origin_y(app.preview.markdown.area.y, app.preview.markdown.scroll())
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
            (
                laid.tops()[0] + laid.block_at(0).unwrap().height,
                laid.tops()[1],
            )
        };
        let (top, bottom) = gap(&app);
        let first = "甲".len();
        for row in 0..3 {
            put_caret(&mut app, "乙", false);
            let laid = app.preview.markdown.layout.as_ref().unwrap();
            let x = laid.column_bounds().0 + 45.0;
            let y = laid.origin_y(app.preview.markdown.area.y, app.preview.markdown.scroll())
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
            [HEADER.len(), HEADER.len() + 2, HEADER.len() + 3],
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
            let x = laid.column_bounds().0 + 32.0;
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
        let shown = app.preview.markdown.full_text().unwrap();
        let at = |c: &str| shown[..shown.find(c).unwrap()].chars().count();
        let (above, below) = (point_of(&app, at("前文")), point_of(&app, at("后文")));
        // 公式在两段之间居中显示：从上往下找第一处落在公式上的点（正文的字不会落到公式的原文处）。
        let area = app.preview.markdown.area;
        let start = text.find(formula).unwrap();
        let x = area.x + area.w / 2.0;
        let middle = (above.1 as usize..below.1 as usize)
            .map(|y| (x, y as f32))
            .find(|&(x, y)| app.preview.markdown.source_at(x, y) == Some(start))
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
        let shown = app.preview.markdown.full_text().unwrap();
        assert!(shown.contains("x^2 + y^2 = z^2"), "{name}: {shown}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// 回归：行内公式（如希腊字母、运算符）中没有可定位的字，点击公式时光标仍应进入公式的原文。
#[test]
fn clicking_inline_math_puts_the_caret_in_its_source() {
    let text = "前文\n\n$\\alpha\\beta\\gamma \\nabla\\partial\\infty$\n\n后文\n";
    let (mut app, dir) = live_doc("md-inline-math", text);
    let shown = app.preview.markdown.full_text().unwrap();
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
        .find(|&(x, y)| app.preview.markdown.source_at(x, y) == Some(tex))
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
    assert_eq!(footnote, Some(laid.block_count() - 1));
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
fn vertical_arrows_follow_wrapped_rows_in_one_source_line() {
    use winit::keyboard::{Key, NamedKey};

    let source = "甲".repeat(100);
    let (mut app, dir) = live_doc("md-arrow-wrapped-row", &source);
    app.start_live_editing();
    paint(&mut app);
    let second_row = {
        let laid = app.preview.markdown.layout.as_ref().unwrap();
        let mut spans = laid.spans(0..1);
        let (_, _, _, first_baseline, ..) = spans.next().expect("first display row");
        let (_, _, _, _, _, span, _, _) = spans
            .find(|&(_, _, _, baseline, ..)| baseline > first_baseline)
            .expect("the paragraph wraps within one source line");
        span.source.expect("the wrapped row maps to the source") as usize
    };
    assert!(second_row > 0 && source.is_char_boundary(second_row));

    assert_eq!(app.live_caret_source(), Some(0));
    assert!(app.markdown_key(&Key::Named(NamedKey::ArrowDown), false));
    paint(&mut app);
    assert_eq!(app.live_caret_source(), Some(second_row));
    assert!(app.markdown_key(&Key::Named(NamedKey::ArrowUp), false));
    paint(&mut app);
    assert_eq!(app.live_caret_source(), Some(0));

    // Horizontal movement and typing still reach the regular editing commands.
    assert!(!app.markdown_key(&Key::Named(NamedKey::ArrowLeft), false));
    assert!(!app.markdown_key(&Key::Character("x".into()), false));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn shift_up_preserves_the_anchor_across_a_displayed_line_break() {
    use winit::keyboard::{Key, ModifiersState, NamedKey};

    let first = "甲甲  \n";
    let (mut app, dir) = live_doc("md-shift-up-hard-break", &format!("{first}乙乙"));
    put_caret(&mut app, "乙", true);
    let anchor = first.chars().count() + 1;
    assert_eq!(
        app.editor.active().unwrap().selection,
        winc_core::Selection::point(anchor)
    );

    app.pointer.modifiers = ModifiersState::SHIFT;
    assert!(app.markdown_key(&Key::Named(NamedKey::ArrowUp), false));
    app.pointer.modifiers = ModifiersState::empty();
    paint(&mut app);
    assert_eq!(
        app.editor.active().unwrap().selection,
        winc_core::Selection::new(anchor, 1)
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn document_end_scrolls_an_offscreen_last_paragraph_into_view() {
    let source = "前文\n\n".repeat(80) + "最后一段";
    let (mut app, dir) = live_doc("md-scroll-to-document-end", &source);
    put_caret(&mut app, "前文", false);
    let top = app.preview.markdown.scroll();
    let (last_top, viewport_height) = {
        let laid = app.preview.markdown.layout.as_ref().unwrap();
        (
            laid.tops().last().unwrap() + (laid.body_bottom() - laid.height),
            app.preview.markdown.area.h,
        )
    };
    assert!(last_top > top + viewport_height);

    app.run(winc_editor::Command::DocEnd { extend: false });
    paint(&mut app);
    assert_eq!(app.live_caret_source(), Some(source.len()));
    assert!(app.preview.markdown.scroll() > top);
    let y = caret_screen_y(&app);
    let area = app.preview.markdown.area;
    assert!((area.y..area.bottom()).contains(&y), "{y} {area:?}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn typing_trailing_spaces_advances_visible_caret() {
    let (mut app, dir) = live_doc(
        "md-trailing-spaces",
        "你好，打了很多空格\n\n**摘要** 正文\n",
    );
    put_caret(&mut app, "你好，打了很多空格", true);
    let caret_x = |app: &App| {
        let laid = app.preview.markdown.layout.as_ref().unwrap();
        laid.caret_point(app.live_caret_source().unwrap())
            .unwrap()
            .0
    };
    let mut prev_x = caret_x(&app);
    for n in 1..=4 {
        app.run(winc_editor::Command::Insert(" ".into()));
        settle(&mut app);
        let x = caret_x(&app);
        assert!(
            x > prev_x + 1.0,
            "第 {n} 个行尾空格未推动光标: {x} <= {prev_x}"
        );
        prev_x = x;
    }

    // 在加粗元素之后输入空格同样推动光标，不丢弃纯空格段。
    put_caret(&mut app, "**摘要** 正文", true);
    let before = caret_x(&app);
    app.run(winc_editor::Command::Insert("   ".into()));
    settle(&mut app);
    let after = caret_x(&app);
    assert!(
        after > before + 5.0,
        "样式后空格未推动光标: {after} <= {before}"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn blank_row_caret_has_clearance_from_neighboring_paragraphs() {
    let source = "适用读者：正文\n\n\n**摘要** 第二段\n";
    let (app, dir) = live_doc("md-blank-clearance", source);
    let laid = app.preview.markdown.layout.as_ref().unwrap();
    let gap_top = laid.tops()[0] + laid.block_at(0).unwrap().height;
    let gap_bottom = laid.tops()[1];
    let second_blank = source.find("\n\n\n").unwrap() + 2;
    let (_, caret_y, caret_h) = laid.caret_point(second_blank).unwrap();
    let top_in_content = laid.content_y(caret_y);
    assert!(
        top_in_content > gap_top,
        "空行光标紧贴上一段底部: {top_in_content} <= {gap_top}"
    );
    assert!(
        top_in_content + caret_h < gap_bottom,
        "空行光标撞进下一段顶部: {} >= {gap_bottom}",
        top_in_content + caret_h
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn typing_spaces_on_blank_line_advances_caret_without_jump_or_overlap() {
    let source = "适用读者：正文\n\n\n**摘要** 第二段\n";
    let (mut app, dir) = live_doc("md-blank-spaces", source);
    let blank_byte = source.find("\n\n\n").unwrap() + 2;
    let blank_char = app
        .editor
        .active()
        .unwrap()
        .text()
        .rope()
        .unwrap()
        .byte_to_char(blank_byte);
    app.editor.active_mut().unwrap().selection = winc_core::Selection::point(blank_char);
    app.start_live_editing();
    settle(&mut app);

    let caret_x = |app: &App| {
        let laid = app.preview.markdown.layout.as_ref().unwrap();
        laid.caret_point(app.live_caret_source().unwrap())
            .unwrap()
            .0
    };
    let mut prev_x = caret_x(&app);
    assert!(prev_x.abs() < 0.5, "空行行首光标应从 0 开始: {prev_x}");

    for n in 1..=4 {
        app.run(winc_editor::Command::Insert(" ".into()));
        paint(&mut app);
        let immediate_x = caret_x(&app);
        settle(&mut app);
        let settled_x = caret_x(&app);
        assert!(
            immediate_x > prev_x + 1.0,
            "第 {n} 个空行空格即时帧未推动光标: {immediate_x} <= {prev_x}"
        );
        assert!(
            settled_x > prev_x + 1.0,
            "第 {n} 个空行空格解析后未推动光标: {settled_x} <= {prev_x}"
        );
        assert!(
            (immediate_x - settled_x).abs() < 0.5,
            "第 {n} 个空行空格解析前后横向跳动: immediate={immediate_x}, settled={settled_x}"
        );
        // 绘制出的光标矩形不得侵入邻接段落。
        let ime = app.ime.cursor.expect("空行光标已绘制");
        let laid = app.preview.markdown.layout.as_ref().unwrap();
        let origin_y = laid.origin_y(app.preview.markdown.area.y, app.preview.markdown.scroll());
        let p0_bottom = origin_y + laid.tops()[0] + laid.block_at(0).unwrap().height;
        let p1_top = origin_y + laid.tops()[1];
        assert!(
            ime.y > p0_bottom,
            "绘制空行光标上沿侵入上一段: {} <= {p0_bottom}",
            ime.y
        );
        assert!(
            ime.y + ime.h < p1_top,
            "绘制空行光标下沿侵入下一段: {} >= {p1_top}",
            ime.y + ime.h
        );
        prev_x = settled_x;
    }

    // 在含 4 个空格的空行上点击行首和空格末尾，分别落回第 0 列和第 4 列。
    let (left, area_y, scroll, caret_y, caret_h) = {
        let laid = app.preview.markdown.layout.as_ref().unwrap();
        let (_, y, h) = laid.caret_point(app.live_caret_source().unwrap()).unwrap();
        (
            laid.column_bounds().0,
            app.preview.markdown.area.y,
            app.preview.markdown.scroll(),
            y,
            h,
        )
    };
    let click_y = area_y - scroll + caret_y + caret_h / 2.0;
    let hit_start = app
        .preview
        .markdown
        .layout
        .as_ref()
        .unwrap()
        .source_at(area_y, scroll, left, click_y);
    let hit_end = app.preview.markdown.layout.as_ref().unwrap().source_at(
        area_y,
        scroll,
        left + prev_x,
        click_y,
    );
    assert_eq!(hit_start, Some(blank_byte));
    assert_eq!(hit_end, Some(blank_byte + 4));

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn epidemic_modeling_multiline_spaces_and_click_roundtrip() {
    let source = concat!(
        "---\n",
        "title: 传染病动力学入门\n",
        "---\n\n",
        "# 传染病动力学入门：SIR 模型\n\n",
        "公共卫生建模学习小组  \n",
        "2024 年 3 月 · 第 1.2 版\\\n",
        "适用读者：具备微积分基础的公共卫生、统计与计算机专业读者\n\n",
        "**摘要** 🦠 本文从仓室模型的基本思想出发，重点讨论**基本再生数** $R_0$ 的含义。\n",
    );
    let (mut app, dir) = live_doc("md-epidemic-repro", source);

    // 1. 模拟用户在「第 1.2 版\」后删掉反斜杠并连打 7 个空格再打标点。
    put_caret(&mut app, "第 1.2 版\\", true);
    app.run(winc_editor::Command::Backspace { word: false });
    settle(&mut app);

    let caret_x = |app: &App| {
        let laid = app.preview.markdown.layout.as_ref().unwrap();
        laid.caret_point(app.live_caret_source().unwrap())
            .unwrap()
            .0
    };
    let mut prev_x = caret_x(&app);
    for n in 1..=7 {
        app.run(winc_editor::Command::Insert(" ".into()));
        paint(&mut app);
        let immediate_x = caret_x(&app);
        settle(&mut app);
        let settled_x = caret_x(&app);
        assert!(
            immediate_x > prev_x + 1.0,
            "多行段落中间行第 {n} 个空格即时帧未推动光标: {immediate_x} <= {prev_x}"
        );
        assert!(
            settled_x > prev_x + 1.0,
            "多行段落中间行第 {n} 个空格解析后未推动光标: {settled_x} <= {prev_x}"
        );
        prev_x = settled_x;
    }

    // 2. 验证在「**摘要** 🦠 ... $R_0$」段落内点击不同文字后，光标回落位置无异常偏移。
    for needle in ["本文", "基本再生数", "的含义"] {
        put_caret(&mut app, needle, false);
        settle(&mut app);
        let src = app.live_caret_source().unwrap();
        let laid = app.preview.markdown.layout.as_ref().unwrap();
        let (x, y, h) = laid.caret_point(src).expect("caret point");
        let left = laid.column_bounds().0;
        let area_y = app.preview.markdown.area.y;
        let scroll = app.preview.markdown.scroll();
        let hit = laid.source_at(
            area_y,
            scroll,
            left + x + 1.0,
            area_y - scroll + y + h / 2.0,
        );
        assert_eq!(
            hit,
            Some(src),
            "点击 `{needle}` 处后命中源码偏移不匹配: hit={hit:?}, expected={src}"
        );
    }

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn blank_row_caret_matches_body_caret_height_and_selects_spaces() {
    let source = "适用读者：正文\n\n\n**摘要** 第二段\n";
    let (mut app, dir) = live_doc("md-blank-caret-height-and-select", source);
    put_caret(&mut app, "适用读者：正文", true);
    let body_h = app.ime.cursor.expect("正文光标已绘制").h;

    let blank_byte = source.find("\n\n\n").unwrap() + 2;
    let blank_char = app
        .editor
        .active()
        .unwrap()
        .text()
        .rope()
        .unwrap()
        .byte_to_char(blank_byte);
    app.editor.active_mut().unwrap().selection = winc_core::Selection::point(blank_char);
    settle(&mut app);
    let blank_h = app.ime.cursor.expect("空行光标已绘制").h;
    assert!(
        (blank_h - body_h).abs() < 0.5,
        "空行光标高度应与正文一致: blank_h={blank_h}, body_h={body_h}"
    );

    // 在空行输入 4 个空格后，选中这些空格必须生成非空选区矩形，且宽度随选中空格数线性增长。
    app.run(winc_editor::Command::Insert("    ".into()));
    settle(&mut app);
    let laid = app.preview.markdown.layout.as_ref().unwrap();
    let two = laid.blank_selection_rects(blank_byte, blank_byte + 2);
    let four = laid.blank_selection_rects(blank_byte, blank_byte + 4);
    assert_eq!(two.len(), 1, "选中 2 个空行空格应生成 1 个选区矩形");
    assert_eq!(four.len(), 1, "选中 4 个空行空格应生成 1 个选区矩形");
    assert!(two[0].w > 2.0 && four[0].w > two[0].w * 1.8);

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn holding_a_key_with_async_parse_does_not_scroll_the_page() {
    let source = concat!(
        "---\n",
        "title: 传染病动力学入门\n",
        "authors:\n  - 公共卫生建模学习小组\n",
        "---\n\n",
        "# 传染病动力学入门\n\n",
        "公共卫生建模学习小组  \n",
        "2024 年 3 月 · 第 1.2 版\n\n",
        "适用读者：正文\n\n",
    )
    .to_owned()
        + &"段落内容。\n\n".repeat(40);
    let (mut app, dir) = live_doc("md-no-scroll-drift", &source);
    put_caret(&mut app, "第 1.2 版", true);
    settle(&mut app);
    let initial_scroll = app.preview.markdown.scroll();

    // 模拟长文档解析超过 INLINE_PARSE，连续按住一个字符并在异步解析完成前后重绘。
    for _ in 0..8 {
        app.preview.markdown.cost = Some(std::time::Duration::from_millis(20));
        app.run(winc_editor::Command::Insert("a".into()));
        paint(&mut app);
        settle(&mut app);
    }
    let after_scroll = app.preview.markdown.scroll();
    assert!(
        (after_scroll - initial_scroll).abs() < 0.5,
        "连续输入字符不应导致页面持续滚动: before={initial_scroll}, after={after_scroll}"
    );
    std::fs::remove_dir_all(dir).unwrap();
}
