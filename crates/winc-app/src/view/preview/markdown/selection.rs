//! Editable Markdown selection backgrounds, measured in displayed rows.

use winc_render::{Color, Frame, Rect};
use winc_typeset::Item;

use super::{Laid, line_box};

struct Row {
    baseline: f32,
    top: f32,
    bottom: f32,
    end: usize,
    selected: Option<(f32, f32)>,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw(
    frame: &mut Frame<'_>,
    laid: &Laid,
    block: usize,
    shown: &[u32],
    body: Rect,
    clip: Rect,
    (from, to): (usize, usize),
    color: Color,
) {
    let Some(range) = laid.block_range(block) else {
        return;
    };
    if range.end < from || range.start >= to {
        return;
    }
    let Some(items) = laid.block_at(block).map(winc_typeset::LaidBlock::items) else {
        return;
    };
    let mut spans: Vec<_> = shown
        .iter()
        .filter_map(|&i| match &items[i as usize] {
            Item::Text {
                x,
                baseline,
                size,
                text,
                span: Some(span),
                ..
            } if span.source.is_some() && !span.advances.is_empty() => {
                Some((*baseline, *x, *size, text, span))
            }
            _ => None,
        })
        .collect();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));

    let mut rows: Vec<Row> = Vec::new();
    for (baseline, x, size, text, span) in spans {
        let (top, height) = line_box(body.y + baseline, size);
        let end = range.start
            + span
                .source_of(text, span.advances.len())
                .unwrap_or_default();
        if rows
            .last()
            .is_none_or(|row| (row.baseline - baseline).abs() >= 0.5)
        {
            rows.push(Row {
                baseline,
                top,
                bottom: top + height,
                end,
                selected: None,
            });
        }
        let row = rows.last_mut().unwrap();
        row.top = row.top.min(top);
        row.bottom = row.bottom.max(top + height);
        row.end = row.end.max(end);

        let a = span.chars_before(text, from.saturating_sub(range.start));
        let b = span.chars_before(text, to.saturating_sub(range.start));
        if a < b {
            let left = body.x + x + span.advances[..a].iter().sum::<f32>();
            let right = left + span.advances[a..b].iter().sum::<f32>();
            row.selected = Some(
                row.selected
                    .map_or((left, right), |(l, r)| (l.min(left), r.max(right))),
            );
        }
    }

    let point = |offset| {
        (range.start..=range.end)
            .contains(&offset)
            .then(|| laid.caret_point_with_affinity(offset, true))
            .flatten()
            .map(|(x, y, _)| (body.x + x, body.y + laid.content_y(y) - laid.top(block)))
    };
    let start = point(from);
    let end_top = point(to).map(|(_, top)| top);
    for (i, row) in rows.iter().enumerate() {
        let continues = to > row.end && end_top.is_none_or(|top| top > row.top + 0.5);
        let selected = row.selected.or_else(|| {
            let (x, top) = start?;
            (continues && (top - row.top).abs() < 0.5).then_some((x, x))
        });
        let Some((left, mut right)) = selected else {
            continue;
        };
        // The previous block carries the selection to this block's top edge.
        let top = if i == 0 && block > 0 && from < range.start {
            row.top.min(body.y)
        } else {
            row.top
        };
        let mut bottom = row.bottom;
        // A continued selection includes the rest of the display line and its leading.
        if continues {
            right = right.max(body.right());
            if let Some(next) = rows.get(i + 1).filter(|row| row.selected.is_some()) {
                bottom = bottom.max(next.top);
            } else if to > range.end
                && let Some(next) = laid.tops().get(block + 1)
            {
                bottom = bottom.max(body.y + next - laid.top(block));
            }
        }
        frame.rect_clipped(
            Rect::new(left, top, right - left, bottom - top),
            color,
            0.0,
            clip,
        );
    }
}
