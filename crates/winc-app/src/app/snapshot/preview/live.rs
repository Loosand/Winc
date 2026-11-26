//! Markdown 预览中实时编辑的测试：光标、按键和围栏代码块。

use super::*;

#[test]
fn split_preview_follows_edits_and_scrolls() {
    let (mut app, dir) = readme("md-edit", false, PreviewMode::Split);
    let before = app.preview.markdown.height;
    // 在文末追加两段：预览变高。
    app.run(winc_editor::Command::DocEnd { extend: false });
    app.run(winc_editor::Command::Insert(
        "\n\n多一段\n\n再多一段\n".into(),
    ));
    settle(&mut app);
    assert!(app.preview.markdown.height > before);

    // 滚轮滚动预览。
    let area = app.preview.markdown.area;
    app.pointer.mouse = (area.x + 50.0, area.y + 50.0);
    app.preview.markdown.scroll_to(0.0);
    app.on_wheel(MouseScrollDelta::PixelDelta(PhysicalPosition::new(
        0.0, -120.0,
    )));
    assert!(app.preview.markdown.scroll() > 0.0);
    // 不会滚动超过底部。
    app.on_wheel(MouseScrollDelta::PixelDelta(PhysicalPosition::new(
        0.0, -100_000.0,
    )));
    paint(&mut app);
    let max = app.preview.markdown.height - area.h;
    assert!((app.preview.markdown.scroll() - max).abs() < 0.5);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_full_preview_edits_the_document() {
    let (mut app, dir) = readme("md-live", false, PreviewMode::Preview);
    // 刚打开时整篇按排版显示。
    let shown = app.preview.markdown.full_text().unwrap();
    assert!(!shown.contains("**Rust**"));
    // 点击「启动快」：光标移到原文中对应的位置，被点击的文字位置不变。该段落仍按排版显示，
    // 只显示光标接触到的标记，即「启动快」两侧的 `*`；未接触的 `**Rust**` 不显示标记。
    let at = shown[..shown.find("启动快").unwrap()].chars().count();
    let (x, y) = point_of(&app, at);
    super::click(&mut app, (x, y));
    assert_eq!(after_caret(&app, 3), "启动快");
    let before = caret_screen_y(&app);
    paint(&mut app);
    let shown = app.preview.markdown.full_text().unwrap();
    assert!(
        shown.contains("一个用 Rust 写的") && shown.contains("*启动快*"),
        "{shown}"
    );
    assert!(!shown.contains("# winc"));
    assert!((caret_screen_y(&app) - before).abs() < 1.0);

    // 输入修改的是文档；重新解析之前，预览已显示刚输入的文字。
    app.run(winc_editor::Command::Insert("很".into()));
    paint(&mut app);
    assert!(
        app.preview
            .markdown
            .full_text()
            .unwrap()
            .contains("很启动快")
    );
    settle(&mut app);
    assert!(
        app.editor
            .active()
            .unwrap()
            .text()
            .rope()
            .unwrap()
            .to_string()
            .contains("*很启动快*")
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn enter_after_a_fence_closes_the_block() {
    use winit::keyboard::{Key, NamedKey};
    let enter = Key::Named(NamedKey::Enter);

    let (mut app, dir) = live_doc("md-fence", "前文\n\n后文\n");
    put_caret(&mut app, "前文", true);
    for text in ["\n\n", "```rust"] {
        app.run(winc_editor::Command::Insert(text.into()));
    }
    // 尚未解析（仍为正文段落）时同样补全闭围栏，光标位于两行围栏之间；后文不会变为代码。
    assert!(app.markdown_key(&enter, false));
    assert_eq!(with_caret(&app), "前文\n\n```rust\n|\n```\n\n后文\n");
    settle(&mut app);
    app.run(winc_editor::Command::Insert("fn x() {}".into()));
    settle(&mut app);
    let shown = app.preview.markdown.full_text().unwrap();
    assert!(
        shown.contains("fn x() {}") && shown.contains("后文"),
        "{shown}"
    );
    // 在已闭合的围栏上按回车：按普通换行处理。
    put_caret(&mut app, "```rust", true);
    let _ = app.markdown_key(&enter, false);
    assert_eq!(doc_text(&app).matches("```").count(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn backspace_after_a_marker_removes_it() {
    use winit::keyboard::{Key, NamedKey};
    let backspace = Key::Named(NamedKey::Backspace);

    let (mut app, dir) = live_doc(
        "md-backspace",
        "- [ ] 甲\n- 乙\n\n> 引\n\n```\n- x\n```\n\n## 题\n",
    );
    put_caret(&mut app, "甲", false);
    // 逐层移除任务框和列表记号；「乙」成为上一项中的另一段。
    assert!(app.markdown_key(&backspace, false));
    assert!(
        with_caret(&app).starts_with("- |甲\n"),
        "{}",
        with_caret(&app)
    );
    settle(&mut app);
    put_caret(&mut app, "乙", false);
    assert!(app.markdown_key(&backspace, false));
    assert!(
        with_caret(&app).contains("\n  |乙\n"),
        "{}",
        with_caret(&app)
    );
    settle(&mut app);
    put_caret(&mut app, "引", false);
    assert!(app.markdown_key(&backspace, false));
    assert!(with_caret(&app).contains("\n|引\n"), "{}", with_caret(&app));
    settle(&mut app);
    // 标题的 `#` 全部移除，成为段落。
    put_caret(&mut app, "题", false);
    assert!(app.markdown_key(&backspace, false));
    assert!(
        with_caret(&app).ends_with("\n|题\n"),
        "{}",
        with_caret(&app)
    );
    settle(&mut app);
    // 光标前还有文字或位于代码块中时，按普通删除处理，删除一个字符。
    put_caret(&mut app, "引", true);
    assert!(!app.markdown_key(&backspace, false));
    put_caret(&mut app, "x", false);
    assert!(!app.markdown_key(&backspace, false));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn enter_in_the_preview_continues_lists_and_splits_paragraphs() {
    use winit::keyboard::{Key, NamedKey};
    let enter = Key::Named(NamedKey::Enter);

    let (mut app, dir) = live_doc("md-enter", "- 一\n- 二\n\n段落一\n\n```\ncode\n```\n");
    put_caret(&mut app, "二", true);
    // 列表里回车续一项，空项再回车结束列表、另起一段。
    assert!(app.markdown_key(&enter, false));
    assert_eq!(
        with_caret(&app),
        "- 一\n- 二\n- |\n\n段落一\n\n```\ncode\n```\n"
    );
    settle(&mut app);
    assert!(app.markdown_key(&enter, false));
    app.run(winc_editor::Command::Insert("新".into()));
    assert_eq!(
        with_caret(&app),
        "- 一\n- 二\n\n新|\n\n段落一\n\n```\ncode\n```\n"
    );
    // 段落中回车另起一段，⇧ 回车段内换行。
    put_caret(&mut app, "段落", true);
    assert!(app.markdown_key(&enter, false));
    assert!(with_caret(&app).contains("段落\n\n|一"));
    settle(&mut app);
    app.pointer.modifiers = winit::keyboard::ModifiersState::SHIFT;
    assert!(app.markdown_key(&enter, false));
    app.pointer.modifiers = winit::keyboard::ModifiersState::empty();
    assert!(
        with_caret(&app).contains("段落\n\n\n|一"),
        "{}",
        with_caret(&app)
    );
    // 代码块中交给编辑区按普通方式换行。
    put_caret(&mut app, "code", true);
    assert!(!app.markdown_key(&enter, false));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn enter_before_an_adjacent_block_leaves_room_for_a_paragraph() {
    use winc_editor::Command;
    use winit::keyboard::{Key, NamedKey};

    for (name, source, before, after) in [
        ("ordered-list", "## 标题\n2. 保留\n", "## 标题", "2. 保留\n"),
        ("rule", "## 标题\r\n---\r\n", "## 标题", "---\r\n"),
        (
            "fence",
            "正文\n```\n保留\n```\n",
            "正文",
            "```\n保留\n```\n",
        ),
    ] {
        let (mut app, dir) = live_doc(&format!("md-enter-before-{name}"), source);
        let eol = if source.contains('\r') { "\r\n" } else { "\n" };
        put_caret(&mut app, before.trim_start_matches("## "), true);
        assert!(app.markdown_key(&Key::Named(NamedKey::Enter), false));
        settle(&mut app);
        let empty_paragraph = format!("{before}{eol}{eol}|{eol}{eol}{after}");
        assert_eq!(with_caret(&app), empty_paragraph, "{name}");
        app.run(Command::Insert("新段落".into()));
        settle(&mut app);
        assert_eq!(
            doc_text(&app),
            format!("{before}{eol}{eol}新段落{eol}{eol}{after}"),
            "{name}"
        );
        app.run(Command::Undo);
        app.run(Command::Undo);
        assert_eq!(doc_text(&app), source, "{name}");
        app.run(Command::Redo);
        assert_eq!(with_caret(&app), empty_paragraph, "{name}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn leaving_a_task_list_and_joining_paragraphs_preserves_the_crlf_note() {
    use winc_editor::Command;
    use winit::keyboard::{Key, NamedKey};
    let source = "- [x] 已做\r\n- [ ] ";
    let (mut app, dir) = live_doc("md-task-exit-join", source);
    put_caret(&mut app, "- [ ] ", true);
    let enter = Key::Named(NamedKey::Enter);
    assert!(app.markdown_key(&enter, false));
    app.run(Command::Insert("第一段".into()));
    settle(&mut app);
    assert!(app.markdown_key(&enter, false));
    app.run(Command::Insert("第二段".into()));
    let split = "- [x] 已做\r\n\r\n第一段\r\n\r\n第二段";
    assert_eq!(doc_text(&app), split);
    put_caret(&mut app, "第二段", false);
    assert!(app.markdown_key(&Key::Named(NamedKey::Backspace), false));
    assert_eq!(with_caret(&app), "- [x] 已做\r\n\r\n第一段|第二段");
    app.run(Command::Undo);
    assert_eq!(doc_text(&app), split);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn one_backspace_removes_the_empty_paragraph_created_after_a_heading() {
    use winc_editor::Command;
    use winit::keyboard::{Key, NamedKey};

    let enter = Key::Named(NamedKey::Enter);
    let backspace = Key::Named(NamedKey::Backspace);
    for (name, source) in [
        ("list", "# Skills\n\n- 第一项\n"),
        ("adjacent-crlf", "# Skills\r\n2. 第一项\r\n"),
        ("end", "# Skills"),
    ] {
        let (mut app, dir) = live_doc(&format!("md-enter-backspace-{name}"), source);
        put_caret(&mut app, "Skills", true);
        assert!(app.markdown_key(&enter, false));
        settle(&mut app);
        let empty_paragraph = with_caret(&app);
        if !app.markdown_key(&backspace, false) {
            app.run(Command::Backspace { word: false });
        }
        settle(&mut app);
        assert_eq!(doc_text(&app), source, "{name}");
        assert_eq!(app.live_caret_source(), Some("# Skills".len()), "{name}");
        app.run(Command::Undo);
        assert_eq!(with_caret(&app), empty_paragraph, "{name}");
        app.run(Command::Redo);
        assert_eq!(doc_text(&app), source, "{name}");
        assert_eq!(app.live_caret_source(), Some("# Skills".len()), "{name}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn shift_enter_keeps_a_visible_line_break_when_the_note_is_reopened() {
    use winc_editor::Command;
    use winit::keyboard::{Key, ModifiersState, NamedKey};
    let enter = Key::Named(NamedKey::Enter);
    for (name, prefix, continuation) in [
        ("paragraph", "", ""),
        ("list", "- ", "  "),
        ("quote", "> ", "> "),
        ("nested-continuation", "- 先写一行\n  ", "  "),
    ] {
        let source = format!("{prefix}甲乙\n");
        let (mut app, dir) = live_doc(&format!("md-hard-break-{name}"), &source);
        put_caret(&mut app, "甲", true);
        app.pointer.modifiers = ModifiersState::SHIFT;
        assert!(app.markdown_key(&enter, false));
        app.pointer.modifiers = ModifiersState::empty();
        let expected = format!("{prefix}甲  \n{continuation}乙\n");
        assert_eq!(doc_text(&app), expected, "{name}");
        assert!(with_caret(&app).contains(&format!("{continuation}|乙")));
        app.run(Command::Undo);
        assert_eq!(doc_text(&app), source, "{name}");
        app.run(Command::Redo);
        assert_eq!(doc_text(&app), expected, "{name}");
        let id = app.editor.active().unwrap().id;
        assert!(app.save_document(id, None));
        let path = dir.join("LIVE.md");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
        let mut reopened = app_with(&path, false);
        paint(&mut reopened);
        reopened.set_preview_mode(PreviewMode::Preview);
        settle(&mut reopened);
        let shown = reopened.preview.markdown.full_text().unwrap();
        assert!(shown.contains("甲\n乙"), "{name}: {shown:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn enter_at_an_existing_crlf_break_splits_the_paragraph_without_a_stray_lf() {
    use winit::keyboard::{Key, NamedKey};
    let (mut app, dir) = live_doc("md-enter-crlf", "第一行\r\n第二行\r\n");
    put_caret(&mut app, "第一行", true);
    assert!(app.markdown_key(&Key::Named(NamedKey::Enter), false));
    assert_eq!(with_caret(&app), "第一行\r\n\r\n|第二行\r\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn code_fences_show_only_with_the_caret_on_them() {
    let (mut app, dir) = live_doc("md-fences", "前\n\n```rust\nfn x() {}\n```\n");
    put_caret(&mut app, "fn x", false);
    let shown = app.preview.markdown.full_text().unwrap();
    assert!(shown.contains("fn x") && !shown.contains("```"), "{shown}");
    // 按 ← 移到围栏行：显示围栏，可以修改语言标识。
    app.run(winc_editor::Command::MoveLeft {
        extend: false,
        word: false,
    });
    settle(&mut app);
    let shown = app.preview.markdown.full_text().unwrap();
    assert!(shown.contains("```rust\nfn x"), "{shown}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 回车续出的空项：光标停在新项的记号之后，不归到下一项；下一项照常按排版显示。
#[test]
fn enter_in_a_list_puts_the_caret_in_the_new_empty_item() {
    use winit::keyboard::{Key, NamedKey};
    let (mut app, dir) = live_doc("md-empty-item", "1. 一\n2. 二\n3. 三\n\n后文\n");
    put_caret(&mut app, "二", true);
    let before = caret_screen_y(&app);
    assert!(app.markdown_key(&Key::Named(NamedKey::Enter), false));
    settle(&mut app);
    assert_eq!(with_caret(&app), "1. 一\n2. 二\n3. |\n4. 三\n\n后文\n");
    assert!(caret_screen_y(&app) > before + 10.0);
    let shown = app.preview.markdown.full_text().unwrap();
    assert!(shown.contains("\n三\n") && !shown.contains("3."), "{shown}");
    // 在空项上输入的文字排在这一项里。
    app.run(winc_editor::Command::Insert("新".into()));
    settle(&mut app);
    let shown = app.preview.markdown.full_text().unwrap();
    assert!(shown.contains("二\n新\n三"), "{shown}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 删除键：空项上先移除记号，再删除留下的缩进并回到上一行末尾。
#[test]
fn backspace_on_an_empty_item_removes_it_in_two_steps() {
    use winit::keyboard::{Key, NamedKey};
    let back = Key::Named(NamedKey::Backspace);
    let (mut app, dir) = live_doc("md-back-item", "- 甲\n  - 乙\n  - \n\n后文\n");
    put_caret(&mut app, "  - \n", true);
    let doc = app.editor.active_mut().unwrap();
    doc.selection = winc_core::Selection::point(doc.selection.head - 1);
    settle(&mut app);
    assert!(app.markdown_key(&back, false));
    settle(&mut app);
    assert_eq!(with_caret(&app), "- 甲\n  - 乙\n    |\n\n后文\n");
    assert!(app.markdown_key(&back, false));
    settle(&mut app);
    assert_eq!(with_caret(&app), "- 甲\n  - 乙|\n\n后文\n");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 有序列表中间回车续出一项：其后各项的序号顺延，光标停在新的一项里，撤销一步恢复原文。
#[test]
fn enter_in_an_ordered_list_renumbers_the_items_below() {
    use winit::keyboard::{Key, NamedKey};
    let text = "1. 一\n2. 二\n   - 子项\n3. 三\n\n后文\n";
    let (mut app, dir) = live_doc("md-renumber", text);
    put_caret(&mut app, "一", true);
    assert!(app.markdown_key(&Key::Named(NamedKey::Enter), false));
    settle(&mut app);
    assert_eq!(
        with_caret(&app),
        "1. 一\n2. |\n3. 二\n   - 子项\n4. 三\n\n后文\n"
    );
    app.run(winc_editor::Command::Undo);
    assert_eq!(doc_text(&app), text);
    let _ = std::fs::remove_dir_all(&dir);
}

/// 行内格式的命令只在可编辑的预览中可用：选中的文字加上、去掉标记，没有选区时插入一对标记；
/// 代码块中不修改。
#[test]
fn format_commands_toggle_inline_marks_in_the_live_preview() {
    use crate::keymap::Action;
    let (mut app, dir) = live_doc("md-format", "正文 粗体 字\n\n```\ncode\n```\n");
    let select = |app: &mut App, needle: &str| {
        let at = doc_text(app).find(needle).unwrap();
        let doc = app.editor.active_mut().unwrap();
        let rope = doc.text().rope().unwrap();
        let (a, b) = (rope.byte_to_char(at), rope.byte_to_char(at + needle.len()));
        doc.selection = winc_core::Selection::new(a, b);
        settle(app);
    };
    select(&mut app, "粗体");
    assert!(app.run_action(Action::MarkdownBold));
    settle(&mut app);
    assert!(doc_text(&app).starts_with("正文 **粗体** 字\n"));
    let sel = app.editor.active().unwrap().selection;
    assert_eq!(sel.to() - sel.from(), 2);
    assert!(app.run_action(Action::MarkdownBold));
    settle(&mut app);
    assert!(doc_text(&app).starts_with("正文 粗体 字\n"));
    // 高亮同样是行内格式：选中的文字加上 `==`，再来一次去掉。
    select(&mut app, "字");
    assert!(app.run_action(Action::MarkdownHighlight));
    settle(&mut app);
    assert!(doc_text(&app).starts_with("正文 粗体 ==字==\n"));
    assert!(app.run_action(Action::MarkdownHighlight));
    settle(&mut app);
    assert!(doc_text(&app).starts_with("正文 粗体 字\n"));
    put_caret(&mut app, "字", true);
    assert!(app.run_action(Action::MarkdownCode));
    assert_eq!(with_caret(&app), "正文 粗体 字`|`\n\n```\ncode\n```\n");
    app.run(winc_editor::Command::Undo);
    put_caret(&mut app, "code", false);
    let before = doc_text(&app);
    assert!(!app.run_action(Action::MarkdownItalic));
    assert_eq!(doc_text(&app), before);
    // 源代码中同样可用（`markdown.shortcuts` 默认开着），关闭后同一个键执行其他命令。
    app.set_preview_mode(PreviewMode::Source);
    paint(&mut app);
    assert!(app.action_available(Action::MarkdownBold));
    app.config.markdown_shortcuts = Some(false);
    assert!(!app.action_available(Action::MarkdownBold));
    let _ = std::fs::remove_dir_all(&dir);
}

/// 任务列表可以逐字输入：`- [` 之后括号配对补上 `]`，再输入空格即成为任务项；`[]`、`-[]`
/// 加空格是同样的简写。
#[test]
fn task_items_can_be_typed() {
    use winit::keyboard::Key;
    let space = Key::Character(" ".into());
    for typed in [&["-", " ", "["][..], &["[", "]"], &["-", "[", "]"]] {
        let (mut app, dir) = live_doc("md-typed-task", "前文\n\n\n\n后文\n");
        let doc = app.editor.active_mut().unwrap();
        doc.selection = winc_core::Selection::point(4);
        settle(&mut app);
        for key in typed {
            app.run(winc_editor::Command::Insert((*key).into()));
            paint(&mut app);
        }
        assert!(app.markdown_key(&space, false), "{typed:?}");
        paint(&mut app);
        assert_eq!(with_caret(&app), "前文\n\n- [ ] |\n\n后文\n", "{typed:?}");
        app.run(winc_editor::Command::Insert("任务".into()));
        settle(&mut app);
        let parsed = app.preview.markdown.parsed.clone().unwrap();
        let task = parsed.blocks.iter().find_map(|b| match b {
            winc_preview::markdown::Block::List(_, items) => items[0].task,
            _ => None,
        });
        assert_eq!(task, Some(false), "{typed:?}");
        let shown = app.preview.markdown.full_text().unwrap();
        assert!(shown.contains("前文\n任务\n后文"), "{shown}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// 输入规则（任务框简写、第三个围栏字符自动闭合、围栏开行回车自动闭合）必须支持单步撤销：
/// 第 1 次撤销仅退回自动转换并保留用户原始输入，第 2 次撤销才撤回原始输入；组字期间不触发。
#[test]
fn input_rules_support_single_step_undo_and_respect_ime_preedit() {
    use winc_editor::Command;
    use winit::keyboard::{Key, NamedKey};

    let space = Key::Character(" ".into());
    let tick = Key::Character("`".into());
    let enter = Key::Named(NamedKey::Enter);

    // 1. 空格任务框规则：`[]` + 空格 -> `- [ ] ` -> 1 次 Undo -> `[] ` -> 2 次 Undo -> `[]`。
    {
        let (mut app, dir) = live_doc("md-rule-undo-task", "前文\n\n\n\n后文\n");
        let doc = app.editor.active_mut().unwrap();
        doc.selection = winc_core::Selection::point(4);
        settle(&mut app);
        app.run(Command::Insert("[]".into()));
        paint(&mut app);
        assert!(app.markdown_key(&space, false));
        assert_eq!(with_caret(&app), "前文\n\n- [ ] |\n\n后文\n");

        app.run(Command::Undo);
        assert_eq!(with_caret(&app), "前文\n\n[] |\n\n后文\n");
        app.run(Command::Undo);
        assert_eq!(with_caret(&app), "前文\n\n[]|\n\n后文\n");
        app.run(Command::Redo);
        assert_eq!(with_caret(&app), "前文\n\n[] |\n\n后文\n");
        app.run(Command::Redo);
        assert_eq!(with_caret(&app), "前文\n\n- [ ] |\n\n后文\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 2. 第三个反引号自动闭围栏：`` + ` -> ```|\n``` -> 1 次 Undo -> ```| -> 2 次 Undo -> ``|。
    {
        let (mut app, dir) = live_doc("md-rule-undo-fence-char", "前文\n\n\n\n后文\n");
        let doc = app.editor.active_mut().unwrap();
        doc.selection = winc_core::Selection::point(4);
        settle(&mut app);
        app.run(Command::Insert("``".into()));
        paint(&mut app);
        assert!(app.markdown_key(&tick, false));
        assert_eq!(with_caret(&app), "前文\n\n```|\n```\n\n后文\n");

        app.run(Command::Undo);
        assert_eq!(with_caret(&app), "前文\n\n```|\n\n后文\n");
        app.run(Command::Undo);
        assert_eq!(with_caret(&app), "前文\n\n``|\n\n后文\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 3. 围栏开行回车自动闭围栏：```rust + Enter -> ```rust\n|\n``` -> 1 次 Undo -> ```rust\n| -> 2 次 Undo -> ```rust|。
    {
        let (mut app, dir) = live_doc("md-rule-undo-fence-enter", "前文\n\n```rust\n\n后文\n");
        put_caret(&mut app, "```rust", true);
        assert!(app.markdown_key(&enter, false));
        assert_eq!(with_caret(&app), "前文\n\n```rust\n|\n```\n\n后文\n");

        app.run(Command::Undo);
        assert_eq!(with_caret(&app), "前文\n\n```rust\n|\n\n后文\n");
        app.run(Command::Undo);
        assert_eq!(with_caret(&app), "前文\n\n```rust|\n\n后文\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 4. IME 组字期间不得触发空格任务框或围栏输入规则。
    {
        let (mut app, dir) = live_doc("md-rule-ime-guard", "前文\n\n[]\n\n后文\n");
        put_caret(&mut app, "[]", true);
        app.ime.preedit = "kong".into();
        assert!(!app.markdown_key(&space, false));
        assert_eq!(doc_text(&app), "前文\n\n[]\n\n后文\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn multiple_enters_then_backspaces_return_cleanly_without_phantom_separator() {
    use winc_editor::Command;
    use winit::keyboard::{Key, NamedKey};

    let enter = Key::Named(NamedKey::Enter);
    let backspace = Key::Named(NamedKey::Backspace);
    let press_backspace = |app: &mut App| {
        if !app.markdown_key(&backspace, false) {
            app.run(Command::Backspace { word: false });
        }
        settle(app);
    };

    for (name, source, anchor) in [
        ("between-paragraphs", "第一段。\n\n第二段。\n", "第一段。"),
        (
            "between-paragraphs-crlf",
            "第一段。\r\n\r\n第二段。\r\n",
            "第一段。",
        ),
        ("after-heading", "# 标题\n\n正文。\n", "标题"),
        ("at-eof-no-newline", "最后一段。", "最后一段。"),
        ("at-eof-trailing-newline", "最后一段。\n", "最后一段。"),
    ] {
        for enters in [2usize, 3] {
            let (mut app, dir) = live_doc(&format!("md-multi-enter-{name}-{enters}"), source);
            put_caret(&mut app, anchor, true);
            let initial = with_caret(&app);

            for _ in 0..enters {
                assert!(app.markdown_key(&enter, false), "{name}/{enters}");
                settle(&mut app);
            }
            for step in 0..enters {
                press_backspace(&mut app);
                if step + 1 < enters {
                    assert_ne!(
                        doc_text(&app),
                        source,
                        "{name}/{enters}: step {step} returned too early"
                    );
                }
            }
            // 回车 N 次后退格 N 次，恰好回到原段末尾且保留原有段落分隔，不落入分隔空行。
            assert_eq!(with_caret(&app), initial, "{name}/{enters}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

#[test]
fn markdown_source_mode_wraps_long_lines_by_default() {
    let long = "这是一段在源码模式下应该按编辑区宽度自动折行显示的长文本。".repeat(8);
    let source = format!("# 标题\n\n{long}\n\n尾段。\n");
    let (mut app, dir) = live_doc("md-source-default-wrap", &source);
    assert_eq!(app.config.word_wrap, None);

    app.set_preview_mode(PreviewMode::Source);
    paint(&mut app);
    let doc = app.editor.active().unwrap();
    assert!(doc.is_wrapping(), "Markdown 源码模式默认应自动换行");
    assert!(doc.rows_of(2) > 1, "长段落在源码模式下应折为多行");

    // 显式关闭 word_wrap 时仍尊重用户配置。
    app.config.word_wrap = Some(false);
    app.word_wrap_changed();
    paint(&mut app);
    let doc = app.editor.active().unwrap();
    assert!(!doc.is_wrapping());
    assert_eq!(doc.rows_of(2), 1);

    let _ = std::fs::remove_dir_all(&dir);
}
