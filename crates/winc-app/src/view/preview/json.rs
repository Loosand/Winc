//! JSON 结构图的原生绘制：把属性放在卡片上，从容器行连到其子卡片。

use std::borrow::Cow;
use winc_i18n::t;
use winc_preview::json::{Error, Row, Value};
use winc_render::{Stroke, rounded_outline};
use winc_syntax::TokenKind;

use crate::app::preview::json::{JsonView, Layout, ROW_H, Target};
use crate::view::{App, Color, FontKind, Frame, Hover, Palette, Rect, ellipsize};

const TEXT: f32 = 13.0;
const PADDING: f32 = 11.0;
const TOGGLE: f32 = 16.0;
const TOOLBAR: f32 = 38.0;

pub(in crate::view) fn draw(app: &mut App, frame: &mut Frame<'_>, area: Rect) {
    let (p, size, hover) = (app.palette, app.fonts.ui_small, app.pointer.hover);
    let view = &mut app.preview.json;
    view.area = area;
    frame.rect(area, p.editor_bg);
    let error = |error: &Error| match *error {
        Error::Syntax { line, column } => t!("preview-json-syntax", line = line, column = column),
        Error::Limit => t!("preview-json-limit").to_owned(),
    };
    let Some(graph) = &view.graph else {
        let (message, color) = match &view.error {
            Some(reason) => (error(reason), p.error),
            None => (t!("preview-json-loading").to_owned(), p.text_muted),
        };
        message_at(frame, area, size, &message, color);
        return;
    };
    let bar = Rect::new(area.x, area.y, area.w, TOOLBAR.min(area.h));
    let footer = view.error.as_ref().map(|reason| {
        let h = 30.0_f32.min((area.h - bar.h).max(0.0));
        let rect = Rect::new(area.x, area.bottom() - h, area.w, h);
        message_at(frame, rect, size, &error(reason), p.error);
        rect
    });
    let body = Rect::new(
        area.x,
        bar.bottom(),
        area.w,
        (area.h - bar.h - footer.map_or(0.0, |r| r.h)).max(0.0),
    );
    view.body = body;
    if body.w <= 0.0 || body.h <= 0.0 {
        return;
    }
    if view.layout.is_none() {
        view.layout = Some(Layout::new(graph, &view.collapsed, |rows| {
            card_width(frame, rows)
        }));
    }
    view.place();
    grid(frame, view, p);

    let (graph, layout) = (view.graph.as_ref().unwrap(), view.layout.as_ref().unwrap());
    let mut hits = std::mem::take(&mut view.hits);
    for (rows, rect) in graph.iter().zip(&layout.cards) {
        let Some(rect) = rect else { continue };
        for (i, row) in rows.iter().enumerate() {
            if let Some(to) = row.child.and_then(|id| layout.cards[id]) {
                let out = view.position((rect.right(), rect.y + (i as f32 + 0.5) * ROW_H));
                let end = view.position((to.x, to.y + to.h / 2.0));
                connect(frame, body, out, end, p.text_muted.alpha(0.43));
            }
        }
    }
    for (i, (rect, rows)) in layout.cards.iter().zip(graph).enumerate() {
        let Some(rect) = rect else { continue };
        let (x, y) = view.position((rect.x, rect.y));
        let card = Rect::new(x, y, rect.w * view.zoom, rect.h * view.zoom);
        if overlaps(card, body) {
            draw_card(frame, view, &mut hits, i, card, rows, p, hover);
        }
    }
    toolbar(frame, bar, view.zoom, size, &mut hits, p, hover);
    view.hits = hits;
}

