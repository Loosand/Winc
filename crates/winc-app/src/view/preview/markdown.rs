//! Markdown 预览的排版和绘制，字号与间距都按逻辑像素算。
//!
//! 正文是编辑器字号的 16/13 倍。段距、行高和标题层级由 `winc-typeset` 统一；
//! 排版要量字宽，所以在绘制
//! 时进行，结果按顶层块缓存（[`Laid`]）：文字改了只重排变了的块，宽度、字号、配色或图片大小
//! 变了才整份重排。换行时不在「，。」这类标点前面、「（“」这类标点后面断。

use std::sync::Arc;

use winc_i18n::t;
use winc_preview::markdown::properties::{PROPERTY, RAW};
use winc_render::{Color, FontKind, Frame, Rect, Stroke};

use crate::app::preview::markdown::properties::popup::PropertyInput;
use crate::app::preview::markdown::resize::ImageAt;
use crate::app::preview::markdown::{ImageSlot, TableBar};
use crate::app::{App, Focus, ScrollArea};
use crate::icons;
use crate::theme::Palette;

mod caret;
mod line_numbers;
mod note_footer;
mod outline;
mod resize;
mod selection;
mod table;
mod toolbar;
mod viewer;

use caret::draw_live_caret;
use line_numbers::{draw_source_numbers, number_room};
use outline::{
    TOC_FLASH_RADIUS, TOC_PINNED_BODY, TOC_PINNED_COLUMN, TOC_PINNED_EDGE, TOC_ROOM, draw_toc,
    has_outline,
};
use table::{draw_table_fades, draw_table_scrollbar, table_toolbar};

pub(in crate::view) use viewer::draw_diagram_view;
use winc_typeset::Item;

use crate::app::preview::markdown::laid::*;
use crate::layout::drawn_scroll;

/// 以 `(left, top)` 为块的左上角绘制 `items` 中的底色和文字，裁剪到 `clip`。
fn draw_plain(frame: &mut Frame<'_>, items: &[Item], (left, top): (f32, f32), clip: Rect) {
    let at = |r: &Rect| Rect::new(left + r.x, top + r.y, r.w, r.h);
    for item in items {
        match item {
            Item::Fill {
                rect,
                color,
                radius,
            } => frame.rect_clipped(at(rect), *color, *radius, clip),
            Item::Corners { rect, color, radii } => {
                frame.rect_corners(at(rect), *color, *radii, clip);
            }
            Item::Text {
                x,
                baseline,
                text,
                font,
                faux,
                size,
                color,
                ..
            } => {
                frame.text_faux(
                    *font,
                    *faux,
                    *size,
                    left + x,
                    top + baseline,
                    text,
                    *color,
                    clip,
                );
            }
            _ => {}
        }
    }
}

/// 在指向不存在的文件或标题的链接下方绘制错误色的虚线。
fn draw_broken_links(app: &mut App, frame: &mut Frame<'_>, area: Rect) {
    let color = app.palette.error.alpha(0.7);
    // 检查链接需要可变借用 `app`，检查期间暂时移出本帧的链接。
    let links = std::mem::take(&mut app.hits.markdown_links);
    for (rect, url) in &links {
        // 预览控件的地址以 `\u{1}` 开头，不检查。
        if url.starts_with('\u{1}') || app.check_markdown_link(url).is_none() {
            continue;
        }
        let y = (rect.bottom() - 1.5).round();
        let mut x = rect.x;
        while x < rect.right() {
            let w = 2.0f32.min(rect.right() - x);
            frame.rect_clipped(Rect::new(x, y, w, 1.0), color, 0.0, area);
            x += 4.0;
        }
    }
    app.hits.markdown_links = links;
}

