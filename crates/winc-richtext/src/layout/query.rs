//! 排版结果的无状态几何查询算法：表格命中、点击定位、区间选区矩形投影、光标项定位与行号采集。

use std::ops::Range;
use std::sync::Arc;

use winc_render::Rect;
use winc_typeset::{Item, LaidBlock};

use super::blanks::BlankGaps;
use super::types::{Point, is_markup, line_box};

/// 鼠标落在的显示字符和原文字节位置。空行没有显示文字，但可以定位到原文。
pub(super) struct Hit {
    pub(super) offset: Option<usize>,
    pub(super) source: Option<usize>,
}

/// 命中内容坐标 `(x, y)` 所在的表格单元格矩形（相对内容顶部与表格横向滚动偏移）。
pub(super) fn table_cell(
    blocks: &[(u64, Arc<LaidBlock>)],
    tops: &[f32],
    range: Range<usize>,
    x: f32,
    y: f32,
    shift: &impl Fn(usize, usize) -> (f32, Option<Rect>),
) -> Option<(usize, Rect)> {
    for block in range {
        let y = y - tops[block];
        for (k, rect, cols, rows) in blocks[block].1.tables() {
            let (off, strip) = shift(block, k);
            if strip.is_some_and(|s| !s.contains(x, y)) {
                continue;
            }
            let rect = Rect::new(rect.x - off, rect.y, rect.w, rect.h);
            if !rect.contains(x, y) {
                continue;
            }
            let (mut right, mut bottom) = (rect.x, rect.y);
            let c = cols.iter().position(|w| {
                right += w;
                x < right
            })?;
            let r = rows.iter().position(|h| {
                bottom += h;
                y < bottom
            })?;
            let cell = Rect::new(right - cols[c], bottom - rows[r], cols[c], rows[r]);
            return Some((block, cell));
        }
    }
    None
}

/// 在内容坐标 `(x, y)` 处命中最近的文字字素、公式/图片锚点或空行槽位。
#[allow(clippy::too_many_arguments)]
pub(super) fn hit_content(
    blocks: &[(u64, Arc<LaidBlock>)],
    tops: &[f32],
    starts: &[usize],
    ranges: &[Range<usize>],
    blanks: &BlankGaps,
    x: f32,
    y: f32,
    shift: &impl Fn(usize, usize) -> (f32, Option<Rect>),
) -> Option<Hit> {
    let mut best: Option<((f32, f32), Hit)> = None;
    let i = tops.partition_point(|&t| t <= y).saturating_sub(1);
    let visible = i.saturating_sub(1)..(i + 2).min(blocks.len());
    let table = table_cell(blocks, tops, visible.clone(), x, y, shift);
    let mut inside: Option<((f32, f32), Hit)> = None;
    for block in visible.clone() {
        let top = tops[block];
        for (k, item) in blocks[block].1.items().iter().enumerate() {
            let Item::Text {
                x: ix_raw,
                baseline,
                size,
                text,
                span: Some(span),
                ..
            } = item
            else {
                continue;
            };
            let start = starts[block] + span.start;
            let ix = *ix_raw - shift(block, k).0;
            let (y0, h) = line_box(top + *baseline, *size);
            let width: f32 = span.advances.iter().sum();
            let dy = (y0 - y).max(y - (y0 + h)).max(0.0);
            let dx = (ix - x).max(x - (ix + width)).max(0.0);
            let here = table
                .as_ref()
                .is_some_and(|(b, cell)| *b == block && cell.contains(ix, y0 + h / 2.0 - tops[*b]));
            let slot = if here { &mut inside } else { &mut best };
            if slot.as_ref().is_some_and(|(d, _)| *d <= (dy, dx)) {
                continue;
            }
            let mut pen = ix;
            let mut at = span.advances.len();
            for (ci, w) in span.advances.iter().enumerate() {
                if x < pen + w / 2.0 {
                    at = ci;
                    break;
                }
                pen += w;
            }
            let hit = Hit {
                offset: Some(start + at),
                source: ranges
                    .get(block)
                    .map(|r| r.start + span.source_of(text, at).unwrap_or(0)),
            };
            *slot = Some(((dy, dx), hit));
        }
    }
    if inside.is_some() {
        return inside.map(|(_, hit)| hit);
    }
    for block in visible {
        let top = tops[block];
        for (k, rect, (start, source)) in blocks[block].1.source_spots() {
            let off = shift(block, k).0;
            if rect.contains(x + off, y - top) {
                return Some(Hit {
                    offset: Some(starts[block] + start),
                    source: ranges.get(block).map(|r| r.start + source as usize),
                });
            }
        }
    }
    if best.as_ref().is_none_or(|((dy, _), _)| *dy > 0.0)
        && let Some(source) = blanks.hit(x, y)
    {
        return Some(Hit {
            offset: None,
            source: Some(source),
        });
    }
    best.map(|(_, hit)| hit)
}