fn grid(frame: &mut Frame<'_>, view: &JsonView, p: &Palette) {
    let clip = view.body;
    let (x0, y0) = view.position((0.0, 0.0));
    let step = (28.0 * view.zoom).clamp(19.0, 56.0);
    let color = p.text_muted.alpha(if p.dark { 0.095 } else { 0.11 });
    let mut x = clip.x + (x0 - clip.x).rem_euclid(step);
    while x < clip.right() {
        frame.rect_clipped(Rect::new(x, clip.y, 1.0, clip.h), color, 0.0, clip);
        x += step;
    }
    let mut y = clip.y + (y0 - clip.y).rem_euclid(step);
    while y < clip.bottom() {
        frame.rect_clipped(Rect::new(clip.x, y, clip.w, 1.0), color, 0.0, clip);
        y += step;
    }
}

fn overlaps(a: Rect, b: Rect) -> bool {
    let rect = a.intersect(b);
    rect.w > 0.0 && rect.h > 0.0
}

fn connect(
    frame: &mut Frame<'_>,
    clip: Rect,
    (x0, y0): (f32, f32),
    (x1, y1): (f32, f32),
    color: Color,
) {
    if x0 > clip.right() || x1 < clip.x || y0.max(y1) < clip.y || y0.min(y1) > clip.bottom() {
        return;
    }
    // 三次曲线，两端切线沿水平方向；整条描边一次混合，分段处不会叠出深色点。
    let mid = (x0 + x1) / 2.0;
    let mut prev = (x0, y0);
    let segments: [Stroke; 12] = std::array::from_fn(|i| {
        let t = (i + 1) as f32 / 12.0;
        let s = 1.0 - t;
        let next = (
            s * s * s * x0 + 3.0 * s * t * mid + t * t * t * x1,
            y0 + (y1 - y0) * t * t * (3.0 - 2.0 * t),
        );
        let line = Stroke::Line(prev.0, prev.1, next.0, next.1);
        prev = next;
        line
    });
    frame.stroke(&segments, 1.1, color, clip);
}

fn value_text(value: &Value) -> (Cow<'_, str>, Option<TokenKind>) {
    match value {
        Value::Object(0) => ("{}".into(), None),
        Value::Array(0) => ("[]".into(), None),
        Value::Object(count) => (format!("{{{count}}}").into(), None),
        Value::Array(count) => (format!("[{count}]").into(), None),
        Value::String(text) => (text.as_str().into(), Some(TokenKind::String)),
        Value::Number(text) => (text.as_str().into(), Some(TokenKind::Number)),
        Value::Literal(text) => ((*text).into(), Some(TokenKind::Builtin)),
    }
}

fn card_width(frame: &mut Frame<'_>, rows: &[Row]) -> f32 {
    rows.iter().fold(86.0, |width, row| {
        let (value, _) = value_text(&row.value);
        let key = if row.key.is_empty() {
            0.0
        } else {
            frame.measure(FontKind::Mono, TEXT, &row.key)
                + frame.measure(FontKind::Mono, TEXT, ": ")
        };
        let control = if row.child.is_some() { TOGGLE } else { 0.0 };
        width.max(2.0 * PADDING + control + key + frame.measure(FontKind::Mono, TEXT, &value))
    })
}

