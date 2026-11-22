//! Draw the local and full-library note graphs using the same canvas and controls.

use super::*;

use crate::app::workspace::note_links::graph::Target;
use winc_i18n::t;
use winc_render::Stroke;

pub(crate) fn icon(frame: &mut Frame<'_>, (x, y): (f32, f32), color: Color, clip: Rect) {
    let a = (x - 4.0, y - 4.0);
    let b = (x - 3.0, y + 4.0);
    let c = (x + 5.0, y + 1.0);
    frame.stroke(
        &[
            Stroke::Line(a.0, a.1, c.0, c.1),
            Stroke::Line(b.0, b.1, c.0, c.1),
        ],
        1.0,
        color,
        clip,
    );
    for (x, y) in [a, b, c] {
        frame.rect_clipped(Rect::new(x - 1.8, y - 1.8, 3.6, 3.6), color, 2.0, clip);
    }
}

pub(in crate::view) fn draw(app: &mut App, frame: &mut Frame<'_>) {
    app.ensure_note_links();
    let area = app.layout.editor;
    let p = app.palette;
    frame.rect(area, p.editor_bg);
    let mouse = app.mouse();
    let size = app.fonts.ui_small;
    let index = app.note_links.index.as_ref();
    let view = &mut app.note_links.graph;
    let bar = Rect::new(area.x, area.y, area.w, 46.0f32.min(area.h));
    let body = Rect::new(
        area.x,
        bar.bottom(),
        area.w,
        (area.bottom() - bar.bottom()).max(0.0),
    );
    view.area = body;
    let mut hits = std::mem::take(&mut view.hits);
    hits.clear();
    frame.rect_clipped(
        Rect::new(bar.x, bar.bottom() - 1.0, bar.w, 1.0),
        p.border,
        0.0,
        bar,
    );
    let mut x = bar.x + 12.0;
    for (text, target, active) in [
        (t!("workspace-note-graph-search"), Target::Search, false),
        (t!("workspace-note-graph-fit"), Target::Fit, false),
        (
            t!("workspace-note-graph-isolated"),
            Target::Isolated,
            view.isolated,
        ),
    ] {
        if target == Target::Isolated && view.center.is_some() {
            continue;
        }
        let w = frame.measure(FontKind::Ui, size, text) + 20.0;
        let r = Rect::new(x, bar.y + 8.0, w, 30.0).intersect(bar);
        if r.w < w {
            break;
        }
        if active || r.contains(mouse.0, mouse.1) {
            frame.rect_clipped(r, p.hover, 5.0, bar);
        }
        frame.text(
            FontKind::Ui,
            size,
            r.x + 10.0,
            r.y + 19.0,
            text,
            if active { p.accent } else { p.text },
            r,
        );
        hits.push((r, target));
        x = r.right() + 4.0;
    }
    if let Some(center) = &view.center {
        let room = (bar.right() - x - 16.0).max(0.0);
        if room > 48.0 {
            let title = center.file_stem().unwrap_or_default().to_string_lossy();
            let title = crate::view::ellipsize(frame, &title, size, room);
            let width = frame.measure(FontKind::Ui, size, &title);
            frame.text(
                FontKind::Ui,
                size,
                bar.right() - 16.0 - width,
                bar.y + 27.0,
                &title,
                p.text_muted,
                bar,
            );
        }
    }
    if let Some(index) = index {
        if view.fit {
            view.fit(index);
        }
        let graph = view.graph(index);
        let selected = view
            .selected
            .as_ref()
            .and_then(|path| graph.paths.binary_search(path).ok());
        let related =
            |i| selected.is_some_and(|s| i == s || graph.neighbors[s].binary_search(&i).is_ok());
        let show =
            |i: usize| view.isolated || view.center.is_some() || !graph.neighbors[i].is_empty();
        if !graph.points.iter().enumerate().any(|(i, _)| show(i)) {
            message(
                frame,
                body,
                size,
                t!("workspace-note-graph-empty"),
                p.text_muted,
            );
        }
        for &(a, b) in &graph.edges {
            let ((x0, y0), (x1, y1)) = (
                view.position(graph.points[a]),
                view.position(graph.points[b]),
            );
            if (x0.max(x1) < body.x || x0.min(x1) > body.right())
                || (y0.max(y1) < body.y || y0.min(y1) > body.bottom())
            {
                continue;
            }
            let lit = selected.is_some_and(|s| a == s || b == s);
            let color = if lit {
                p.accent.alpha(0.7)
            } else {
                p.text_muted
                    .alpha(if selected.is_some() { 0.12 } else { 0.22 })
            };
            frame.line((x0, y0), (x1, y1), if lit { 1.4 } else { 1.0 }, color, body);
            if lit && view.zoom >= 0.4 {
                let (dx, dy) = (x1 - x0, y1 - y0);
                let length = dx.hypot(dy);
                if length > 20.0 {
                    let (ux, uy) = (dx / length, dy / length);
                    let tip = (x0 + dx * 0.65, y0 + dy * 0.65);
                    let base = (tip.0 - ux * 6.0, tip.1 - uy * 6.0);
                    frame.polygon(
                        &[
                            tip,
                            (base.0 - uy * 3.0, base.1 + ux * 3.0),
                            (base.0 + uy * 3.0, base.1 - ux * 3.0),
                        ],
                        color,
                        body,
                    );
                }
            }
        }
        let mut labels = Vec::new();
        for (i, &point) in graph.points.iter().enumerate() {
            if !show(i) {
                continue;
            }
            let (x, y) = view.position(point);
            let r = Rect::new(x - 8.0, y - 8.0, 16.0, 16.0).intersect(body);
            if r.w <= 0.0 || r.h <= 0.0 {
                continue;
            }
            let hot = r.contains(mouse.0, mouse.1);
            let active = selected == Some(i);
            let radius = if active {
                6.0
            } else {
                (3.0 + graph.neighbors[i].len() as f32 * 0.14).min(5.0)
            };
            let color = if active || hot || related(i) {
                p.accent
            } else {
                p.text_muted
            };
            if active {
                frame.rect_clipped(
                    Rect::new(x - 9.0, y - 9.0, 18.0, 18.0),
                    p.accent.alpha(0.19),
                    9.0,
                    body,
                );
            }
            frame.rect_clipped(
                Rect::new(x - radius, y - radius, radius * 2.0, radius * 2.0),
                color,
                radius,
                body,
            );
            hits.push((r, Target::Node(i)));
            if view.zoom >= 0.76 || active || hot || (related(i) && view.zoom >= 0.40) {
                labels.push((active || hot, i, x, y, color));
            }
        }
        labels.sort_by_key(|&(important, ..)| !important);
        let mut occupied: Vec<Rect> = Vec::new();
        for (important, i, x, y, color) in labels {
            let title = graph.paths[i]
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy();
            let text =
                crate::view::ellipsize(frame, &title, size, if important { 256.0 } else { 122.0 });
            let width = frame.measure(FontKind::Ui, size, &text);
            let label = Rect::new(x - width / 2.0 - 4.0, y + 11.0, width + 8.0, size + 5.0);
            if !important
                && occupied.iter().any(|r| {
                    let overlap = label.intersect(*r);
                    overlap.w > 0.0 && overlap.h > 0.0
                })
            {
                continue;
            }
            occupied.push(label);
            frame.rect_clipped(label, p.editor_bg.alpha(0.92), 4.0, body);
            frame.text(
                FontKind::Ui,
                size,
                label.x + 4.0,
                label.y + size,
                &text,
                color,
                body,
            );
        }
    } else {
        message(
            frame,
            body,
            size,
            t!("workspace-note-links-loading"),
            p.text_muted,
        );
    }
    view.hits = hits;
}

fn message(frame: &mut Frame<'_>, area: Rect, size: f32, text: &str, color: Color) {
    let w = frame.measure(FontKind::Ui, size, text);
    frame.text(
        FontKind::Ui,
        size,
        area.x + (area.w - w) / 2.0,
        area.y + area.h / 2.0,
        text,
        color,
        area,
    );
}
