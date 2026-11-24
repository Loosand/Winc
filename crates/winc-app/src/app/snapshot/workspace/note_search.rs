//! Offscreen regressions for the notebook search dialog and its opening animation.

use super::*;
use crate::app::PickerPart;

#[test]
fn notebook_search_dialog_does_not_leave_a_gap_in_the_scrim_during_opening() {
    use crate::app::animation::{Clock, DIALOG_IN};

    let (mut app, root) = note_app("minc-search-scrim");
    paint(&mut app);
    app.open_notebook_search();
    paint(&mut app);
    assert!(app.overlays.picker.notebook.is_some());

    let card = app.layout.picker;
    let pixels = |app: &App| {
        let image = app.frame.renderer.as_ref().unwrap().image().unwrap();
        let x = ((card.x + card.w / 2.0) * SCALE) as usize;
        // Both samples are above the shadow, on opposite sides of the animation's backdrop snapshot.
        [20.0, 30.0].map(|gap| {
            let y = ((card.y - gap) * SCALE) as usize;
            image.pixels[y * image.width as usize + x]
        })
    };
    let settled = pixels(&app);
    assert_eq!(settled[0], settled[1]);

    app.overlays.picker.card.motion = Some((true, Clock::frozen(DIALOG_IN / 3, DIALOG_IN)));
    paint(&mut app);
    let opening = pixels(&app);

    app.close_picker();
    paint(&mut app);
    let background = pixels(&app);
    let _ = std::fs::remove_dir_all(root.parent().unwrap());
    assert_eq!(
        opening[0], opening[1],
        "the scrim should dim both pixels equally"
    );
    assert_ne!(
        opening, background,
        "the scrim should be visible during opening"
    );
}

#[test]
fn notebook_search_dialog_matches_titles_and_body_and_preview_does_not_open_a_result() {
    let (mut app, root) = note_app("minc-search");
    let tea = root.join("Recipes/茶.md");
    app.open_path(&tea);
    app.state.recent_note_edits = vec![tea.clone()];
    paint(&mut app);
    app.open_notebook_search();
    paint(&mut app);
    assert_eq!(app.panels.sidebar.view, SidebarView::Files);
    assert_eq!(app.focus, Focus::Picker);
    // Old recency data no longer filters or ranks notebook search results.
    assert_eq!(app.overlays.picker.recent, 0);
    assert_eq!(app.overlays.picker.hits.len(), 2);
    app.overlays.picker.input.set_text("薄荷");
    app.picker_refresh();
    paint(&mut app);
    let results = |app: &App| -> Vec<String> {
        let p = &app.overlays.picker;
        let notes = &p.notebook.as_ref().unwrap().index.notes;
        p.hits.iter().map(|&i| notes[i].title.clone()).collect()
    };
    assert_eq!(results(&app), ["薄荷", "茶"]);
    let card = app.layout.picker;
    let parts = crate::app::workspace::note_search::search_parts(card, true);
    let preview = crate::app::workspace::note_search::preview_rect(card, parts);
    press(
        &mut app,
        Rect::new(preview.x + 10.0, parts.list.y, 20.0, parts.row_height),
    );
    assert!(app.overlays.picker.open);
    let titles = app
        .hits
        .picker_parts
        .iter()
        .find(|(_, part)| *part == PickerPart::NoteTitles)
        .unwrap()
        .0;
    press(&mut app, titles);
    assert_eq!(results(&app), ["薄荷"]);
    app.picker_accept();
    assert!(!app.overlays.picker.open);
    assert_eq!(
        app.editor.active().unwrap().path.as_deref(),
        Some(root.join("薄荷.md").as_path())
    );
    let _ = std::fs::remove_dir_all(root.parent().unwrap());
}
