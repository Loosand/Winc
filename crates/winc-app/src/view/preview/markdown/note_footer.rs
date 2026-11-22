//! Draw backlinks after the last paragraph without adding anything to the document.

use super::*;
use crate::app::workspace::note_links::footer::{Action, CONTROL};

const GAP: f32 = 32.0;
const HEADER: f32 = 64.0;
const ROW: f32 = 28.0;

pub(super) fn height(app: &App) -> f32 {
    let Some(path) = app.editor.active().and_then(|doc| doc.path.as_deref()) else {
        return 0.0;
    };
    if !app.note_in_library(path) {
        return 0.0;
    }
    let Some(groups) = app
        .note_links
        .index
        .as_ref()
        .and_then(|index| index.backlinks(path))
    else {
        return 0.0;
    };
    let expanded = app.note_links.expanded.get(path);
    let rows: usize = groups
        .iter()
        .map(|(from, refs)| {
            if expanded.is_some_and(|open| open.contains(from)) {
                refs.len() + 1
            } else {
                2
            }
        })
        .sum();
    GAP + HEADER + rows as f32 * ROW
}

pub(super) fn draw(app: &mut App, frame: &mut Frame<'_>, at: Rect, clip: Rect) {
    let visible = at.intersect(clip);
    if visible.w <= 0.0 || visible.h <= 0.0 {
        return;
    }
    let Some(path) = app.editor.active().and_then(|doc| doc.path.clone()) else {
        return;
    };
    let index = app.note_links.index.clone();
    let Some(groups) = index.as_ref().and_then(|index| index.backlinks(&path)) else {
        return;
    };
    let p = app.palette;
    let small = app.fonts.ui_small;
    let y = at.y + GAP;
    frame.rect_clipped(Rect::new(at.x, y, at.w, 1.0), p.border, 0.0, clip);
    let heading = t!("workspace-note-backlinks");
    frame.text(
        FontKind::Ui,
        app.fonts.ui,
        at.x,
        y + 24.0,
        heading,
        p.text,
        clip,
    );
    let graph = t!("workspace-note-view-graph");
    let button_w = frame.measure(FontKind::Ui, small, graph) + 20.0;
    let graph_at = Rect::new(at.right() - button_w, y + 8.0, button_w, ROW);
    let graph_color = if graph_at.contains(app.mouse().0, app.mouse().1) {
        p.accent
    } else {
        p.text_muted
    };
    frame.text(
        FontKind::Ui,
        small,
        graph_at.x + 10.0,
        y + 24.0,
        graph,
        graph_color,
        clip,
    );
    hit(app, graph_at.intersect(clip), Action::Graph);
    let summary = t!(
        "workspace-note-backlink-count",
        notes = groups.len(),
        links = groups.values().map(Vec::len).sum::<usize>()
    );
    frame.text(
        FontKind::Ui,
        small,
        at.x,
        y + 47.0,
        &summary,
        p.text_muted,
        clip,
    );
    let mut y = y + HEADER;
    for (from, refs) in groups {
        let expanded = app
            .note_links
            .expanded
            .get(&path)
            .is_some_and(|open| open.contains(from));
        let shown = if expanded { refs.len() } else { 1 };
        let group_h = (1 + shown) as f32 * ROW;
        if y >= clip.bottom() {
            break;
        }
        if y + group_h <= clip.y {
            y += group_h;
            continue;
        }
        let row = Rect::new(at.x, y, at.w, ROW);
        let name = from.file_stem().unwrap_or_default().to_string_lossy();
        let name = crate::view::ellipsize(frame, &name, app.fonts.ui, (row.w - 64.0).max(0.0));
        frame.text(
            FontKind::Ui,
            app.fonts.ui,
            row.x,
            row.y + 18.0,
            &name,
            p.text,
            clip,
        );
        let more_w = if refs.len() > 1 { 44.0 } else { 0.0 };
        hit(
            app,
            Rect::new(row.x, row.y, (row.w - more_w).max(0.0), row.h).intersect(clip),
            Action::Open(from.clone()),
        );
        if refs.len() > 1 {
            let badge = format!("{} {}", if expanded { "▾" } else { "▸" }, refs.len());
            let more = Rect::new(row.right() - more_w, row.y, more_w, ROW);
            frame.text(
                FontKind::Ui,
                small,
                more.x + 8.0,
                more.y + 18.0,
                &badge,
                p.text_muted,
                clip,
            );
            hit(app, more.intersect(clip), Action::Expand(from.clone()));
        }
        y += ROW;
        for citation in &refs[..shown] {
            if y >= clip.bottom() {
                return;
            }
            let row = Rect::new(at.x, y, at.w, ROW).intersect(clip);
            if row.w > 0.0 && row.h > 0.0 {
                let hot = row.contains(app.mouse().0, app.mouse().1);
                if hot {
                    frame.rect_clipped(row, p.hover, 4.0, clip);
                }
                let chunks = [
                    (&citation.before, p.text_muted),
                    (&citation.label, p.accent),
                    (&citation.after, p.text_muted),
                ];
                let mut x = row.x + 12.0;
                let right = row.right() - 10.0;
                for (i, (text, color)) in chunks.into_iter().enumerate() {
                    let room = (right - x).max(0.0);
                    let room = if i == 0 { room * 0.38 } else { room };
                    let text = crate::view::ellipsize(frame, text, small, room);
                    frame.text(FontKind::Ui, small, x, y + 18.0, &text, color, row);
                    x += frame.measure(FontKind::Ui, small, &text);
                }
                hit(app, row, Action::Cite(from.clone(), citation.clone()));
            }
            y += ROW;
        }
    }
}

/// Scroll after the opened document has been laid out, even if its caret had no previous frame.
pub(super) fn follow_citation(app: &mut App) {
    let Some((id, revision, byte)) = app.note_links.citation_jump.take() else {
        return;
    };
    if !app
        .editor
        .active()
        .is_some_and(|doc| (doc.id, doc.revision) == (id, revision))
    {
        return;
    }
    let md = &mut app.preview.markdown;
    if let Some(layout) = &md.layout
        && let Some(top) = layout.caret_point(byte).map(|(_, top, _)| top).or_else(|| {
            let i = layout
                .ranges
                .partition_point(|range| range.start <= byte)
                .checked_sub(1)?;
            Some(layout.tops[i] + layout.pad)
        })
    {
        md.scroll_to(top - md.area.h / 3.0);
    }
}

fn hit(app: &mut App, rect: Rect, action: Action) {
    if rect.w > 0.0 && rect.h > 0.0 {
        let i = app.note_links.footer_hits.len();
        app.note_links.footer_hits.push(action);
        app.hits
            .markdown_links
            .push((rect, format!("{CONTROL}{i}")));
    }
}