/// 在 `area`（窗口坐标，逻辑像素）里画当前文档的 Markdown 预览，分栏时是右半边，只看预览时
/// 是整个编辑区。
///
/// 排版结果和光标位置写回 `app.preview.markdown`，可点击范围记入 `app.hits`；需要读取的图片记进请求，下一帧再画。
pub(in crate::view) fn draw(app: &mut App, frame: &mut Frame<'_>, area: Rect) {
    let p = app.palette;
    frame.rect(area, p.editor_bg);
    app.preview.markdown.area = area;
    if app
        .editor
        .active()
        .and_then(|doc| doc.path.as_deref())
        .is_some_and(|path| app.note_in_library(path))
    {
        app.ensure_note_links();
    }
    // 可编辑的预览在排版之前取得当前文字的解析结果，本帧即按最新的结构显示。
    app.parse_markdown_inline();
    // 第一次解析还没回来：先留空。
    let Some(parsed) = app.preview.markdown.parsed.clone() else {
        return;
    };
    let pad = if area.w >= 600.0 { PAD } else { PAD_NARROW };
    let widest = match app.config.markdown_text_width.unwrap_or_default() {
        winc_config::TextWidth::Standard => MAX_WIDTH,
        winc_config::TextWidth::Wide => WIDE_WIDTH,
        winc_config::TextWidth::Full => f32::MAX,
    };
    let width = (area.w - pad * 2.0).clamp(1.0, widest).floor();
    let left = (area.x + (area.w - width) / 2.0).round();
    // 有标题列表时正文左侧为指示条留出位置，留白不足时正文右移并变窄。钉住的列表占一列，
    // 同样由正文让出；让出后正文窄于 `TOC_PINNED_BODY` 时只保留指示条的位置。
    let gutter = number_room(app, frame);
    let narrowed = |to: f32| (area.right() - pad - to).floor();
    let outline = has_outline(&parsed);
    let bars = area.x + TOC_ROOM + gutter;
    let pushed = area.x + TOC_PINNED_EDGE + TOC_PINNED_COLUMN + gutter;
    let pinned = app.state.markdown_toc_pinned && narrowed(pushed) >= TOC_PINNED_BODY;
    let (left, width) = match (outline, pinned) {
        (true, true) if left < pushed => (pushed, width.min(narrowed(pushed))),
        (true, _) if left < bars => (bars, width.min(narrowed(bars)).max(1.0)),
        _ => (left, width),
    };
    // 钉住状态变化时正文按新宽度排版，左边缘从原位置过渡到新位置，与标题列表的过渡同步。
    app.preview.markdown.toc_left = left;
    let left = match (app.preview.markdown.toc_pin, app.preview.markdown.toc_slide) {
        (Some((_, clock)), Some(from)) => clock.between(from, left),
        _ => {
            app.preview.markdown.toc_slide = None;
            left
        }
    };
    let base = (app.fonts.code * BASE_RATIO).round();
    let key = Key {
        width,
        base,
        text: p.text,
        background: p.editor_bg,
        images: app.preview.markdown.images_version,
        // 可编辑的预览在左侧按原文编行号，代码块里不再另编。
        numbers: app.markdown_line_numbers_on() && !app.markdown_live(),
        tall: (area.h * TALL_SHARE).round().max(TALL_MIN),
    };
    let source = Arc::as_ptr(&parsed) as usize;
    let live = app.live_layout_leaf();
    let live_key = (live.as_ref()).map(|l| live_key(l, app.preedit_at(Focus::Editor)));
    let blank = live.as_ref().is_some_and(|l| l.blank);
    let loaded = std::mem::take(&mut app.preview.markdown.loaded);
    let toggled = app.preview.markdown.toggled_now.take();
    let property_changed = std::mem::take(&mut app.preview.markdown.property_layout_dirty);
    // 重排之前光标在旧排版里的高度：重排后据此保持光标在屏幕上不动（见 [`keep_caret_still`]）。
    let mut still = None;
    // 记录重排之前各块的绘制位置，重排后各块从该位置滑动到新位置（见 `BlockMotion`）。
    let mut before = None;
    // 很长的文档只保留这个范围内的块的排版结果：视口前后各两屏，以及光标、选区所在的块前后。
    let near = {
        let view = drawn_scroll(
            app.preview
                .markdown
                .scroll()
                .min(app.preview.markdown.max_scroll()),
        ) - pad;
        let around = |block: usize| block.saturating_sub(NEAR_BLOCKS)..block + NEAR_BLOCKS + 1;
        let chosen = app.preview.markdown.selection;
        let picked = (app.preview.markdown.layout.as_ref().zip(chosen)).map(|(laid, (a, b))| {
            let block = |at: usize| laid.starts.partition_point(|&s| s <= at).saturating_sub(1);
            block(a.min(b)).saturating_sub(1)..block(a.max(b)) + 2
        });
        let caret = live.as_ref().map(|live| around(live.block));
        let head = (app.markdown_live() && app.live_caret_shown())
            .then(|| app.live_caret_source())
            .flatten()
            .filter(|&source| live.as_ref().is_none_or(|live| live.at != source))
            .and_then(|_| app.live_leaf())
            .map(|leaf| around(leaf.block));
        Near {
            rows: (view - area.h * 2.0, view + area.h * 3.0),
            blocks: caret.into_iter().chain(head).chain(picked).collect(),
        }
    };
    let mut laid = match app.preview.markdown.layout.take() {
        Some(laid)
            if laid.key == key
                && laid.source == source
                && laid.live == live_key
                && loaded.is_empty()
                && toggled.is_none()
                && !property_changed
                && !laid.pending()
                && (laid.near.as_ref()).is_none_or(|kept| kept.covers(&near, area.h)) =>
        {
            laid
        }
        old => {
            let hscroll = old.as_ref().map(|l| l.hscroll.clone()).unwrap_or_default();
            // 继续上一帧未完成的排版，或视口移到了保留的范围之外：排版条件和文字都未变，选区、
            // 查找结果和光标位置保留。
            let resumed = old.as_ref().is_some_and(|l| {
                l.key == key
                    && l.source == source
                    && l.live == live_key
                    && loaded.is_empty()
                    && toggled.is_none()
            });
            // 文字变了：选区、查找用的文字作废。只是重排（宽度、图片变了）时选区留着。
            if old.as_ref().is_none_or(|l| l.source != source) {
                app.preview.markdown.selection = None;
            }
            if !resumed {
                app.preview.markdown.find_doc = None;
                let clicked = app.preview.markdown.clicked.take();
                still = app
                    .markdown_live()
                    .then(|| app.live_caret_source())
                    .flatten()
                    .and_then(|caret| {
                        old_caret(app, old.as_ref(), source, live_key.as_ref(), clicked, caret)
                    });
                let slide = std::mem::take(&mut app.preview.markdown.slide);
                before = old
                    .as_ref()
                    .filter(|l| l.key == key && app.markdown_live())
                    .map(|l| (Before::of(app, l), slide));
            }
            // 文字未变时保留旧的各块供沿用，并记录视口顶边所在的块，排版期间保持其屏幕位置。
            let same = (old.as_ref())
                .filter(|l| l.source == source && l.blocks.len() == parsed.blocks.len());
            let width_changed = same.is_some_and(|l| l.key.width != key.width);
            let top = drawn_scroll(
                app.preview
                    .markdown
                    .scroll()
                    .min(app.preview.markdown.max_scroll()),
            );
            let held = same.and_then(|l| l.place_at(top - l.pad));
            let mut window = same.map(|l| l.blocks_in(top - l.pad - area.h, top + area.h * 2.0));
            let mut kept = same.map(|l| l.blocks.clone());
            let was_pending = same.is_some_and(Laid::pending);
            // 有图片刚读好时，用到它的块大小变了，需要重排；折叠块开合时，它所在的块需要
            // 重排；其余块沿用上次的结果。
            let mut reuse = old.filter(|l| l.key == key).map(|l| {
                l.fresh()
                    .into_iter()
                    .filter(|(_, block)| {
                        !block.uses_any(&loaded)
                            && toggled.as_deref().is_none_or(|url| !block.links_to(url))
                    })
                    .collect()
            });
            let deadline = app.preview.markdown.catch_up.deadline();
            // 视口上方的块高度变化后，视口内的块可能与排版前估计的不同：仍有未排的块时再排一轮。
            let mut laid = loop {
                let later = (kept.as_deref().zip(window.clone())).map(|(old, window)| Later {
                    old,
                    window,
                    deadline,
                });
                let fresh = reuse.take().unwrap_or_default();
                let laid = lay_out(
                    app,
                    frame,
                    &parsed,
                    key,
                    source,
                    live.as_ref(),
                    fresh,
                    later,
                    Some(near.clone()),
                );
                let y = held.map_or(top - pad, |(i, off)| {
                    laid.tops[i] + off.min(room_below(&laid, i))
                });
                let seen = laid.blocks_in(y, y + area.h);
                let waits = |i: usize| laid.stale.get(i).copied().unwrap_or(false);
                if window.as_ref() == Some(&seen) || !seen.clone().any(waits) {
                    break laid;
                }
                (reuse, window) = (Some(laid.fresh()), Some(seen));
                kept = Some(laid.blocks);
            };
            // 分帧排版和图片加载完成后，视口顶边所在块的屏幕位置不变。
            let anchored = width_changed || was_pending || laid.pending() || !loaded.is_empty();
            if let Some((i, off)) = held.filter(|_| anchored) {
                let y = laid.tops[i] + off.min(room_below(&laid, i)) + pad;
                app.preview.markdown.set_scroll(y);
                still = None;
            }
            app.preview.markdown.catch_up.pending = laid.pending();
            // 释放了要画的项的块不再使用排版缓存，缓存只保留其余的块用到的条目。
            if laid.near.is_some() {
                let used = laid.blocks.iter().map(|(_, block)| &*block.memo);
                app.preview.markdown.memo.keep_only(used);
            }
            laid.hscroll = hscroll;
            laid
        }
    };
    laid.left = left;
    laid.pad = pad;
    laid.drawn_upstream = app.live_upstream();
    app.note_links.footer_hits.clear();
    let footer = note_footer::height(app);
    app.preview.markdown.height = laid.height + pad * 2.0 + footer;
    // 可编辑的预览与编辑区相同：内容高于一屏时，最后一行下方可以滚出一段空白。
    if app.markdown_live() && app.preview.markdown.height > area.h {
        app.preview.markdown.height += area.h * crate::layout::OVERSCROLL;
    }
    keep_caret_still(app, still, &laid);
    app.preview.markdown.layout = Some(laid);
    app.follow_live_caret();
    // 分栏时右边跟着左边滚；刚换成只看预览时对齐一次。
    let split = area != app.layout.editor;
    if split {
        app.sync_markdown_scroll();
    } else {
        app.align_markdown_once();
    }
    app.scroll_to_pending_anchor();
    note_footer::follow_citation(app);
    app.hold_book_place();
    // `layout` 刚在上面放进去，取出时一定存在。
    let mut laid = app.preview.markdown.layout.take().expect("刚放进去");

    let max = (app.preview.markdown.height - area.h).max(0.0);
    let scroll = app.preview.markdown.scroll().min(max);
    let origin_y = area.y + pad - scroll;
    if let Some((before, slide)) = before {
        let muted = p.text_muted;
        let moves = before.motion(&laid, scroll, muted, slide);
        app.start_markdown_motion(moves);
    }
    let motion = app.preview.markdown.motion.take();
    // 拖着选文字时不出复制按钮。
    let mouse = if !app.dragging() {
        app.mouse()
    } else {
        (f32::NAN, f32::NAN)
    };
    // 分栏时标出编辑区光标所在的行，便于对照两侧的位置；键盘焦点在预览上时不标出。
    let caret = (split && !app.preview.markdown.focused)
        .then(|| app.markdown_caret_source())
        .flatten()
        .and_then(|source| laid.caret_point(source));
    // 可编辑的预览：文字光标和选区（原文位置），画在预览上。在预览上点过、移过光标或者打过
    // 字之后才显示光标；刚打开、只是阅读时不显示，光标停在文首也不会落在居中的图片旁边。
    let editing = app.live_caret_shown();
    let live_caret = app
        .markdown_live()
        .then(|| app.live_caret_source())
        .flatten()
        .filter(|_| editing);
    if let Some(source) = live_caret {
        laid.follow_caret(source);
    }
    let live_selection = live_caret
        .and_then(|_| {
            let doc = app.editor.active()?;
            let rope = doc.text().rope()?;
            let (a, b) = (doc.selection.from(), doc.selection.to());
            let byte = |c: usize| rope.char_to_byte(c.min(rope.len_chars()));
            Some((byte(a), byte(b)))
        })
        .filter(|(a, b)| a < b);
    let focus = app.markdown_focus() && app.focus == crate::app::Focus::Editor;
    let resizable = app.markdown_live();
    let md = &mut app.preview.markdown;
    let selection = md
        .selection
        .map(|(a, b)| (a.min(b), a.max(b)))
        .filter(|(a, b)| a < b);
    // 查找条在查预览的文字时，标出匹配。
    let search = &app.bars.search;
    let matches = (app.layout.search.h > 0.0
        && md.find_doc.is_some()
        && search.document() == md.find_doc.as_ref().map(|d| d.id))
    .then(|| (search, search.current_match()));
    // 可编辑的预览中查找对着文档：匹配是原文中的位置（字符偏移），换算到每段文字里的字。
    let live_matches = (live_caret.is_some() && app.layout.search.h > 0.0)
        .then(|| app.editor.active())
        .flatten()
        .filter(|d| search.document() == Some(d.id))
        .and_then(|d| d.text().rope())
        .map(|rope| (rope, search.current_match()));
    let mut wanted = Vec::new();
    // 这一帧绘制范围内的图片。
    let mut images = Vec::new();
    let mut shown = Vec::new();
    let mut toolbar = None;
    // 可横向滚动的表格的滚动条，各块绘制完成后再绘制：（块，表格，显示区域，滚动位置）。
    let mut bars = Vec::new();
    let first = laid
        .tops
        .partition_point(|&t| origin_y + t <= area.y)
        .saturating_sub(1);
    // 专注模式：光标所在的块以外画得淡一些。
    let focused = live_caret
        .filter(|_| focus)
        .and_then(|source| laid.block_at_source(source));
    let editing_yaml = !app.panes.drawing_background
        && app
            .overlays
            .picker
            .property
            .as_ref()
            .is_some_and(|p| p.input == PropertyInput::Source);
    for (i, (_, block)) in laid.blocks.iter().enumerate().skip(first) {
        let opacity = match focused {
            Some(b) if b != i => crate::theme::FOCUS_DIM,
            _ => 1.0,
        };
        let offset = |i: usize| motion.as_ref().map_or(0.0, |m| m.offset(i));
        let top = origin_y + laid.tops[i] + offset(i);
        if top > area.bottom() {
            break;
        }
        if top + block.height < area.y {
            continue;
        }
        let whole = area;
        // 滑动过程中每块只绘制到下一块（同样在滑动）的上边缘：变高的块逐渐显露，
        // 变矮的块不与下方滑上来的块重叠。
        let area = match &motion {
            Some(m) if m.moving() => {
                let next = laid
                    .tops
                    .get(i + 1)
                    .map_or(f32::INFINITY, |t| origin_y + t + offset(i + 1));
                let (y0, y1) = (top.max(area.y), next.min(area.bottom()));
                Rect::new(area.x, y0, area.w, (y1 - y0).max(0.0))
            }
            _ => area,
        };
        let fading = motion.as_ref().map_or(&[][..], |m| m.fading(i));
        let fade = motion.as_ref().map_or(1.0, |m| m.clock.progress());
        let fixed = |r: &Rect| Rect::new(left + r.x, top + r.y, r.w, r.h);
        // 刚显示原文的块：渲染结果另有偏移，原文裁剪到渲染结果的上边缘。
        let inner = motion.as_ref().and_then(|m| m.inner(i));
        let reach = inner.map_or(0.0, |(_, d)| d.abs());
        block.visible(
            area.y - top - reach,
            area.bottom() - top + reach,
            &mut shown,
        );
        if let Some(leaving) = motion.as_ref().and_then(|m| m.leaving(i)) {
            let at = origin_y + laid.tops[i] + leaving.offset * (1.0 - fade);
            let clip = Rect::new(whole.x, whole.y, whole.w, (top - whole.y).max(0.0));
            let alpha = 1.0 - motion.as_ref().map_or(1.0, |m| m.clock.linear());
            let source = &leaving.old.items[..leaving.items as usize];
            frame.with_opacity(alpha, |frame| draw_plain(frame, source, (left, at), clip));
        }
        // The inline YAML editor paints directly on the panel, replacing the preview text.
        let yaml_editor = (i == 0 && editing_yaml)
            .then(|| block.link_box(RAW))
            .flatten();
        frame.with_fade(p.editor_bg, opacity, |frame| {
            let row_selection = live_selection.filter(|_| {
                inner.is_none()
                    && motion.as_ref().is_none_or(|m| !m.moving())
                    && !shown.iter().any(|&k| {
                        matches!(
                            block.items[k as usize],
                            Item::Code { .. } | Item::Table { .. }
                        )
                    })
            });
            if let Some(range) = row_selection {
                selection::draw(
                    frame,
                    &laid,
                    i,
                    &shown,
                    Rect::new(left, top, key.width, block.height),
                    area,
                    range,
                    p.selection,
                );
            }
            let property_hover = app.overlays.context_menu.is_none()
                && !app.settings.open
                && !(app
                    .overlays
                    .picker
                    .property
                    .as_ref()
                    .is_some_and(|p| p.calendar() || p.choices())
                    && app.layout.picker.contains(mouse.0, mouse.1));
            let hovered_property = (i == 0 && property_hover)
                .then(|| {
                    shown.iter().find_map(|&index| {
                        let Item::Link { rect, url } = &block.items[index as usize] else {
                            return None;
                        };
                        let index = url.strip_prefix(PROPERTY)?.strip_prefix("row:")?;
                        fixed(rect)
                            .intersect(area)
                            .contains(mouse.0, mouse.1)
                            .then_some(index)
                    })
                })
                .flatten();
            let active_property = app.overlays.picker.property.as_ref().and_then(|p| p.row());
            for (k, item) in shown
                .iter()
                .map(|&k| (k as usize, &block.items[k as usize]))
            {
                let (top, area) = match inner {
                    Some((first, d)) if k >= first => (top + d, area),
                    Some((_, d)) => {
                        let edge =
                            (top + block.preview.map_or(0.0, |p| p.1) + d).min(area.bottom());
                        (
                            top,
                            Rect::new(area.x, area.y, area.w, (edge - area.y).max(0.0)),
                        )
                    }
                    None => (top, area),
                };
                // 可横向滚动的表格中的项：按滚动位置左移，裁剪到正文一栏内。
                let (off, strip) = laid.shift(i, k);
                let area = strip.map_or(area, |s| area.intersect(fixed(&s)));
                let left = left - off;
                let at = |r: &Rect| Rect::new(left + r.x, top + r.y, r.w, r.h);
                match item {
                    Item::Text {
                        x,
                        baseline,
                        text,
                        font,
                        faux,
                        size,
                        color,
                        span,
                    } => {
                        if yaml_editor.is_some_and(|(y, h)| (y..y + h).contains(baseline)) {
                            continue;
                        }
                        if let Some(span) = span {
                            let from = laid.starts[i] + span.start;
                            let to = from + span.advances.len();
                            let mut mark = |a: usize, b: usize, color: Color| {
                                let (a, b) = (a.max(from), b.min(to));
                                if a >= b {
                                    return;
                                }
                                let (a, b) = (a - from, b - from);
                                let x0: f32 = span.advances[..a].iter().sum();
                                let x1: f32 = x0 + span.advances[a..b].iter().sum::<f32>();
                                let (y0, h) = line_box(top + baseline, *size);
                                let r = Rect::new(left + x + x0, y0, x1 - x0, h);
                                frame.rect_clipped(r, color, 2.0, area);
                            };
                            if let Some((search, current)) = matches {
                                for &(a, b) in search.matches_in(from, to) {
                                    let color = if current == Some((a, b)) {
                                        p.search_current
                                    } else {
                                        p.search_match
                                    };
                                    mark(a, b, color);
                                }
                            }
                            if let Some((a, b)) = selection {
                                mark(a, b, p.selection);
                            }
                            let start = laid.ranges.get(i).map_or(0, |r| r.start);
                            let local = |s: usize| span.chars_before(text, s.saturating_sub(start));
                            if let Some((rope, current)) = live_matches
                                && let Some(first) = span.source_of(text, 0)
                                && let Some(last) = span.source_of(text, span.advances.len())
                            {
                                let chars = |b: usize| rope.byte_to_char(b.min(rope.len_bytes()));
                                let byte = |c: usize| rope.char_to_byte(c.min(rope.len_chars()));
                                let (c0, c1) = (chars(start + first), chars(start + last));
                                for &(a, b) in search.matches_in(c0, c1.max(c0 + 1)) {
                                    let color = if current == Some((a, b)) {
                                        p.search_current
                                    } else {
                                        p.search_match
                                    };
                                    mark(from + local(byte(a)), from + local(byte(b)), color);
                                }
                            }
                            if let Some((a, b)) = live_selection
                                && row_selection.is_none()
                                && span.source.is_some()
                            {
                                mark(from + local(a), from + local(b), p.selection);
                            }
                        }
                        frame.text_faux(
                            *font,
                            *faux,
                            *size,
                            left + x,
                            top + baseline,
                            text,
                            if fading.contains(&k) {
                                color.alpha(color.a * fade)
                            } else {
                                *color
                            },
                            area,
                        );
                    }
                    Item::Fill {
                        rect,
                        color,
                        radius,
                    } => frame.rect_clipped(at(rect), *color, *radius, area),
                    Item::Corners { rect, color, radii } => {
                        frame.rect_corners(at(rect), *color, *radii, area);
                    }
                    Item::Image {
                        rect,
                        path,
                        at: spot,
                    } => {
                        let r = at(rect);
                        // 可编辑的预览中，原文可以改写的图片可调整大小。
                        if spot.is_some() && resizable {
                            let image = ImageAt {
                                rect: r,
                                block: i,
                                item: k,
                            };
                            app.hits.markdown_images.push(image);
                        }
                        let scale = frame.scale();
                        let size = ((r.w * scale).round() as u32, (r.h * scale).round() as u32);
                        let drawn = match md.images.get(path) {
                            Some(ImageSlot::Ready { pixels, .. }) => {
                                pixels.iter().find(|px| (px.width, px.height) == size)
                            }
                            _ => None,
                        };
                        match drawn {
                            Some(px) => {
                                frame.pixels((r.x, r.y), px.width, px.height, &px.rgba, area)
                            }
                            None => wanted.push((path.clone(), size)),
                        }
                        images.push(path.clone());
                        // 本地图片可从右键菜单在新标签中打开。
                        if md.urls.get(path).is_none_or(|u| !u.contains("://")) {
                            app.hits.markdown_pictures.push((r, path.clone()));
                        }
                    }
                    Item::Link { rect, url } => {
                        let r = at(rect).intersect(area);
                        if r.w > 0.0 && r.h > 0.0 {
                            if let Some(action) = url.strip_prefix(PROPERTY) {
                                if let Some(index) = action.strip_prefix("row:") {
                                    if active_property == Some(index) {
                                        frame.rect_clipped(r, p.selected_unfocused, 6.0, area);
                                    } else if hovered_property == Some(index) {
                                        frame.rect_clipped(r, p.hover, 6.0, area);
                                    }
                                    continue;
                                }
                                let hovered = property_hover && r.contains(mouse.0, mouse.1);
                                if let Some(index) = action.strip_prefix("delete:") {
                                    if hovered_property == Some(index)
                                        || active_property == Some(index)
                                    {
                                        if hovered {
                                            frame.rect_clipped(
                                                r.inset(3.0, 3.0),
                                                p.error.alpha(0.12),
                                                5.0,
                                                area,
                                            );
                                        }
                                        draw_property_delete(
                                            frame,
                                            r,
                                            if hovered { p.error } else { p.text_muted },
                                            area,
                                        );
                                    }
                                } else if hovered && property_button(action) {
                                    frame.rect_clipped(r.inset(1.0, 1.0), p.hover, 5.0, area);
                                }
                            }
                            app.hits.markdown_links.push((r, url.to_string()));
                        }
                    }
                    Item::Check {
                        rect,
                        checked,
                        source,
                    } => {
                        let r = at(rect);
                        draw_check(frame, p, r, *checked, area);
                        // 可点击范围比框向外扩展一圈，较小的框也易于点中。
                        let hit = r.inset(-3.0, -3.0).intersect(area);
                        if hit.w > 0.0 && hit.h > 0.0 {
                            let start = laid.ranges.get(i).map_or(0, |r| r.start);
                            let at = start.saturating_add(*source as usize);
                            app.hits.markdown_links.push((hit, format!("{TASK}{at}")));
                        }
                    }
                    Item::Path {
                        strokes,
                        width,
                        color,
                        ..
                    } => {
                        let moved: Vec<Stroke> = strokes
                            .iter()
                            .map(|&s| winc_typeset::shift(s, left, top))
                            .collect();
                        frame.stroke(&moved, *width, *color, area);
                    }
                    Item::Polygon { points, color, .. } => {
                        let moved: Vec<(f32, f32)> =
                            points.iter().map(|&(x, y)| (x + left, y + top)).collect();
                        frame.polygon(&moved, *color, area);
                    }
                    // 所有块绘制完成后再绘制，使其覆盖在上一块的文字之上。
                    // 工具栏贴着正文一栏的左边，不随表格横向滚动。
                    Item::Table {
                        rect,
                        align: Some(align),
                        ..
                    } => toolbar = Some((fixed(rect), *align)),
                    Item::Table { .. } | Item::Spot { .. } => {}
                    Item::Code {
                        rect,
                        text,
                        diagram,
                    } => {
                        let r = at(rect);
                        app.hits.markdown_codes.push(r);
                        if r.contains(mouse.0, mouse.1) && area.contains(mouse.0, mouse.1) {
                            let button = copy_button(r);
                            code_button(frame, p, button, area, icons::copy);
                            app.hits.markdown_links.push((button, COPY.to_owned()));
                            app.hits.markdown_copy = Some(text.clone());
                            // 图的复制按钮左侧另有全屏查看按钮。
                            if *diagram {
                                let expand = Rect {
                                    x: button.x - button.w - 4.0,
                                    ..button
                                };
                                code_button(frame, p, expand, area, icons::expand);
                                app.hits.markdown_links.push((expand, EXPAND.to_owned()));
                                app.hits.markdown_expand = Some(text.clone());
                            }
                        }
                    }
                }
            }
        });
        for (s, scroller) in block.scrolls.iter().enumerate() {
            bars.push((i, s, fixed(&scroller.rect), laid.hscroll_of(i, s)));
        }
    }
    // 显示滚动条的表格：鼠标所在的表格，其次是最近一次横向滚动的表格。
    let mut chosen = None;
    for &(i, s, strip, off) in &bars {
        let scroller = &laid.blocks[i].1.scrolls[s];
        draw_table_fades(frame, p, scroller, strip, off, area);
        let bar = TableBar {
            key: (i, s),
            track: strip.intersect(area),
            content: scroller.content,
        };
        if strip.contains(mouse.0, mouse.1) {
            chosen = Some((bar, off, true));
        } else if chosen.is_none() && md.last_table == Some((i, s)) {
            chosen = Some((bar, off, false));
        }
    }
    let small = app.fonts.ui_small;
    resize::draw_image_resize(md, &app.hits.markdown_images, p, small, frame, area);
    // 跳转到的脚注或脚注引用铺一层强调色的底色，慢慢淡出（同标题列表点过的标题）。
    if let Some(((y, h), clock)) = md.anchor_flash {
        let alpha = 0.14 * (1.0 - clock.linear());
        let band = Rect::new(left, origin_y + y, key.width, h);
        frame.rect_clipped(
            band,
            p.accent.alpha(p.accent.a * alpha),
            TOC_FLASH_RADIUS,
            area,
        );
    }
    if let Some((table, align)) = toolbar {
        table_toolbar(
            frame,
            p,
            table,
            align,
            area,
            mouse,
            &mut app.hits.markdown_links,
        );
    }
    // 竖条在内容左边的留白正中，宽 2 逻辑像素。
    let caret_bar =
        caret.map(|(_, y, h)| Rect::new(left - pad / 2.0 - 1.0, area.y - scroll + y, 2.0, h));
    // 光标的绘制位置：找不到时（空文档、光标在第一块之前）位于内容左上角。
    // 滑动过程中随所在的块移动。
    let live_point = live_caret.map(|source| {
        let (x, y, h) =
            laid.caret_point(source)
                .unwrap_or((0.0, pad, (key.base * 0.85 * 1.4).round()));
        let gap_row = laid.blank_point(source).is_some();
        // 空行的命中高度由段距决定，文字光标仍按正文字号在行内居中。
        let (y, h) = if gap_row {
            let (_, body_height) = line_box(0.0, key.base);
            (y + (h - body_height) / 2.0, body_height)
        } else {
            (y, h)
        };
        let block = laid.block_at_source(source);
        let shift = motion.as_ref().zip(block).map_or(0.0, |(m, b)| {
            // 光标位于刚显示原文的块的渲染结果中时，加上渲染结果的偏移。
            let below = laid.blocks[b]
                .1
                .preview
                .map(|(_, top)| top + laid.tops[b] + pad);
            let inner = m
                .inner(b)
                .filter(|_| below.is_some_and(|top| y + h / 2.0 >= top));
            m.offset(b) + inner.map_or(0.0, |(_, d)| d)
        });
        let point = (x, y + shift, h);
        // 正文中的组字文字已排入所在的行，不再在其上重复绘制。
        let inline = laid.blocks.iter().any(|(_, b)| b.preedit);
        // 段落之后的空行上组字使用正文的字体，不沿用上方标题的字体。
        let body = (winc_render::FontKind::Ui, Default::default(), key.base);
        let font = if blank || gap_row {
            Some(body)
        } else {
            font_at(&laid, source)
        };
        (point, (!inline).then_some(font))
    });
    let footer_at = Rect::new(left, origin_y + laid.height, key.width, footer);
    md.layout = Some(laid);
    md.motion = motion;
    if footer > 0.0 {
        note_footer::draw(app, frame, footer_at, area);
    }
    // 与源代码的光标相同，只在编辑区持有焦点时绘制；焦点在终端、查找条等处时不显示，
    // 也不占用输入法候选窗的位置。
    if let Some(((x, y, h), font)) = live_point.filter(|_| {
        app.focus == Focus::Editor
            && (live_selection.is_none() || !app.preedit_at(Focus::Editor).is_empty())
    }) {
        let (x, y) = (left + x, area.y - scroll + y);
        app.ime.cursor = Some(draw_live_caret(app, frame, (x, y, h), font, area));
    }
    let missing: Vec<_> = wanted.iter().map(|(path, _)| path.clone()).collect();
    for (path, size) in wanted {
        app.request_markdown_pixels(&path, size);
    }
    app.release_markdown_images(&images, &missing);
    draw_table_scrollbar(app, frame, chosen, area);
    if let Some(bar) = caret_bar {
        frame.rect_clipped(bar, p.accent.alpha(0.7), 1.0, area);
    }
    draw_source_numbers(app, frame, area);
    draw_broken_links(app, frame, area);
    toolbar::draw_selection_toolbar(app, frame, area);
    draw_toc(app, frame, area, pad);

    // 和编辑区的滚动条一样：平时隐藏，滚动、悬停时显示，可以拖动。
    let owner = app.editor.active().map_or(0, |doc| doc.id.0);
    let content = app.preview.markdown.height;
    crate::view::draw_scrollbar(
        app,
        frame,
        ScrollArea::Markdown,
        owner,
        area,
        scroll,
        content,
    );
}

