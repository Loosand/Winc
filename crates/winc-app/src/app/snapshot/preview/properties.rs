//! Markdown 属性面板和原文保存。

use super::*;

#[test]
fn notebook_properties_panel_edits_one_field_and_saves_portable_yaml() {
    use crate::app::input::context_menu::{MenuCommand, MenuTarget};
    use winc_preview::markdown::properties::{Kind, PROPERTY};
    let source = "---\nkind: 文章 # 原注释\nauthor:\n  - Dan Abramov\ntags:\n  - 技术\ncomplex: {a: 2}\n---\n\n正文 **保留**\n";
    let (mut app, dir) = live_doc("md-properties-form", source);
    let root = app.library_root().unwrap().to_path_buf();
    std::fs::create_dir_all(root.join(".obsidian")).unwrap();
    std::fs::write(
        root.join(".obsidian/types.json"),
        r#"{"types":{"kind":"select"}}"#,
    )
    .unwrap();
    app.property_types_changed(false, &[root.join(".obsidian/types.json")]);
    let hit = app
        .hits
        .markdown_links
        .iter()
        .find(|(_, target)| target == &format!("{PROPERTY}type:0"))
        .unwrap()
        .0;
    super::click(&mut app, (hit.x + hit.w * 0.5, hit.y + hit.h * 0.5));
    assert!(matches!(
        app.overlays.context_menu.as_ref().map(|m| &m.target),
        Some(MenuTarget::NoteProperty(menu)) if menu.index == Some(0)
    ));
    app.run_menu_command(MenuCommand::EditProperty);
    let dialog = app.overlays.form_dialog.as_mut().unwrap();
    assert!(
        matches!(&dialog.kind, crate::app::git::form::FormKind::Property(edit) if edit.kind == Kind::Select)
    );
    dialog.name.set_text("教程");
    dialog.second.set_text("文章，教程，工具");
    app.submit_form_dialog();
    assert!(app.overlays.form_dialog.is_none());
    let expected = source.replace("kind: 文章", "kind: \"教程\"");
    assert_eq!(doc_text(&app), expected);
    app.run(winc_editor::Command::Undo);
    assert_eq!(doc_text(&app), source);
    app.run(winc_editor::Command::Redo);
    let id = app.editor.active().unwrap().id;
    assert!(app.save_document(id, None));
    assert_eq!(
        std::fs::read_to_string(dir.join("LIVE.md")).unwrap(),
        expected
    );
    let spec = app
        .state
        .note_property_schema
        .get(root.to_str().unwrap())
        .unwrap()
        .as_object()
        .unwrap()["kind"]
        .as_object()
        .unwrap();
    assert_eq!(spec["kind"].as_str(), Some("select"));
    assert_eq!(spec["options"].as_array().unwrap().len(), 3);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn property_values_open_anchored_editors_and_calendar_keeps_iso_yaml() {
    use crate::app::git::branch::picker::PickerPart;
    use crate::app::preview::markdown::properties::popup::CalendarPart;
    use winc_preview::markdown::properties::{Kind, PROPERTY};

    let source = "---\nkind: 文章 # 注释\ntags: [学习]\ndate: 2026-11-05\nstarred:\n---\n\n正文\n";
    let (mut app, dir) = live_doc("md-properties-popover", source);
    let root = app.library_root().unwrap().to_owned();
    app.state.note_property_schema.insert(
        root.to_string_lossy().into_owned(),
        winc_config::Json::parse(r#"{"kind":{"kind":"select","options":["文章","工具","随笔"]},"starred":{"kind":"checkbox"}}"#)
            .unwrap(),
    );
    let obsidian = root.join(".obsidian");
    std::fs::create_dir_all(&obsidian).unwrap();
    std::fs::write(
        obsidian.join("types.json"),
        r#"{"types":{"kind":"select","tags":"multiselect"}}"#,
    )
    .unwrap();
    app.property_types_changed(false, &[obsidian.join("types.json")]);
    let center = |r: winc_render::Rect| (r.x + r.w * 0.5, r.y + r.h * 0.5);
    let value = |app: &App, index| {
        app.hits
            .markdown_links
            .iter()
            .find(|(_, link)| link == &format!("{PROPERTY}{index}"))
            .unwrap()
            .0
    };
    let click_part = |app: &mut App, part| {
        let rect = app
            .hits
            .picker_parts
            .iter()
            .find(|(_, p)| *p == part)
            .unwrap()
            .0;
        super::click(app, center(rect));
    };
    settle(&mut app);
    let hit = value(&app, 0);
    super::click(&mut app, center(hit));
    assert!(app.overlays.context_menu.is_none());
    assert_eq!(
        app.overlays.picker.property.as_ref().unwrap().kind(),
        Kind::Select
    );
    assert_eq!(app.layout.picker.x, hit.x);
    assert!(app.layout.picker.y >= hit.bottom());
    let screenshots = std::env::var_os("WINC_SNAPSHOT_DIR").is_some();
    if screenshots {
        super::shoot(&mut app, "minc-property-choice");
    }
    app.edit_field(|f| f.insert("教程"));
    assert_eq!(app.overlays.picker.hits.len(), 1);
    app.picker_accept();
    assert!(!app.overlays.picker.open);
    assert!(doc_text(&app).contains("kind: \"教程\" # 注释"));
    settle(&mut app);

    let hit = value(&app, 1);
    super::click(&mut app, center(hit));
    assert_eq!(
        app.overlays.picker.property.as_ref().unwrap().kind(),
        Kind::MultiSelect
    );
    app.edit_field(|f| f.insert("技术"));
    app.picker_accept();
    assert!(app.overlays.picker.open);
    assert!(doc_text(&app).contains("tags: [\"学习\", \"技术\"]"));
    app.picker_accept();
    assert!(app.overlays.picker.open);
    assert!(doc_text(&app).contains("tags: [\"技术\"]"));
    app.close_picker();
    settle(&mut app);

    let hit = value(&app, 2);
    super::click(&mut app, center(hit));
    settle(&mut app);
    assert_eq!(
        app.overlays.picker.property.as_ref().unwrap().month,
        (2026, 11)
    );
    if screenshots {
        super::shoot(&mut app, "minc-property-calendar");
    }
    click_part(&mut app, PickerPart::PropertyCalendar(CalendarPart::Next));
    settle(&mut app);
    assert_eq!(
        app.overlays.picker.property.as_ref().unwrap().month,
        (2026, 12)
    );
    click_part(&mut app, PickerPart::PropertyCalendar(CalendarPart::Day(2)));
    assert!(!app.overlays.picker.open);
    assert!(doc_text(&app).contains("date: \"2026-12-02\""));
    settle(&mut app);

    let hit = value(&app, 3);
    super::click(&mut app, center(hit));
    assert!(!app.overlays.picker.open);
    assert!(doc_text(&app).contains("starred: true"));
    app.run(winc_editor::Command::Undo);
    assert!(doc_text(&app).contains("starred:\n---\n\n正文\n"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn property_panel_edits_inline_and_switches_between_fields_and_yaml() {
    use winc_preview::markdown::properties::{COLLAPSE, FIELDS, PROPERTY, RAW, SOURCE};
    let source =
        "---\nkind: 文章 # keep\ntags:\n- 技术\n- 性能\nstarred: false\nrating: 5\n---\n\n正文\n";
    let (mut app, dir) = live_doc("md-property-inline", source);
    let click = |app: &mut App, target: &str| {
        let r = app
            .hits
            .markdown_links
            .iter()
            .find(|(_, url)| url == target)
            .unwrap()
            .0;
        super::click(app, (r.x + r.w / 2.0, r.y + r.h / 2.0));
    };
    click(&mut app, COLLAPSE);
    settle(&mut app);
    assert!(!app.hits.markdown_links.iter().any(|(_, url)| url == FIELDS));
    click(&mut app, COLLAPSE);
    settle(&mut app);
    if std::env::var_os("WINC_SNAPSHOT_DIR").is_some() {
        super::shoot(&mut app, "minc-property-fields");
    }

    click(&mut app, &format!("{PROPERTY}0"));
    settle(&mut app);
    if std::env::var_os("WINC_SNAPSHOT_DIR").is_some() {
        super::shoot(&mut app, "minc-property-inline");
    }
    let cell = app
        .hits
        .markdown_links
        .iter()
        .find(|(_, url)| url == &format!("{PROPERTY}0"))
        .unwrap()
        .0;
    assert_eq!(app.layout.picker, cell);
    app.edit_field(|field| {
        field.set_text("教程");
        true
    });
    // The same click commits the text and activates the name field.
    click(&mut app, &format!("{PROPERTY}name:0"));
    assert!(doc_text(&app).contains("kind: \"教程\" # keep"));
    app.edit_field(|field| {
        field.set_text("type");
        true
    });
    app.picker_accept();
    settle(&mut app);
    assert!(doc_text(&app).contains("\"type\": \"教程\" # keep"));

    click(&mut app, &format!("{PROPERTY}list:1"));
    app.edit_field(|field| field.insert("Web, UI"));
    app.picker_accept();
    settle(&mut app);
    assert!(doc_text(&app).contains("tags: [\"技术\", \"性能\", \"Web, UI\"]"));
    click(&mut app, &format!("{PROPERTY}item:1:0"));
    settle(&mut app);
    assert!(doc_text(&app).contains("tags: [\"性能\", \"Web, UI\"]"));
    click(&mut app, &format!("{PROPERTY}delete:2"));
    settle(&mut app);
    assert!(!doc_text(&app).contains("starred"));
    app.run(winc_editor::Command::Undo);
    settle(&mut app);
    assert!(doc_text(&app).contains("starred: false"));

    click(&mut app, SOURCE);
    settle(&mut app);
    assert!(app.hits.markdown_links.iter().any(|(_, url)| url == RAW));
    assert!(app.overlays.picker.input.is_multiline());
    assert!(app.overlays.picker.input.text().starts_with("\"type\":"));
    app.picker_accept();
    settle(&mut app);
    assert!(!app.overlays.picker.open);
    if std::env::var_os("WINC_SNAPSHOT_DIR").is_some() {
        super::shoot(&mut app, "minc-property-source-idle");
    }
    click(&mut app, RAW);
    assert!(app.overlays.picker.input.is_multiline());
    app.edit_field(|field| {
        field.set_text("tags: [");
        true
    });
    if std::env::var_os("WINC_SNAPSHOT_DIR").is_some() {
        super::shoot(&mut app, "minc-property-source");
    }
    click(&mut app, FIELDS);
    assert!(app.overlays.picker.input.is_multiline());
    assert!(doc_text(&app).contains("starred: false"));
    app.edit_field(|field| {
        field.set_text("# raw\ntitle: 新标题\ntags: [技术]");
        true
    });
    click(&mut app, FIELDS);
    settle(&mut app);
    assert!(!app.overlays.picker.open);
    assert_eq!(
        doc_text(&app),
        "---\n# raw\ntitle: 新标题\ntags: [技术]\n---\n\n正文\n"
    );
    assert!(
        app.hits
            .markdown_links
            .iter()
            .any(|(_, url)| url == &format!("{PROPERTY}name:0"))
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn property_list_reserves_inline_editing_space_in_a_narrow_note() {
    use winc_preview::markdown::properties::{ADD, FIELDS, PROPERTY, SOURCE};
    let dir = sample_folder("md-property-narrow");
    let path = dir.join("LIVE.md");
    std::fs::write(
        &path,
        "---\ntitle: 写作备忘\ndate: 2026-02-17\nstarred: true\ntags: [一个很长的属性标签用于检查窄窗口中添加按钮会不会挤到边缘]\nrating: 5\n---\n\n正文\n",
    )
    .unwrap();
    let mut app = app_with(&path, true);
    app.frame.renderer = Some(
        winc_render::Renderer::offscreen(
            700.0,
            620.0,
            super::SCALE,
            app.palette.frame_bg,
            None,
            None,
        )
        .unwrap(),
    );
    paint(&mut app);
    app.set_preview_mode(PreviewMode::Preview);
    settle(&mut app);
    let hit = |app: &App, target: &str| {
        app.hits
            .markdown_links
            .iter()
            .find(|(_, url)| url == target)
            .unwrap()
            .0
    };
    let (add, fields, source) = (hit(&app, ADD), hit(&app, FIELDS), hit(&app, SOURCE));
    assert!(add.right() <= fields.x && fields.right() <= source.x);
    let plus = hit(&app, &format!("{PROPERTY}list:3"));
    let chip = hit(&app, &format!("{PROPERTY}item:3:0"));
    assert!(plus.y >= chip.bottom());

    super::click(&mut app, (plus.x + plus.w * 0.5, plus.y + plus.h * 0.5));
    settle(&mut app);
    assert!(app.overlays.picker.input.view.area.w >= 96.0);
    app.on_mouse_move(plus.x + plus.w * 0.5, plus.y + plus.h * 0.5);
    assert_eq!(app.pointer.cursor_icon, winit::window::CursorIcon::Text);
    if std::env::var_os("WINC_SNAPSHOT_DIR").is_some() {
        super::shoot(&mut app, "minc-property-narrow-dark");
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn adding_properties_to_a_note_without_yaml_does_not_change_the_body() {
    use crate::app::input::context_menu::MenuCommand;
    use winc_preview::markdown::properties::Kind;
    let (mut app, dir) = live_doc("md-properties-add", "正文\n");
    let doc = app.editor.active().unwrap();
    app.run_property_menu(
        doc.id,
        doc.revision,
        None,
        MenuCommand::PropertyType(Kind::Number),
    );
    let dialog = app.overlays.form_dialog.as_mut().unwrap();
    dialog.name.set_text("评分");
    dialog.second.set_text("4.5");
    app.submit_form_dialog();
    assert_eq!(doc_text(&app), "---\n\"评分\": 4.5\n---\n\n正文\n");
    settle(&mut app);
    assert!(
        app.preview
            .markdown
            .parsed
            .as_ref()
            .unwrap()
            .properties
            .is_some()
    );
    let _ = std::fs::remove_dir_all(dir);
}