#[allow(clippy::too_many_arguments)]
fn draw_card(
    frame: &mut Frame<'_>,
    view: &JsonView,
    hits: &mut Vec<(Rect, Target)>,
    id: usize,
    card: Rect,
    rows: &[Row],
    p: &Palette,
    hover: Hover,
) {
    let z = view.zoom;
    let clip = view.body;
    let radius = 6.0 * z;
    frame.rect_clipped(card, p.group_bg, radius, clip);
    if z < 0.29 {
        return;
    }
    let size = TEXT * z;
    let h = ROW_H * z;
    for (i, row) in rows.iter().enumerate() {
        let y = card.y + i as f32 * h;
        let r = Rect::new(card.x, y, card.w, h);
        let row_clip = r.intersect(clip);
        if row_clip.w <= 0.0 || row_clip.h <= 0.0 {
            continue;
        }
        if i > 0 {
            frame.rect_clipped(
                Rect::new(r.x, y, r.w, z.min(1.0)),
                p.border.alpha(0.62),
                0.0,
                clip,
            );
        }
        let mut x = card.x + PADDING * z;
        let baseline = y + (ROW_H / 2.0 + 4.0) * z;
        if let Some(child) = row.child {
            let hit = Target::Toggle { card: id, row: i };
            if hover == Hover::Json(hit) {
                frame.rect_clipped(r.inset(z, z), p.hover, radius * 0.7, clip);
            }
            hits.push((row_clip, hit));
            let button = Rect::new(x - z, y + 7.0 * z, 12.0 * z, 12.0 * z);
            frame.rect_clipped(button, p.hover, 3.0 * z, clip);
            let cx = button.x + button.w / 2.0;
            let cy = button.y + button.h / 2.0;
            let extent = 2.8 * z;
            let color = if view.collapsed.contains(&child) {
                p.accent
            } else {
                p.text_muted
            };
            frame.line((cx - extent, cy), (cx + extent, cy), 1.0, color, clip);
            if view.collapsed.contains(&child) {
                frame.line((cx, cy - extent), (cx, cy + extent), 1.0, color, clip);
            }
            x += TOGGLE * z;
        }
        if size >= 6.5 {
            if !row.key.is_empty() {
                x = frame.text(
                    FontKind::Mono,
                    size,
                    x,
                    baseline,
                    &row.key,
                    p.token(TokenKind::Property),
                    row_clip,
                );
                x = frame.text(
                    FontKind::Mono,
                    size,
                    x,
                    baseline,
                    ": ",
                    p.text_muted,
                    row_clip,
                );
            }
            let (value, kind) = value_text(&row.value);
            let color = kind.map_or(p.text_muted, |kind| p.token(kind));
            frame.text(FontKind::Mono, size, x, baseline, &value, color, row_clip);
        }
    }
    frame.stroke(
        &rounded_outline(card.x, card.y, card.w, card.h, radius),
        1.0,
        p.border,
        clip,
    );
}

fn toolbar(
    frame: &mut Frame<'_>,
    bar: Rect,
    zoom: f32,
    size: f32,
    hits: &mut Vec<(Rect, Target)>,
    p: &Palette,
    hover: Hover,
) {
    frame.rect_clipped(
        Rect::new(bar.x, bar.bottom() - 1.0, bar.w, 1.0),
        p.border,
        0.0,
        bar,
    );
    let percent = format!("{:.0}%", 100.0 * zoom);
    let fit = t!("preview-json-fit");
    let mut left = bar.x + 7.0;
    for (text, width, action) in [
        ("−", 26.0, Some(Target::ZoomOut)),
        (percent.as_str(), 38.0, None),
        ("+", 26.0, Some(Target::ZoomIn)),
        (
            fit,
            frame.measure(FontKind::Ui, size, fit) + 18.0,
            Some(Target::Fit),
        ),
    ] {
        let button = Rect::new(left, bar.y + 5.0, width, bar.h - 10.0);
        left = button.right() + 2.0;
        if button.right() > bar.right() {
            break;
        }
        if let Some(action) = action {
            if hover == Hover::Json(action) {
                frame.rect_clipped(button, p.hover, 5.0, bar);
            }
            hits.push((button, action));
        }
        message_at(
            frame,
            button,
            size,
            text,
            if action.is_some() {
                p.text
            } else {
                p.text_muted
            },
        );
    }
}

fn message_at(frame: &mut Frame<'_>, area: Rect, size: f32, text: &str, color: Color) {
    let text = ellipsize(frame, text, size, (area.w - 8.0).max(0.0));
    let w = frame.measure(FontKind::Ui, size, &text);
    frame.text(
        FontKind::Ui,
        size,
        area.x + (area.w - w) / 2.0,
        area.y + (area.h + size * 0.62) / 2.0,
        &text,
        color,
        area,
    );
}
