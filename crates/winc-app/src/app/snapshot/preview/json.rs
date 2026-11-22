//! JSON 源代码、分栏、只看预览，以及结构图的交互和后台更新。

use super::*;
use crate::app::preview::json::Target;
use crate::layout::preview_segments;
use winc_editor::{Command, PreviewKind};
use winc_preview::json::{Error, Value};

fn center(rect: Rect) -> (f32, f32) {
    (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0)
}

fn json_paint(app: &mut App) {
    app.advance_previews();
    app.wait_for_json();
    paint(app);
}

fn replace_json(app: &mut App, text: &str) {
    let end = app
        .editor
        .active()
        .unwrap()
        .text()
        .rope()
        .unwrap()
        .len_chars();
    app.editor.active_mut().unwrap().selection = winc_core::Selection::new(0, end);
    app.run(Command::Insert(text.into()));
}

fn press(app: &mut App, target: Target) {
    let (rect, _) = app
        .preview
        .json
        .hits
        .iter()
        .find(|(_, hit)| *hit == target)
        .unwrap();
    click(app, center(*rect));
    paint(app);
}

#[test]
fn json_modes_and_live_edits_share_the_same_source() {
    let dir = sample_folder("json-modes");
    let path = dir.join("data.JSON");
    let original = r#"{"items":[{"name":"Apple","count":3}],"ok":true}"#;
    std::fs::write(&path, original).unwrap();
    let mut app = app_with(&path, true);
    paint(&mut app);
    assert_eq!(
        app.editor.active().unwrap().preview_kind(),
        Some(PreviewKind::Json)
    );
    assert_eq!(
        app.editor.active().unwrap().preview_mode(),
        PreviewMode::Source
    );
    assert!(app.preview.json.graph.is_none());

    let split = preview_segments(app.layout.preview)[1];
    click(&mut app, center(split));
    json_paint(&mut app);
    assert!(app.editor.active().unwrap().edits_text());
    assert_eq!(app.preview.json.area, app.layout.split);
    assert_eq!(
        app.state.preview_modes.get("json").map(String::as_str),
        Some("split")
    );
    assert!(!app.state.preview_modes.contains_key("svg"));
    assert_eq!(app.preview.json.graph.as_ref().unwrap().len(), 3);
    shoot(&mut app, "json-graph-split");

    replace_json(&mut app, "{\n  \"oops\": }");
    app.advance_previews();
    app.wait_for_json();
    assert!(matches!(
        app.preview.json.error,
        Some(Error::Syntax { line: 2, .. })
    ));
    assert_eq!(app.preview.json.graph.as_ref().unwrap().len(), 3);

    let changed = r#"{"items":["pear","plum"],"ok":false}"#;
    replace_json(&mut app, changed);
    json_paint(&mut app);
    assert!(app.preview.json.error.is_none());
    let graph = app.preview.json.graph.as_ref().unwrap();
    assert_eq!(graph[1][0].value, Value::String("\"pear\"".into()));
    assert_eq!(graph[0][1].value, Value::Literal("false"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);

    let preview = preview_segments(app.layout.preview)[2];
    click(&mut app, center(preview));
    json_paint(&mut app);
    let doc = app.editor.active().unwrap();
    assert_eq!(doc.preview_mode(), PreviewMode::Preview);
    assert!(!doc.shows_text() && !doc.edits_text() && doc.readonly && doc.text_editable());
    assert_eq!(app.preview.json.area, app.layout.editor);
    let source = preview_segments(app.layout.preview)[0];
    click(&mut app, center(source));
    json_paint(&mut app);
    assert!(app.preview.json.graph.is_none() && app.preview.json.area.w == 0.0);
    let doc = app.editor.active().unwrap();
    assert_eq!(doc.preview_mode(), PreviewMode::Source);
    assert!(doc.shows_text() && !doc.readonly && doc.is_dirty());
    assert_eq!(doc_text(&app), changed);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn json_graph_can_fold_pan_zoom_and_fit() {
    let dir = sample_folder("json-graph-controls");
    let path = dir.join("tree.json");
    std::fs::write(
        &path,
        r#"{"plan":{"stage":{"status":"done"}},"owner":"Lin"}"#,
    )
    .unwrap();
    let mut app = app_with(&path, false);
    app.set_preview_mode(PreviewMode::Preview);
    json_paint(&mut app);
    let fit = (app.preview.json.zoom, app.preview.json.pan);
    let row = Target::Toggle { card: 0, row: 0 };
    let row_y = |app: &App| {
        app.preview
            .json
            .hits
            .iter()
            .find(|(_, hit)| *hit == row)
            .unwrap()
            .0
            .y
    };
    let before = row_y(&app);
    press(&mut app, row);
    assert!(app.preview.json.collapsed.contains(&1));
    assert!((row_y(&app) - before).abs() < 0.01);
    assert!(app.preview.json.layout.as_ref().unwrap().cards[2].is_none());
    press(&mut app, row);
    assert!(app.preview.json.collapsed.is_empty());
    assert!(app.preview.json.layout.as_ref().unwrap().cards[2].is_some());

    let old = app.preview.json.zoom;
    press(&mut app, Target::ZoomIn);
    assert!(app.preview.json.zoom > old);
    let body = app.preview.json.body;
    let pan = app.preview.json.pan;
    let (x, y) = (body.x + 8.0, body.y + 8.0);
    app.on_mouse_move(x, y);
    app.on_mouse_down(MouseButton::Left);
    assert_eq!(app.pointer.drag, Drag::Json { at: (x, y) });
    app.on_mouse_move(x + 17.0, y + 24.0);
    app.on_mouse_up(MouseButton::Left);
    assert_eq!(app.preview.json.pan, (pan.0 + 17.0, pan.1 + 24.0));
    app.pointer.modifiers = command_modifiers();
    let zoom = app.preview.json.zoom;
    app.on_wheel_event(
        MouseScrollDelta::LineDelta(0.0, 1.0),
        winit::event::TouchPhase::Moved,
    );
    assert!(app.preview.json.zoom > zoom);
    app.pointer.modifiers = Default::default();
    let pan = app.preview.json.pan.1;
    app.on_wheel(MouseScrollDelta::PixelDelta(PhysicalPosition::new(
        0.0, -48.0,
    )));
    assert!(app.preview.json.pan.1 < pan);
    press(&mut app, Target::Fit);
    assert_eq!((app.preview.json.zoom, app.preview.json.pan), fit);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn json_workers_do_not_publish_stale_versions_to_other_tabs() {
    let dir = sample_folder("json-graph-revisions");
    let (a, b) = (dir.join("a.json"), dir.join("b.json"));
    std::fs::write(&a, r#"{"one":1}"#).unwrap();
    std::fs::write(&b, "{").unwrap();
    let mut app = app_with(&a, false);
    app.choose_preview_mode(PreviewMode::Split);
    app.advance_previews();
    // 无论后台完成的具体时间，收结果之前已经切到了 B。
    app.open_path(&b);
    json_paint(&mut app);
    assert!(app.preview.json.graph.is_none());
    assert!(matches!(app.preview.json.error, Some(Error::Syntax { .. })));
    replace_json(&mut app, "true");
    app.advance_previews();
    replace_json(&mut app, "false");
    json_paint(&mut app);
    assert_eq!(
        app.preview.json.graph.as_ref().unwrap()[0][0].value,
        Value::Literal("false")
    );
    assert!(app.preview.json.error.is_none());
    std::fs::remove_dir_all(dir).unwrap();
}