/// 将任意原文区间 `range` 投影为内容坐标系下的正文高亮矩形列表，并追加到 `rects`。
pub(super) fn append_mark_rects(
    blocks: &[(u64, Arc<LaidBlock>)],
    tops: &[f32],
    ranges: &[Range<usize>],
    range: Range<usize>,
    shift: &impl Fn(usize, usize) -> (f32, Option<Rect>),
    rects: &mut Vec<Rect>,
) {
    for (block_idx, block_range) in ranges.iter().enumerate() {
        if block_range.end <= range.start || block_range.start >= range.end {
            continue;
        }
        let Some((_, laid)) = blocks.get(block_idx) else {
            continue;
        };
        let top = tops[block_idx];
        for (k, item) in laid.items().iter().enumerate() {
            let Item::Text {
                x: ix_raw,
                baseline,
                size,
                text,
                span: Some(span),
                ..
            } = item
            else {
                continue;
            };
            if span.source.is_none() || span.advances.is_empty() {
                continue;
            }
            let a = span.chars_before(text, range.start.saturating_sub(block_range.start));
            let b = span.chars_before(text, range.end.saturating_sub(block_range.start));
            if a >= b {
                continue;
            }
            let ix = *ix_raw - shift(block_idx, k).0;
            let x = ix + span.advances[..a].iter().sum::<f32>();
            let w = span.advances[a..b].iter().sum::<f32>();
            if w <= 0.0 {
                continue;
            }
            let (y, h) = line_box(top + *baseline, *size);
            rects.push(Rect::new(x, y, w, h));
        }
    }
}

/// 在第 `block` 块的项子区间 `items_range` 中查找块内原文偏移 `source` 的光标位置。
#[allow(clippy::too_many_arguments)]
pub(super) fn point_in_items(
    laid: &LaidBlock,
    top: f32,
    pad: f32,
    block: usize,
    items_range: Range<usize>,
    source: usize,
    upstream: bool,
    shift: &impl Fn(usize, usize) -> (f32, Option<Rect>),
) -> Option<Point> {
    let end = items_range.end.min(laid.items().len());
    let start = items_range.start.min(end);
    let spans_iter = laid.items()[start..end]
        .iter()
        .enumerate()
        .filter_map(move |(off, item)| match item {
            Item::Text {
                x,
                baseline,
                size,
                text,
                span: Some(span),
                ..
            } => Some((
                top,
                *x - shift(block, start + off).0,
                *baseline,
                *size,
                span,
                &**text,
            )),
            _ => None,
        });
    if upstream
        && let Some((top, x, baseline, size, span, text)) = spans_iter
            .clone()
            .filter(|(.., span, _)| span.source.is_some_and(|s| (s as usize) < source))
            .max_by_key(|(.., span, _)| span.source)
        && span.char_at_source(text, source) == Some(span.advances.len())
    {
        let x = x + span.advances.iter().sum::<f32>();
        let (y, h) = line_box(top + baseline, size);
        return Some((x, pad + y, h));
    }
    let (top, x, baseline, size, span, text) = spans_iter
        .filter(|(.., span, _)| span.source.is_some_and(|s| s as usize <= source))
        .max_by_key(|(.., span, _)| span.source)?;
    let k = span.char_at_source(text, source)?;
    let x = x + span.advances[..k].iter().sum::<f32>();
    let (y, h) = line_box(top + baseline, size);
    Some((x, pad + y, h))
}

/// 采集视口纵向范围 `view` 内可标注原文行号的各显示行 `(原文行号, 显示行基线)`。
pub(super) fn source_lines_in_view(
    blocks: &[(u64, Arc<LaidBlock>)],
    tops: &[f32],
    ranges: &[Range<usize>],
    rope: &ropey::Rope,
    view: Range<f32>,
    shift: &impl Fn(usize, usize) -> (f32, Option<Rect>),
) -> Vec<(usize, f32)> {
    let first = tops.partition_point(|&t| t <= view.start).saturating_sub(1);
    let last = tops.partition_point(|&t| t < view.end).min(blocks.len());
    let mut rows: Vec<(f32, f32, usize)> = Vec::new();
    for block in first..last {
        let Some(range) = ranges.get(block) else {
            continue;
        };
        let top = tops[block];
        for (k, item) in blocks[block].1.items().iter().enumerate() {
            let Item::Text {
                x,
                baseline,
                span: Some(span),
                ..
            } = item
            else {
                continue;
            };
            let Some(start) = span.source else {
                continue;
            };
            let y = top + *baseline;
            if (view.start..=view.end).contains(&y) {
                rows.push((
                    y,
                    *x - shift(block, k).0,
                    (range.start + start as usize).min(rope.len_bytes()),
                ));
            }
        }
    }
    rows.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    rows.dedup_by(|b, a| (a.0 - b.0).abs() < 0.5);
    rows.into_iter()
        .filter_map(|(y, _, from)| {
            let from = rope.char_to_byte(rope.byte_to_char(from));
            let line = rope.byte_to_line(from);
            let prefix = rope.byte_slice(rope.line_to_byte(line)..from);
            prefix.chars().all(is_markup).then_some((line, y))
        })
        .collect()
}