fn draw_property_delete(frame: &mut Frame<'_>, rect: Rect, color: Color, clip: Rect) {
    let x = rect.x + rect.w * 0.5;
    let y = rect.y + rect.h * 0.5;
    frame.stroke(
        &[
            Stroke::Line(x - 5.0, y - 3.0, x + 5.0, y - 3.0),
            Stroke::Line(x - 1.5, y - 5.0, x + 1.5, y - 5.0),
            Stroke::Line(x - 3.5, y - 3.0, x - 3.0, y + 5.0),
            Stroke::Line(x + 3.5, y - 3.0, x + 3.0, y + 5.0),
            Stroke::Line(x - 3.0, y + 5.0, x + 3.0, y + 5.0),
            Stroke::Line(x - 1.0, y - 0.75, x - 1.0, y + 2.5),
            Stroke::Line(x + 1.0, y - 0.75, x + 1.0, y + 2.5),
        ],
        1.1,
        color,
        clip,
    );
}

fn property_button(action: &str) -> bool {
    matches!(action, "add" | "collapse" | "fields" | "source")
        || ["type:", "item:", "list:"]
            .iter()
            .any(|prefix| action.starts_with(prefix))
}

/// 表格被裁掉的一侧渐隐的宽度（逻辑像素），与标签栏两端的渐隐（`TAB_FADE`）相同。
const TABLE_FADE: f32 = 24.0;

