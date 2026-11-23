//! Markdown 粘贴的测试：链接、富文本、表格和粘贴后的光标。

use super::*;

#[test]
fn pasting_urls_and_spreadsheet_cells_rewrites_them() {
    let (mut app, dir) = live_doc("md-paste-link", "见文档。\n\n`代码`\n\n```\n块\n```\n");
    let select = |app: &mut App, needle: &str| {
        let text = doc_text(app);
        let byte = text.find(needle).unwrap();
        let doc = app.editor.active_mut().unwrap();
        let rope = doc.text().rope().unwrap();
        let (a, b) = (
            rope.byte_to_char(byte),
            rope.byte_to_char(byte + needle.len()),
        );
        doc.selection = winc_core::Selection::new(a, b);
    };
    select(&mut app, "文档");
    assert_eq!(
        app.markdown_paste(" https://a.dev/x\n").as_deref(),
        Some("[文档](https://a.dev/x)")
    );
    assert_eq!(
        app.markdown_paste("https://a.dev/(1)").as_deref(),
        Some("[文档](<https://a.dev/(1)>)")
    );
    // 不是网址、没有选区、在行内代码或代码块中时按普通方式粘贴。
    assert!(app.markdown_paste("文字").is_none());
    assert!(app.markdown_paste("https://a.dev x").is_none());
    select(&mut app, "代码");
    assert!(app.markdown_paste("https://a.dev").is_none());
    select(&mut app, "块");
    assert!(app.markdown_paste("https://a.dev").is_none());
    put_caret(&mut app, "见", true);
    assert!(app.markdown_paste("https://a.dev").is_none());
    // 切换回源代码：预览已释放解析结果，按围栏判断代码块。
    app.set_preview_mode(PreviewMode::Source);
    settle(&mut app);
    assert!(app.preview.markdown.parsed.is_none() && app.preview.markdown.layout.is_none());
    select(&mut app, "块");
    assert!(app.markdown_paste("https://a.dev").is_none());
    select(&mut app, "文档");
    assert!(app.markdown_paste("https://a.dev").is_some());

    // 没有选区时图片网址写成图片，表格单元格写成单独成块的表格。
    put_caret(&mut app, "见文档。", true);
    let image = app.markdown_paste("https://a.dev/shot.PNG?raw=1");
    assert_eq!(image.as_deref(), Some("![](https://a.dev/shot.PNG?raw=1)"));
    assert!(app.markdown_paste("https://a.dev/page").is_none());
    let table = app.markdown_paste("名称\t数量\n苹果\t3\n");
    let expected = "\n\n| 名称 | 数量 |\n| --- | --- |\n| 苹果 | 3 |";
    assert_eq!(table.as_deref(), Some(expected));

    let _ = std::fs::remove_dir_all(&dir);
}

