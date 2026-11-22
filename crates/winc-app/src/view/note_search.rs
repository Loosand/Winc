//! The centered notebook search dialog, with a result list and a quiet reading preview.

use winc_i18n::t;

use super::*;
use crate::app::workspace::library::{PreviewLine, preview_lines};
use crate::app::workspace::note_search::{Purpose, preview_rect, search_parts};
use crate::app::{Focus, PickerPart};

/// Draw the scrim before the animation captures its backdrop, so restoring it cannot erase a band of dimming.
pub(super) fn draw_scrim(app: &App, frame: &mut Frame<'_>) {
    let fade = match app.overlays.picker.card.motion {
        Some((true, clock)) => dialog_curve(true, clock).0,
        _ => 1.0,
    };
    let scrim = app.palette.scrim;
    frame.rect(app.layout.window, scrim.alpha(scrim.a * 0.65 * fade));
}

pub(super) fn draw(app: &mut App, frame: &mut Frame<'_>) {
    let card = app.layout.picker;
    let p = app.palette;
    let Some(search) = &app.overlays.picker.notebook else {
        return;
    };
    let parts = search_parts(card, search.preview);
    let bar = parts.bar;
    draw_card(frame, p, card, 14.0, p.popup_bg);
    let x = bar.x + 24.0;
    icons::magnifier(frame, (x, center(bar).1), p.text_muted, card);
    let close = Rect::new(bar.right() - 38.0, bar.y + 10.0, 28.0, 28.0);
    let preview = Rect::new(close.x - 34.0, close.y, close.w, close.h);
    let icon_button = |frame: &mut Frame<'_>, r, part, active| {
        draw_button(
            frame,
            p,
            r,
            card,
            app.pointer.hover == Hover::PickerPart(part),
            active,
        );
    };
    icon_button(frame, close, PickerPart::CloseNoteSearch, false);
    icons::close(frame, center(close), 9.0, p.text_muted, card);
    let mut hits = vec![(close, PickerPart::CloseNoteSearch)];
    if card.w >= 580.0 && !search.title_picker() {
        icon_button(frame, preview, PickerPart::NotePreview, search.preview);
        icons::list_view(
            frame,
            center(preview),
            if search.preview {
                p.accent
            } else {
                p.text_muted
            },
            card,
        );
        hits.push((preview, PickerPart::NotePreview));
    }
    let field_x = x + 18.0;
    let field_right = if search.title_picker() {
        close.x
    } else {
        preview.x
    };
    let input = Rect::new(
        field_x,
        bar.y,
        (field_right - 8.0 - field_x).max(0.0),
        bar.h,
    );
    let placeholder = match search.purpose {
        Purpose::FocusGraph => t!("workspace-note-graph-search"),
        Purpose::FollowLink(_) => t!("workspace-note-link-choose"),
        Purpose::Open => t!("workspace-notebook-search-placeholder"),
    };
    let focused = app.focus == Focus::Picker;
    let mut field = std::mem::take(&mut app.overlays.picker.input);
    app.ime.cursor = draw_bar_field(app, frame, input, &mut field, placeholder, focused);
    app.overlays.picker.input = field;

    let picker = &app.overlays.picker;
    let search = picker.notebook.as_ref().unwrap();
    if !search.title_picker() {
        let title_only = t!("workspace-notebook-titles-only");
        let toggle_width = frame.measure(FontKind::Ui, app.fonts.ui_small, title_only) + 47.0;
        let toggle = Rect::new(card.x + 15.0, parts.actions.y + 4.0, toggle_width, 27.0);
        draw_button(
            frame,
            p,
            toggle,
            card,
            app.pointer.hover == Hover::PickerPart(PickerPart::NoteTitles),
            search.titles_only,
        );
        ui_text_small(frame, app, toggle.x + 10.0, toggle, "Aa", p.text_dim, card);
        ui_text_small(
            frame,
            app,
            toggle.x + 32.0,
            toggle,
            title_only,
            if search.titles_only {
                p.accent
            } else {
                p.text_muted
            },
            card,
        );
        hits.push((toggle, PickerPart::NoteTitles));
    }

    let list = parts.list;
    let heading = Rect::new(list.x + 10.0, list.y - 25.0, list.w - 20.0, 24.0);
    let group = if picker.input.text().trim().is_empty() {
        t!("workspace-library-notes")
    } else {
        t!("workspace-notebook-search-results")
    };
    ui_text_small(frame, app, heading.x, heading, group, p.text_muted, heading);
    let selected = app.notebook_search_selection();
    let scroll = drawn_scroll(picker.scroll);
    for i in parts.visible(scroll, picker.hits.len()) {
        let note = &search.index.notes[picker.hits[i]];
        let row = parts.row(i, scroll);
        let cy = center(row).1;
        if selected == i {
            frame.rect_clipped(row.inset(1.0, 1.0), p.hover, 7.0, list);
        }
        icons::document(
            frame,
            (row.x + 10.0, cy - 6.5),
            (11.0, 14.0),
            p.text_muted,
            list,
        );
        let x = row.x + 32.0;
        let room = (row.right() - x - 10.0).max(0.0);
        let name_room = if note.folder.is_empty() {
            room
        } else {
            room * 0.73
        };
        let name = ellipsize(frame, &note.title, app.fonts.ui, name_room);
        let path_x = x + frame.measure(FontKind::Ui, app.fonts.ui, &name) + 9.0;
        let path = ellipsize(
            frame,
            &note.folder,
            app.fonts.ui_small,
            (row.right() - path_x - 10.0).max(0.0),
        );
        ui_text(frame, app, x, row, &name, p.text, list);
        ui_text_small(frame, app, path_x, row, &path, p.text_dim, list);
    }
    if picker.hits.is_empty() {
        let row = Rect::new(list.x + 10.0, list.y, list.w - 20.0, 36.0);
        let message = if search.loading() {
            t!("workspace-picker-indexing")
        } else {
            t!("workspace-notebook-search-empty")
        };
        ui_text(frame, app, row.x, row, message, p.text_dim, list);
    }
    if search.preview && card.w >= 580.0 {
        let rect = preview_rect(card, parts);
        frame.rect_clipped(rect, p.border, 11.0, card);
        frame.rect_clipped(rect.inset(1.0, 1.0), p.popup_bg, 10.0, card);
        if let Some(note) = picker
            .hits
            .get(selected)
            .and_then(|&i| search.index.notes.get(i))
        {
            draw_preview(app, frame, rect, note);
        }
    }
    // Draw the footer after the content so the dialog always has a stable baseline.
    let footer = parts.footer;
    frame.rect_clipped(
        Rect::new(footer.x + 1.0, footer.y, footer.w - 2.0, 1.0),
        p.border,
        0.0,
        card,
    );
    let hint = if search.index.limited {
        t!("workspace-notebook-search-limited")
    } else if matches!(search.purpose, Purpose::FocusGraph) {
        t!("workspace-note-graph-search-keys")
    } else {
        t!("workspace-notebook-search-keys")
    };
    let hint = ellipsize(frame, hint, app.fonts.ui_small, (footer.w - 28.0).max(0.0));
    ui_text_small(
        frame,
        app,
        footer.x + 17.0,
        footer,
        &hint,
        p.text_dim,
        footer,
    );
    app.hits.picker_parts = hits;
}

