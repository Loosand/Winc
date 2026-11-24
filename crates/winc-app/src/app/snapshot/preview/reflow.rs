//! 解析交接和预览重排。

use super::*;

/// 回归：输入之后、重新解析之前，光标所在块的范围来自上一版文档。文档变短或范围落在多字节
/// 字符中间时，绘制或按回车不应 panic（发布版中 panic 即进程中止）。
#[test]
fn edits_before_the_next_parse_keep_the_live_block_inside_the_text() {
    use winit::keyboard::{Key, NamedKey};

    let text =
        "第一段中文\n\n第二段中文内容\n\n| 表头 | 中 |\n| - | - |\n| 一 | 二 |\n\n最后一段\n";
    for keep in ["", "中", "中文\n\n表", "第一段中文\n\n第"] {
        let (mut app, dir) = live_doc("md-stale", text);
        // 按后台解析处理：输入之后的排版与按键仍依据上一版的解析结果。
        app.preview.markdown.cost = Some(std::time::Duration::MAX);
        let doc = app.editor.active_mut().unwrap();
        let count = doc.text().rope().unwrap().len_chars();
        doc.selection = winc_core::Selection::new(0, count);
        // 整篇替换为更短的文字，不等待重新解析：后台的解析结果仍属于上一版。
        app.run(winc_editor::Command::Insert(keep.into()));
        let count = app
            .editor
            .active()
            .unwrap()
            .text()
            .rope()
            .unwrap()
            .len_chars();
        for at in 0..=count {
            app.editor.active_mut().unwrap().selection = winc_core::Selection::point(at);
            let rope = app.editor.active().unwrap().text().rope().unwrap().clone();
            if let Some(live) = app.live_leaf() {
                let _ = rope.byte_slice(live.range.clone());
                let _ = rope.byte_slice(live.leaf.clone());
            }
            app.markdown_key(&Key::Named(NamedKey::Enter), false);
            paint(&mut app);
            app.run(winc_editor::Command::Undo);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn background_markdown_reflow_keeps_the_visible_editing_line_in_place() {
    use std::time::Duration;
    use winc_editor::Command;
    let (mut app, dir) = live_doc("md-async-anchored-edit", &long_notes(1200));
    put_caret(&mut app, "中英文 mixed 600", true);
    let middle = app.preview.markdown.area.h / 2.0;
    let off = caret_screen_y(&app) - app.preview.markdown.area.y - middle;
    app.preview
        .markdown
        .set_scroll(app.preview.markdown.scroll() + off);
    paint(&mut app);
    let original = app.editor.active().unwrap().selection.head;
    // Force the asynchronous path rather than depending on a machine-specific parse time.
    app.preview.markdown.cost = Some(Duration::from_secs(1));
    app.run(Command::Insert("\n\n# 新的标题\n\n继续输入".into()));
    paint(&mut app);
    let before = caret_screen_y(&app);
    let from_top = before - app.preview.markdown.area.y;
    assert!(
        (0.0..app.preview.markdown.area.h).contains(&from_top),
        "before: {before}"
    );
    // Exercise a real selection while the worker's new structure is not yet applied.
    app.run(Command::MoveLeft {
        word: false,
        extend: true,
    });
    paint(&mut app);
    let selection = app.editor.active().unwrap().selection;
    assert!(selection.head > original && !selection.is_empty());
    let before = caret_screen_y(&app);
    settle(&mut app);
    let after = caret_screen_y(&app);
    assert_eq!(app.editor.active().unwrap().selection, selection);
    assert!(
        (after - before).abs() <= 2.0,
        "before {before}, after {after}"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn reflow_after_ime_commit_does_not_scroll_back_to_an_offscreen_caret() {
    use std::time::Duration;
    use winc_editor::Command;
    use winit::event::Ime;

    let (mut app, dir) = live_doc("md-async-ime-scroll", &long_notes(1200));
    put_caret(&mut app, "中英文 mixed 600", true);
    app.run(Command::Insert("\n\n# 新的标题\n\n继续输入".into()));
    settle(&mut app);
    app.run(Command::MoveLeft {
        word: false,
        extend: true,
    });
    paint(&mut app);
    let selection = app.editor.active().unwrap().selection;
    assert!(!selection.is_empty());
    let expected = doc_text(&app).replace("继续输入", "继续输新字👩‍💻e\u{301}");
    app.preview.markdown.cost = Some(Duration::from_secs(1));
    app.on_ime(Ime::Preedit("xin".into(), Some((3, 3))));
    paint(&mut app);
    assert_eq!(app.editor.active().unwrap().selection, selection);
    let old = app.preview.markdown.parsed.clone().unwrap();
    app.on_ime(Ime::Commit("新字".into()));
    paint(&mut app); // enqueue this revision; do not drain the result
    assert!(std::sync::Arc::ptr_eq(
        &old,
        app.preview.markdown.parsed.as_ref().unwrap()
    ));
    for text in ["👩‍💻", "e\u{301}"] {
        app.on_ime(Ime::Commit(text.into()));
    }
    let committed = app.editor.active().unwrap().selection;
    assert_eq!(doc_text(&app), expected);
    paint(&mut app);
    app.scroll_markdown_to(0.0);
    paint(&mut app);
    settle(&mut app);
    assert!(
        app.preview.markdown.scroll() <= 2.0,
        "a reparse chased an offscreen caret"
    );
    assert_eq!(doc_text(&app), expected);
    assert_eq!(app.editor.active().unwrap().selection, committed);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn an_outline_jump_during_reparse_stays_put_for_the_next_drag() {
    use std::time::Duration;
    use winc_editor::Command;
    let source = long_notes(1200);
    let (mut app, dir) = live_doc("md-async-outline-drag", &source);
    app.show_outline();
    paint(&mut app);
    let heading = app
        .panels
        .outline
        .items
        .iter()
        .position(|item| item.name == "第 900 节")
        .unwrap();
    put_caret(&mut app, "中英文 mixed 0", true);
    app.focus = crate::app::Focus::Editor;
    app.preview.markdown.cost = Some(Duration::from_secs(1));
    app.run(Command::Insert("新增".into()));
    paint(&mut app);
    app.jump_to_outline(heading);
    settle(&mut app);
    let at = caret_screen_y(&app) - app.preview.markdown.area.y;
    assert!((0.0..app.preview.markdown.area.h).contains(&at));
    let doc = app.editor.active().unwrap();
    let selection = doc.selection;
    let selected = doc
        .text()
        .rope()
        .unwrap()
        .slice(selection.from()..selection.to());
    assert_eq!(selected, "第 900 节");
    let top = app.preview.markdown.scroll() + at - app.preview.markdown.area.h / 3.0;
    app.scroll_markdown_to(top);
    paint(&mut app);
    let text = doc_text(&app);
    let start = text.find("中英文 mixed 900").unwrap();
    let end = start + text[start..].find("条目").unwrap() + "条".len();
    let md = &app.preview.markdown;
    let laid = md.layout.as_ref().unwrap();
    let point = |byte| {
        let (x, y, h) = laid.caret_point(byte).unwrap();
        (laid.left + x + 0.5, md.area.y + y + h / 2.0 - md.scroll())
    };
    let (from, to) = (point(end), point(start));
    app.on_mouse_move(from.0, from.1);
    app.on_mouse_down(MouseButton::Left);
    app.on_mouse_move(to.0, to.1);
    app.on_mouse_up(MouseButton::Left);
    settle(&mut app);
    let doc = app.editor.active().unwrap();
    let selection = doc.selection;
    assert!(selection.anchor > selection.head);
    let selected = doc
        .text()
        .rope()
        .unwrap()
        .slice(selection.from()..selection.to());
    assert!(
        selected.to_string().starts_with("中英文 mixed 900"),
        "{selected}"
    );
    assert!(selected.to_string().ends_with("条"), "{selected}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn edits_slide_the_blocks_below_and_fade_in_markers() {
    let half = crate::app::animation::MARKDOWN_MOTION / 2;
    let (mut app, dir) = live_doc(
        "md-motion",
        "正文 **粗** 字\n\n| a |\n|---|\n| b |\n| c |\n\n后文\n",
    );
    app.preview.markdown.frozen = Some(half);
    // 光标移入粗体（未修改文字）：两侧的 `**` 淡入，块高度不变，不滑动。
    put_caret(&mut app, "字", false);
    put_caret(&mut app, "粗", false);
    let motion = app.preview.markdown.motion.as_ref().expect("标记淡入");
    assert!(!motion.moving());
    assert_eq!(motion.fading.len(), 1);
    assert_eq!(motion.fading[0].1.len(), 2);
    // 输入文字时不淡入。
    app.run(winc_editor::Command::Insert("很".into()));
    settle(&mut app);
    assert!(
        app.preview
            .markdown
            .motion
            .as_ref()
            .is_none_or(|m| m.fading.is_empty())
    );
    // 删除表格的一行：表格变矮，后文从原位置向上滑动，停在过渡的中点。
    put_caret(&mut app, "c", false);
    assert!(app.live_table_tool("delete-row"));
    settle(&mut app);
    let motion = app.preview.markdown.motion.as_ref().expect("后文往上滑");
    let last = motion.from.len() - 1;
    assert!(motion.from[last] > 10.0, "{:?}", motion.from);
    let offset = motion.offset(last);
    assert!(offset > 0.0 && offset < motion.from[last], "{offset}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 输入多字节的字之后、重新解析之前，光标所在项之后的项照常显示：页面高度不变，下方内容不
/// 上下跳动。
#[test]
fn typing_wide_characters_keeps_the_following_blocks_in_place() {
    let text = "前文\n\n1. 第一\n2. 第二\n3. 第三\n\n1. 第四\n\n后文\n";
    let (mut app, dir) = live_doc("md-typing-wide", text);
    // 按后台解析处理：输入之后的第一帧仍依据上一版的解析结果。
    app.preview.markdown.cost = Some(std::time::Duration::MAX);
    put_caret(&mut app, "第二", true);
    let height = app.preview.markdown.height;
    for c in ["a", "项", "，", "b"] {
        app.run(winc_editor::Command::Insert(c.into()));
        paint(&mut app);
        let shown = crate::app::preview::markdown::laid::full_text(&app).unwrap();
        assert!(
            shown.contains("第三") && shown.contains("第四"),
            "{c}：{shown}"
        );
        assert!((app.preview.markdown.height - height).abs() < 0.5, "{c}");
        settle(&mut app);
        assert!((app.preview.markdown.height - height).abs() < 0.5, "{c}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// 输入之后的第一帧即按当前文字的结构排版：与等待后台解析之后的排版相同，没有按上一版结构
/// 推算的中间帧；输入引起的重排不滑动。
#[test]
fn typing_shows_the_final_layout_in_the_same_frame() {
    use winit::keyboard::{Key, NamedKey};
    let enter = Key::Named(NamedKey::Enter);
    let (mut app, dir) = live_doc("md-same-frame", "前文\n\n- 一\n- 二\n\n后文\n");
    app.preview.markdown.frozen = Some(crate::app::animation::MARKDOWN_MOTION / 2);
    let shown = |app: &App| {
        let text = crate::app::preview::markdown::laid::full_text(app).unwrap();
        (text, app.preview.markdown.height, caret_screen_y(app))
    };
    let step = |app: &mut App, name: &str, act: &dyn Fn(&mut App)| {
        // 固定在界面线程上解析：按上一次解析的耗时选择，机器繁忙时会改到后台。
        app.preview.markdown.cost = Some(std::time::Duration::ZERO);
        act(app);
        paint(app);
        let first = shown(app);
        assert!(
            app.preview
                .markdown
                .motion
                .as_ref()
                .is_none_or(|m| !m.moving()),
            "{name}"
        );
        settle(app);
        assert_eq!(first, shown(app), "{name}");
        first
    };
    put_caret(&mut app, "前文", true);
    // 新的一段：空行的位置与输入第一个字之后段落的位置相同。
    let empty = step(&mut app, "回车", &|app| {
        assert!(app.markdown_key(&enter, false));
    });
    let typed = step(&mut app, "输入", &|app| {
        app.run(winc_editor::Command::Insert("新".into()));
    });
    assert!(
        (empty.1 - typed.1).abs() < 1.5 && (empty.2 - typed.2).abs() < 1.5,
        "{empty:?} {typed:?}"
    );
    // 列表里续一项、把一段文字改成标题：结构变化同样在第一帧显示。
    put_caret(&mut app, "二", true);
    let item = step(&mut app, "续项", &|app| {
        assert!(app.markdown_key(&enter, false));
    });
    put_caret(&mut app, "后文", false);
    let heading = step(&mut app, "标题", &|app| {
        app.run(winc_editor::Command::Insert("# ".into()));
    });
    // 标题的字号、上下间距大于段落：第一帧的总高度已经增加。
    assert!(heading.1 > item.1 + 10.0, "{heading:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