/// 返回第 `i` 块的上沿到下一块上沿的距离（逻辑像素）。
fn room_below(laid: &Laid, i: usize) -> f32 {
    let top = laid.tops[i];
    laid.tops
        .get(i + 1)
        .map_or(laid.blocks[i].1.height, |next| next - top)
}

/// mermaid 图的高度上限占预览区高度的比例。
const TALL_SHARE: f32 = 0.66;
/// mermaid 图高度上限的最小值（逻辑像素）。
const TALL_MIN: f32 = 320.0;

/// 绘制代码块或图右上角的一个按钮：不透明底色遮住下方内容，中间绘制图标 `icon`。
fn code_button(
    frame: &mut Frame<'_>,
    p: &Palette,
    button: Rect,
    clip: Rect,
    icon: fn(&mut Frame<'_>, (f32, f32), Color, Rect),
) {
    frame.rect_clipped(button, p.editor_bg, 6.0, clip);
    frame.rect_clipped(button, p.text.alpha(0.08), 6.0, clip);
    let center = (button.x + button.w / 2.0, button.y + button.h / 2.0);
    icon(frame, center, p.text_muted, clip);
}

/// 返回代码块 `code`（屏幕坐标，逻辑像素）右上角的复制按钮。
///
/// 边长 28 逻辑像素，离代码块右边和上边各 6 逻辑像素（经验值）。
pub(crate) fn copy_button(code: Rect) -> Rect {
    const SIZE: f32 = 28.0;
    Rect::new(code.right() - SIZE - 6.0, code.y + 6.0, SIZE, SIZE)
}

/// 任务列表的勾选框（逻辑像素）：选中时填强调色并画对勾，未选中时只画一个细边框。
fn draw_check(frame: &mut Frame<'_>, p: &Palette, r: Rect, checked: bool, clip: Rect) {
    if checked {
        frame.rect_clipped(r, p.accent, 3.0, clip);
        let (x, y, w, h) = (r.x, r.y, r.w, r.h);
        let mark = [
            Stroke::Line(x + w * 0.26, y + h * 0.52, x + w * 0.43, y + h * 0.69),
            Stroke::Line(x + w * 0.43, y + h * 0.69, x + w * 0.75, y + h * 0.33),
        ];
        frame.stroke(&mark, 1.5, p.text_on_accent, clip);
    } else {
        frame.rect_clipped(r, p.text_muted.alpha(0.6), 3.0, clip);
        frame.rect_clipped(r.inset(1.0, 1.0), p.editor_bg, 2.0, clip);
    }
}

#[cfg(test)]
pub(crate) mod tests;