/// 剪贴板中带结构的 HTML 粘贴为 Markdown；没有结构的 HTML 和「粘贴为纯文本」粘贴文字。
#[test]
fn pasting_web_content_writes_markdown() {
    let (mut app, dir) = live_doc("md-paste-html", "前文\n");
    put_caret(&mut app, "前文", true);
    app.clipboard.copied = Some("标题\n要点".into());
    app.clipboard.copied_html = Some("<h2>标题</h2><ul><li><b>要点</b></li></ul>".into());
    app.paste();
    assert_eq!(doc_text(&app), "前文\n\n## 标题\n\n- **要点**\n");
    // 一段带链接的文字就地插入。
    app.run(winc_editor::Command::Insert("\n\n".into()));
    app.clipboard.copied = Some("看文档".into());
    app.clipboard.copied_html = Some("看<a href=\"https://winc.si\">文档</a>".into());
    app.paste();
    assert!(doc_text(&app).ends_with("\n\n看[文档](https://winc.si)\n"));
    app.clipboard.copied = Some("fn main() {}".into());
    app.clipboard.copied_html =
        Some("<div><span style=\"color:#00f\">fn</span> main() {}</div>".into());
    app.paste();
    assert!(doc_text(&app).ends_with("winc.si)fn main() {}\n"));
    // 选中文字后粘贴 HTML 链接时，以选中的文字作为链接文字。
    app.run(winc_editor::Command::Insert("\n\n说明".into()));
    let doc = app.editor.active_mut().unwrap();
    let end = doc.text().rope().unwrap().len_chars() - 1;
    doc.selection = winc_core::Selection::new(end - 2, end);
    app.clipboard.copied = Some("https://winc.si".into());
    app.clipboard.copied_html = Some("<a href=\"https://winc.si\">https://winc.si</a>".into());
    app.paste();
    assert!(
        doc_text(&app).ends_with("\n\n[说明](https://winc.si)\n"),
        "{}",
        doc_text(&app)
    );
    app.clipboard.copied = Some("纯".into());
    app.clipboard.copied_html = Some("<b>纯</b>".into());
    assert!(app.run_action(crate::keymap::Action::MarkdownPastePlain));
    assert!(doc_text(&app).ends_with("(https://winc.si)纯\n"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pasting_a_web_table_preserves_cells_even_when_text_is_absent_or_tabular() {
    use winc_editor::Command;
    for (name, plain) in [
        ("html-only", None),
        ("html-and-tsv", Some("名称\t说明\n重点\t甲乙")),
    ] {
        let (mut app, dir) = live_doc(&format!("md-web-table-{name}"), "前后\n");
        put_caret(&mut app, "前", true);
        app.clipboard.copied = plain.map(str::to_owned);
        app.clipboard.copied_html = Some("<table><tr><th>名称</th><th>说明</th></tr><tr><td><strong>重点</strong></td><td>甲<br>乙</td></tr></table>".into());
        app.paste();
        let expected = "前\n\n| 名称 | 说明 |\n| --- | --- |\n| **重点** | 甲<br>乙 |\n\n后\n";
        assert_eq!(doc_text(&app), expected);
        assert_eq!(after_caret(&app, 1), "后");
        app.run(Command::Insert("续".into()));
        assert_eq!(doc_text(&app), expected.replace("后", "续后"));
        app.run(Command::Undo);
        assert_eq!(doc_text(&app), expected);
        app.run(Command::Undo);
        assert_eq!(doc_text(&app), "前后\n");
        app.run(Command::Redo);
        settle(&mut app);
        assert_eq!(after_caret(&app, 1), "后");
        let id = app.editor.active().unwrap().id;
        assert!(app.save_document(id, None));
        assert_eq!(
            std::fs::read_to_string(dir.join("LIVE.md")).unwrap(),
            expected
        );
        assert!(
            app.preview
                .markdown
                .parsed
                .as_ref()
                .unwrap()
                .blocks
                .iter()
                .any(|b| matches!(b, winc_preview::markdown::Block::Table(..)))
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn pasting_a_web_table_at_the_end_leaves_the_caret_in_a_new_paragraph() {
    use winc_editor::Command;
    use winc_preview::markdown::Block;
    let (mut app, dir) = live_doc("md-web-table-at-end", "");
    app.clipboard.copied_html = Some(
        "<table><tr><th>名称</th><th>说明</th></tr><tr><td>茶</td><td>甲</td></tr></table>".into(),
    );
    app.paste();
    // Type before waiting for a reparse or a frame.
    app.run(Command::Insert("继续".into()));
    let expected = "| 名称 | 说明 |\n| --- | --- |\n| 茶 | 甲 |\n\n继续";
    assert_eq!(doc_text(&app), expected);
    settle(&mut app);
    let id = app.editor.active().unwrap().id;
    assert!(app.save_document(id, None));
    assert_eq!(
        std::fs::read_to_string(dir.join("LIVE.md")).unwrap(),
        expected
    );
    app.close_tab(app.editor.index_of(id).unwrap());
    App::apply_preview_state(&mut app.editor, &app.state);
    app.open_path(&dir.join("LIVE.md"));
    settle(&mut app);
    assert!(app.markdown_live());
    let parsed = app.preview.markdown.parsed.as_ref().unwrap();
    assert!(matches!(
        parsed.blocks.as_slice(),
        [Block::Table(..), Block::Paragraph(_)]
    ));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn pasting_a_web_table_restores_the_replaced_selection_on_undo() {
    use winc_core::Selection;
    use winc_editor::Command;
    let (mut app, dir) = live_doc("md-web-table-replace", "前文\n\n替换我");
    let before = Selection::new(7, 4);
    app.editor.active_mut().unwrap().selection = before;
    app.clipboard.copied_html = Some("<table><tr><th>茶</th><th>数量</th></tr></table>".into());
    app.paste();
    let expected = "前文\n\n| 茶 | 数量 |\n| --- | --- |\n\n";
    assert_eq!(doc_text(&app), expected);
    paint(&mut app); // Draw without waiting for background parsing.
    assert!(app.live_caret_shown());
    assert!(app.live_leaf().is_none_or(|leaf| !leaf.table));
    assert!(caret_screen_y(&app).is_finite());
    assert!(
        !crate::app::preview::markdown::laid::full_text(&app)
            .unwrap()
            .contains("| --- |")
    );
    app.run(Command::Undo);
    assert_eq!(doc_text(&app), "前文\n\n替换我");
    assert_eq!(app.editor.active().unwrap().selection, before);
    app.run(Command::Redo);
    assert_eq!(doc_text(&app), expected);
    settle(&mut app);
    assert!(app.live_leaf().is_none_or(|leaf| !leaf.table));
    assert!(caret_screen_y(&app).is_finite());
    app.run(Command::Insert("下文".into()));
    assert_eq!(doc_text(&app), format!("{expected}下文"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn pasting_a_web_table_places_ime_preedit_below_its_last_cell() {
    use winit::event::Ime;
    let (mut app, dir) = live_doc("md-table-ime", "");
    app.panels.sidebar.legacy_chrome = false;
    app.clipboard.copied_html = Some("<table><tr><th>名称</th><th>说明</th></tr><tr><td><strong>重点</strong></td><td>甲<br>乙</td></tr></table>".into());
    app.paste();
    paint(&mut app);
    let id = app.editor.active().unwrap().id;
    app.auto_save(&[id]);
    app.wait_for_saves();
    app.on_ime(Ime::Preedit("nihao".into(), Some((5, 5))));
    paint(&mut app);
    let laid = app.preview.markdown.layout.as_ref().unwrap();
    let caret = app.ime.cursor.expect("IME candidate position");
    let table_bottom = app.preview.markdown.area.y + laid.pad + laid.blocks[0].1.height
        - app.preview.markdown.scroll();
    assert!(
        caret.y >= table_bottom,
        "{caret:?}, table bottom {table_bottom}"
    );
    app.on_ime(Ime::Commit("你好".into()));
    assert!(doc_text(&app).ends_with("| **重点** | 甲<br>乙 |\n\n你好"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn pasting_spreadsheet_cells_leaves_a_prose_row_and_preserves_crlf() {
    let (mut app, dir) = live_doc("md-tsv-paste-crlf", "前文\r\n");
    put_caret(&mut app, "\r\n", true);
    app.clipboard.copied = Some("品名\t数量\n茶\t3".into());
    app.paste();
    app.run(winc_editor::Command::Insert("下文".into()));
    assert_eq!(
        doc_text(&app),
        "前文\r\n\r\n| 品名 | 数量 |\r\n| --- | --- |\r\n| 茶 | 3 |\r\n\r\n下文"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn pasting_pipes_or_code_does_not_add_a_table_separator() {
    let (mut app, dir) = live_doc("md-paste-pipe-prose", "");
    app.clipboard.copied_html = Some("<p>| <strong>普通</strong> |</p>".into());
    app.paste();
    assert_eq!(doc_text(&app), "| **普通** |");
    std::fs::remove_dir_all(dir).unwrap();

    let (mut app, dir) = live_doc("md-paste-tsv-code", "```\n标记\n```\n");
    put_caret(&mut app, "标记", true);
    app.clipboard.copied = Some("名\t量\n茶\t3".into());
    app.clipboard.copied_html =
        Some("<table><tr><td>名</td><td>量</td></tr><tr><td>茶</td><td>3</td></tr></table>".into());
    app.paste();
    assert_eq!(doc_text(&app), "```\n标记名\t量\n茶\t3\n```\n");
    std::fs::remove_dir_all(dir).unwrap();
}
