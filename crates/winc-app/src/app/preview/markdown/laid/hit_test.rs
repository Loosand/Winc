//! 窗口坐标到 Markdown 源码或显示字符的命中测试。

use super::{App, Item, Laid, Rect, line_box};

/// 预览上窗口坐标 `(x, y)`（逻辑像素）处是整份文字里的第几个字符（落在两个字中间时取
/// 近的那边）；不在字上时取同一行最近的字，不在任何一行上时取上下最近的一行。
pub(crate) fn offset_at(app: &App, x: f32, y: f32) -> Option<usize> {
    hit(app, x, y)?.offset
}

/// 窗口横坐标 `x` 是否落在预览的内容一栏内（左右各放宽 [`CONTENT_SLACK`]）。
///
/// 内容一栏两侧的空白不对应任何文字：在那里按下鼠标不放光标、不开始选字。
pub(crate) fn in_content(app: &App, x: f32) -> bool {
    app.preview.markdown.layout.as_ref().is_none_or(|laid| {
        let left = laid.left - CONTENT_SLACK;
        let right = laid.left + laid.key.width + CONTENT_SLACK;
        (left..=right).contains(&x)
    })
}

/// 内容一栏两侧仍按点在文字上处理的宽度（逻辑像素，经验值）：点在行首稍左处仍能把光标放到
/// 行首。
const CONTENT_SLACK: f32 = 12.0;

/// 预览上窗口坐标 `(x, y)` 处的字（取法同 [`offset_at`]）在原文中的位置（字节偏移）。
///
/// 由原文生成、没有对应原文的文字（提示块的标题）取所在顶层块的开头。
pub(crate) fn source_at(app: &App, x: f32, y: f32) -> Option<usize> {
    hit(app, x, y)?.source
}

/// 把窗口坐标 `(x, y)` 处可横向滚动的表格横向滚动 `dx`（逻辑像素，向右滚动为负），返回是否
/// 滚动了；该处没有可滚动的表格时返回 `false`，由调用方按竖直方向滚动预览。
pub(crate) fn scroll_table(app: &mut App, x: f32, y: f32, dx: f32) -> bool {
    let (area, scroll) = (app.preview.markdown.area, app.preview.markdown.scroll());
    let Some(laid) = app.preview.markdown.layout.as_mut() else {
        return false;
    };
    let (cx, cy) = (x - laid.left, y - (area.y + laid.pad - scroll));
    let Some((block, s, max)) = laid.scroller_at(cx, cy).filter(|&(.., max)| max > 0.0) else {
        return false;
    };
    let now = laid.hscroll_of(block, s);
    laid.hscroll.insert((block, s), (now - dx).clamp(0.0, max));
    app.preview.markdown.last_table = Some((block, s));
    app.request_redraw();
    true
}

/// 鼠标落在的显示字符和原文字节位置。空行没有显示文字，但可以定位到原文。
struct Hit {
    offset: Option<usize>,
    source: Option<usize>,
}

/// 返回鼠标位置（在块 `blocks` 中，内容坐标）下的表格单元格：所在的块及单元格范围（块内坐标）。
fn table_cell(
    laid: &Laid,
    blocks: std::ops::Range<usize>,
    x: f32,
    y: f32,
) -> Option<(usize, Rect)> {
    for block in blocks {
        let y = y - laid.tops[block];
        for (k, item) in laid.blocks[block].1.items.iter().enumerate() {
            let Item::Table {
                rect, cols, rows, ..
            } = item
            else {
                continue;
            };
            // 可横向滚动的表格：只认显示区域内的点，单元格按滚动位置左移。
            let (off, strip) = laid.shift(block, k);
            if strip.is_some_and(|s| !s.contains(x, y)) {
                continue;
            }
            let rect = &Rect::new(rect.x - off, rect.y, rect.w, rect.h);
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

fn hit(app: &App, x: f32, y: f32) -> Option<Hit> {
    let laid = app.preview.markdown.layout.as_ref()?;
    let origin = app.preview.markdown.area.y + laid.pad - app.preview.markdown.scroll();
    let (x, y) = (x - laid.left, y - origin);
    let mut best: Option<((f32, f32), Hit)> = None;
    // 只看鼠标所在的块和上下各一块。
    let i = laid.tops.partition_point(|&t| t <= y).saturating_sub(1);
    let blocks = i.saturating_sub(1)..(i + 2).min(laid.blocks.len());
    // 点击表格单元格时只在该单元格内查找文字，使光标不会进入相邻单元格；
    // 空单元格中有一段空的原文，可供定位。
    let table = table_cell(laid, blocks.clone(), x, y);
    let mut inside: Option<((f32, f32), Hit)> = None;
    for (top, start, ix, baseline, size, span, block, text) in laid.spans(blocks.clone()) {
        let (y0, h) = line_box(top + baseline, size);
        let width: f32 = span.advances.iter().sum();
        let dy = (y0 - y).max(y - (y0 + h)).max(0.0);
        let dx = (ix - x).max(x - (ix + width)).max(0.0);
        let here = table.as_ref().is_some_and(|(b, cell)| {
            *b == block && cell.contains(ix, y0 + h / 2.0 - laid.tops[*b])
        });
        let slot = if here { &mut inside } else { &mut best };
        if slot.as_ref().is_some_and(|(d, _)| *d <= (dy, dx)) {
            continue;
        }
        // 找离 `x` 最近的字缝。
        let mut pen = ix;
        let mut at = span.advances.len();
        for (k, w) in span.advances.iter().enumerate() {
            if x < pen + w / 2.0 {
                at = k;
                break;
            }
            pen += w;
        }
        let hit = Hit {
            offset: Some(start + at),
            source: laid
                .ranges
                .get(block)
                .map(|r| r.start + span.source_of(text, at).unwrap_or(0)),
        };
        *slot = Some(((dy, dx), hit));
    }
    if inside.is_some() {
        return inside.map(|(_, hit)| hit);
    }
    // 点击图片时光标移到图片的原文处，因为图片中没有可定位的文字。
    for block in blocks {
        let top = laid.tops[block];
        for (k, item) in laid.blocks[block].1.items.iter().enumerate() {
            let off = laid.shift(block, k).0;
            let spot = match item {
                Item::Image {
                    rect, at: Some(at), ..
                }
                | Item::Spot { rect, at } => Some((rect, at)),
                _ => None,
            };
            if let Some((rect, (start, source))) = spot
                && rect.contains(x + off, y - top)
            {
                return Some(Hit {
                    offset: Some(laid.starts[block] + start),
                    source: laid.ranges.get(block).map(|r| r.start + *source as usize),
                });
            }
        }
    }
    // Real text and visual objects win at their edges; otherwise an explicit blank row
    // takes precedence over snapping the click to the closest paragraph.
    if best.as_ref().is_none_or(|((dy, _), _)| *dy > 0.0)
        && let Some(source) = laid.blank_at(y)
    {
        return Some(Hit {
            offset: None,
            source: Some(source),
        });
    }
    best.map(|(_, hit)| hit)
}