fn wrap<'a>(frame: &mut Frame<'_>, size: f32, text: &'a str, width: f32) -> Vec<&'a str> {
    crate::layout::wrap_text(text, width, |s| frame.measure(FontKind::Ui, size, s))
}

fn draw_preview(
    app: &App,
    frame: &mut Frame<'_>,
    rect: Rect,
    note: &crate::app::workspace::note_search::SearchNote,
) {
    let p = app.palette;
    let clip = rect.inset(1.0, 1.0);
    let band = Rect::new(clip.x + 3.0, clip.y + 3.0, (clip.w - 6.0).max(0.0), 42.0);
    frame.rect_clipped(band, p.hover, 7.0, clip);
    let x = rect.x + 20.0;
    let mut y = band.bottom() + 17.0;
    let width = (rect.w - 40.0).max(0.0);
    if width <= 0.0 {
        return;
    }
    if !note.folder.is_empty() {
        let row = Rect::new(x, y, width, 22.0);
        let folder = ellipsize(frame, &note.folder, app.fonts.ui_small, width);
        ui_text_small(frame, app, x, row, &folder, p.text_dim, clip);
        y += row.h;
    }
    let size = app.fonts.ui * 1.42;
    for line in wrap(frame, size, &note.title, width).into_iter().take(3) {
        let row = Rect::new(x, y, width, size * 1.35);
        label(frame, app, size, (x, row), line, p.text, clip);
        y += row.h;
    }
    y += 10.0;
    let lines = preview_lines(&note.preview);
    for (i, line) in lines.into_iter().enumerate() {
        let (text, heading) = match &line {
            PreviewLine::Heading(s) if i == 0 && s == &note.title => continue,
            PreviewLine::Heading(s) => (s.clone(), true),
            PreviewLine::Text(s) | PreviewLine::Code(s) => (s.clone(), false),
            PreviewLine::List(s) => (format!("•  {s}"), false),
            PreviewLine::Quote(s) => (format!("│ {s}"), false),
            PreviewLine::Rule => {
                frame.rect_clipped(Rect::new(x, y + 6.0, width, 1.0), p.border, 0.0, clip);
                y += 13.0;
                continue;
            }
        };
        let size = if heading {
            app.fonts.ui
        } else {
            app.fonts.ui_small
        };
        for line in wrap(frame, size, &text, width) {
            let row = Rect::new(x, y, width, size * 1.45);
            if row.bottom() > clip.bottom() - 10.0 {
                return;
            }
            label(
                frame,
                app,
                size,
                (x, row),
                line,
                if heading { p.text } else { p.text_muted },
                clip,
            );
            y += row.h;
        }
        y += if heading { 5.0 } else { 3.0 };
    }
}
