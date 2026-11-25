//! Markdown 标题指示条与目录卡片。

use super::{App, Color, FontKind, Frame, PAD_NARROW, Palette, Rect, drawn_scroll, number_room};
use crate::app::preview::markdown::TocTarget;
use crate::app::{Hover, ScrollArea};

/// 标题指示条左边离预览区域左边至少的距离（逻辑像素）。
const TOC_X: f32 = 10.0;
/// 标题指示条右端（当前所在一节那一条的右端）离正文左边的距离（逻辑像素）：指示条靠着正文，
/// 随正文居中的位置移动。
const TOC_GAP: f32 = 36.0;
/// 指示条中一条的长度（逻辑像素）；当前所在的一节为 [`TOC_CURRENT`]。
const TOC_DASH: f32 = 14.0;
/// 指示条中当前所在一节那一条的长度（逻辑像素）：比其他各条长，一眼看出读到哪里。
const TOC_CURRENT: f32 = 18.0;
/// 画标题指示条所需的最小左侧留白（逻辑像素）：指示条右边与正文之间至少留 8。
pub(super) const TOC_ROOM: f32 = TOC_X + TOC_CURRENT + 8.0;
/// 指示条中相邻两条的间距（逻辑像素）；标题过多时缩小，见 [`draw_toc`]。
const TOC_STEP: f32 = 8.0;
/// 指示条中一条的粗细（逻辑像素）；当前所在的一节再粗 1。
const TOC_BAR: f32 = 2.0;
/// 标题列表一行的高度（逻辑像素）：比文件树的行（`TREE_ROW`）紧凑，标题多时一屏列得下更多。
const TOC_ROW: f32 = 20.0;
/// 点过的标题在正文里铺的底色的圆角半径（逻辑像素）：底色与正文同宽，圆角取小，比列表行的
/// 圆角（`RADIUS_ROW`）小，整行看起来不像一个按钮。
pub(super) const TOC_FLASH_RADIUS: f32 = 3.0;
/// 标题列表的内边距（逻辑像素）：行的底色离卡片边缘的距离。
const TOC_PAD: f32 = 4.0;
/// 标题列表里每深一级标题，文字缩进的宽度（逻辑像素）。
const TOC_INDENT: f32 = 12.0;
/// 标题列表右上角图钉按钮的宽度（逻辑像素）：与一行同高的方块，第一行的标题让出这一段。
const TOC_PIN: f32 = TOC_ROW;
/// 钉住的标题列表最窄的宽度（逻辑像素）：放得下横线、图钉和约六个汉字；左侧留白比它还窄时
/// 收成指示条。
const TOC_PINNED_MIN: f32 = 140.0;

/// 展开的标题列表的最小和最大宽度（逻辑像素）；超出最大宽度的标题文字被省略。
const TOC_WIDTHS: (f32, f32) = (180.0, 320.0);
/// 正文为钉住的标题列表让出的列宽（逻辑像素），等于列表的最小宽度。取固定值，
/// 使正文的位置不随标题长短变化；留白更宽时列表可以更宽，至多为 [`TOC_WIDTHS`] 的上限。
pub(super) const TOC_PINNED_COLUMN: f32 = TOC_WIDTHS.0;
/// 钉住的标题列表两侧间距之和（逻辑像素）：左侧距预览区域 4，右侧距正文或行号 8，与
/// [`draw_toc`] 的布局一致。
pub(super) const TOC_PINNED_EDGE: f32 = 12.0;
/// 让出一列后正文的最小宽度（逻辑像素），等于区域宽 600 逻辑像素、使用窄留白时的正文宽度；
/// 不足时不让出，列表显示为指示条。
pub(super) const TOC_PINNED_BODY: f32 = 600.0 - 2.0 * PAD_NARROW;

/// 文档是否有标题列表：至少两个文字不为空的标题，与 [`draw_toc`] 的条件相同。
pub(super) fn has_outline(parsed: &winc_preview::markdown::Markdown) -> bool {
    let titled = |block: &winc_preview::markdown::Block| match block {
        winc_preview::markdown::Block::Heading(_, inline) => {
            !heading_title(&inline.text).is_empty()
        }
        _ => false,
    };
    parsed
        .blocks
        .iter()
        .filter(|block| titled(block))
        .take(2)
        .count()
        == 2
}

/// 标题指示条和列表里一条横线的颜色与粗细：已经读过的各节用次要色，当前所在的一节用强调色并
/// 加粗，还没读到的用更淡的颜色，像阅读进度。
fn toc_dash(p: &Palette, i: usize, current: Option<usize>) -> (Color, f32) {
    match current {
        Some(c) if i == c => (p.accent, TOC_BAR + 1.0),
        Some(c) if i < c => (p.text_muted, TOC_BAR),
        _ => (p.text_dim, TOC_BAR),
    }
}

/// 在预览左侧的留白中绘制标题指示条：每个标题一条横线，颜色表示阅读进度（见 [`toc_dash`]）。
///
/// 鼠标停留满 `TOC_HOVER_DELAY` 后标题列表在指示条处展开，点击一项滚动到该标题；
/// 图钉使列表在正文左侧保持展开，放不下时仍显示为指示条（见 [`super::draw`]）。标题少于两个时不绘制。
/// 可点击的区域写入 `app.hits.markdown_toc`。
pub(super) fn draw_toc(app: &mut App, frame: &mut Frame<'_>, area: Rect, pad: f32) {
    let (Some(parsed), Some(laid)) = (
        app.preview.markdown.parsed.clone(),
        app.preview.markdown.layout.as_ref(),
    ) else {
        app.preview.markdown.toc.reset_inactive();
        return;
    };
    // 可编辑的预览在正文左侧显示原文行号，指示条位于行号左侧。
    let gutter = number_room(app, frame);
    let col_left = laid.column_bounds().0;
    if col_left - gutter - area.x < TOC_ROOM {
        app.preview.markdown.toc.reset_inactive();
        return;
    }
    // 各标题：（级别，文字，所在顶层块的上边缘，块高）。
    let headings: Vec<(u8, String, f32, f32)> = parsed
        .blocks
        .iter()
        .zip(laid.iter_blocks())
        .filter_map(|(block, (_, top, laid))| match block {
            winc_preview::markdown::Block::Heading(level, inline) => {
                Some((*level, heading_title(&inline.text), top, laid.height))
            }
            _ => None,
        })
        .filter(|(_, title, ..)| !title.is_empty())
        .collect();
    if headings.len() < 2 {
        app.preview.markdown.toc.reset_inactive();
        return;
    }
    let p = app.palette;
    let scroll = drawn_scroll(app.preview.markdown.scroll());
    let max = (app.preview.markdown.height - area.h).max(0.0);
    let tops: Vec<f32> = headings.iter().map(|h| h.2).collect();
    // 当前所在的一节：顶边已经滚到预览顶上（或以上）的最后一个标题。滚到底时，文末几个标题
    // 滚不到顶上，取屏幕里看得到的最后一个；在标题列表里点过、之后没有再滚动时，取点的那一个。
    let current =
        app.preview
            .markdown
            .toc
            .resolve_current(scroll, max, scroll + area.h - pad * 2.0, &tops);
    let top_level = headings.iter().map(|h| h.0).min().unwrap_or(1);
    let depth = |level: u8| f32::from(level.saturating_sub(top_level).min(3));

    // 标题很多时缩小间距，放不下的部分不画。
    let room = (area.h - pad * 2.0).max(0.0);
    let step = (room / headings.len() as f32).clamp(TOC_BAR + 2.0, TOC_STEP);
    let shown = ((room / step).floor() as usize).min(headings.len());
    let x0 = (col_left - gutter - TOC_GAP - TOC_CURRENT).max(area.x + TOC_X);
    let y0 = area.y + pad;
    let left = col_left;
    let width = laid.key.width;

    // 点过的标题在正文里铺一层强调色的底色，慢慢淡出。底色与正文同宽，左右与表格、代码块的
    // 边缘对齐。
    if let Some((i, alpha)) = app.preview.markdown.toc.flash_highlight()
        && let Some(&(_, _, top, height)) = headings.get(i)
    {
        let band = Rect::new(left, y0 - scroll + top, width, height);
        frame.rect_clipped(
            band,
            p.accent.alpha(p.accent.a * alpha),
            TOC_FLASH_RADIUS,
            area,
        );
    }
    let size = app.fonts.ui;
    // 行里的横线离卡片左边的距离；鼠标停留展开时与指示条在同一列。
    let inner = TOC_PAD + 8.0;
    let on_bar = app.scrollbar_active(crate::layout::Scrollbar::MarkdownToc);
    let on_toc = on_bar || matches!(app.pointer.hover, Hover::MarkdownToc(_));
    let pinned = app.state.markdown_toc_pinned;
    let display =
        app.preview
            .markdown
            .toc
            .display_state(pinned, on_toc, area.x, gutter, TOC_PINNED_MIN);
    let widest = display.open.then(|| {
        (headings.iter())
            .map(|(level, title, ..)| {
                depth(*level) * TOC_INDENT + frame.measure(FontKind::Ui, size, title)
            })
            .fold(0.0_f32, f32::max)
    });
    let natural = widest
        .map(|w| (w + inner + TOC_DASH + 10.0 + 8.0 + TOC_PAD).clamp(TOC_WIDTHS.0, TOC_WIDTHS.1));
    // 钉住的列表靠着正文放在左侧留白里，右边与正文之间留 8，留白不够时变窄（标题省略）；
    // 窄于 [`TOC_PINNED_MIN`] 时收成指示条，不盖住正文。鼠标停留展开时卡片在指示条处展开、
    // 盖住指示条，不超出预览的左边。
    // 钉住的列表放在行号左边。
    let edge = left - gutter - 8.0;
    let room = (edge - (area.x + 4.0)).max(0.0);
    let pinning = display.pinning;
    let hover_x = (x0 - inner).max(area.x + 4.0);
    let x = hover_x + (edge - natural.unwrap_or(0.0).min(room) - hover_x) * pinning;
    let dash_x = (x0 - hover_x) + (inner - (x0 - hover_x)) * pinning;
    let text_x = dash_x + TOC_DASH + 10.0;
    let card_w = natural.map(|w| {
        let floating = w.min(area.right() - 16.0 - hover_x);
        floating + (w.min(room) - floating) * pinning
    });
    // 列表展开着时，指示条的各条由列表里的横线接替（从原位滑到各行），这里不再画。
    let open = display.open;
    if !open {
        app.preview.markdown.toc.close_list();
    }
    let bar_y = |i: usize| y0 + i as f32 * step + TOC_BAR / 2.0;
    for i in (0..shown).filter(|_| !open) {
        let (color, thick) = toc_dash(p, i, current);
        let len = if current == Some(i) {
            TOC_CURRENT
        } else {
            TOC_DASH
        };
        let y = y0 + i as f32 * step + (TOC_BAR - thick) / 2.0;
        frame.rect_clipped(Rect::new(x0, y, len, thick), color, thick / 2.0, area);
    }
    let bars_h = (shown.max(1) - 1) as f32 * step + TOC_BAR;
    let bars = Rect::new(x0 - 8.0, y0 - 8.0, TOC_CURRENT + 16.0, bars_h + 16.0);

    let mut hits = Vec::new();
    if let Some(w) = card_w.filter(|_| open) {
        let fit = ((area.h - 16.0 - TOC_PAD * 2.0) / TOC_ROW).floor().max(1.0) as usize;
        let rows = headings.len().min(fit);
        // 标题超过可显示的行数时，列出当前所在一节附近的一段，可按行滚动。
        let natural = current
            .unwrap_or(0)
            .saturating_sub(rows / 2)
            .min(headings.len() - rows);
        let docked_y = bar_y(0) - (TOC_PAD + TOC_ROW / 2.0);
        let list_frame = app.preview.markdown.toc.layout_list(
            crate::app::preview::markdown::TocListLayout {
                area,
                x,
                w,
                rows,
                total: headings.len(),
                natural,
                current,
                docked_y,
                pad: TOC_PAD,
                row_h: TOC_ROW,
                pinning,
                stays: display.stays,
                shown,
            },
            bar_y,
        );
        let start = list_frame.start;
        let card = list_frame.card;
        // 展开时卡片从指示条处展开并淡入，各行的横线从指示条中对应的位置移到所在的行；收起时相反。
        // 钉住的列表始终显示，不参与悬停展开。
        let progress = display.progress;
        // 过渡期间按展开程度裁剪卡片：从指示条所在的位置扩展到整张卡片。
        let grow = |from: f32, to: f32| from + (to - from) * progress;
        let seed = bars.intersect(card);
        let reveal = Rect::new(
            grow(seed.x, card.x),
            grow(seed.y, card.y),
            grow(seed.w, card.w),
            grow(seed.h, card.h),
        );
        // 收起中的列表不再响应鼠标。
        let live = display.live;
        let mut dashes = Vec::with_capacity(rows);
        frame.with_opacity(progress, |frame| {
            // 钉住后列表与预览融为一体：卡片的底色、边框和阴影随钉住的程度淡出。
            frame.with_opacity(1.0 - pinning, |frame| {
                crate::view::draw_card(frame, p, reveal, crate::theme::RADIUS_POPUP, p.popup_bg);
            });
            for (n, i) in (start..start + rows).enumerate() {
                let (level, title, ..) = &headings[i];
                let band = Rect::new(
                    card.x + TOC_PAD,
                    card.y + TOC_PAD + n as f32 * TOC_ROW,
                    card.w - TOC_PAD * 2.0,
                    TOC_ROW,
                );
                if app.pointer.hover == Hover::MarkdownToc(TocTarget::Heading(i)) {
                    frame.rect_clipped(band, p.hover, crate::theme::RADIUS_ROW, reveal);
                }
                let row_mid = band.y + band.h / 2.0;
                let from = if i < shown { bar_y(i) } else { row_mid };
                dashes.push((i, from + (row_mid - from) * progress));
                let tx = card.x + text_x + depth(*level) * TOC_INDENT;
                // 第一行的右端是图钉按钮。
                let pin_room = if n == 0 { TOC_PIN } else { 0.0 };
                let room = (band.right() - 8.0 - pin_room - tx).max(0.0);
                let title = crate::view::ellipsize(frame, title, size, room);
                // 当前所在一节的标题加粗显示。
                let faux = winc_render::Faux {
                    bold: current == Some(i),
                    ..Default::default()
                };
                let baseline = crate::view::ui_baseline(app, band, size);
                frame.text_faux(
                    FontKind::Ui,
                    faux,
                    size,
                    tx,
                    baseline,
                    &title,
                    p.text,
                    reveal,
                );
                if live {
                    hits.push((band, TocTarget::Heading(i)));
                }
            }
            // 图钉按钮盖在第一行的右端：命中时取先记下的一项，按钮排在各行之前。
            let pin = Rect::new(
                card.right() - TOC_PAD - TOC_PIN,
                card.y + TOC_PAD,
                TOC_PIN,
                TOC_ROW,
            );
            let pin_hovered = app.pointer.hover == Hover::MarkdownToc(TocTarget::Pin);
            if pin_hovered {
                frame.rect_clipped(pin, p.hover, crate::theme::RADIUS_ROW, reveal);
            }
            let color = match (pinned, pin_hovered) {
                (true, _) => p.accent,
                (false, true) => p.text,
                (false, false) => p.text_dim,
            };
            let center = (pin.x + pin.w / 2.0, pin.y + pin.h / 2.0);
            crate::icons::pin(frame, center, color, reveal);
            if live {
                hits.insert(0, (pin, TocTarget::Pin));
            }
        });
        // 标题没有全部列出时带滚动条。
        if let Some((scroll, content)) = list_frame.scrollbar.filter(|_| live) {
            let owner = app.editor.active().map_or(0, |doc| doc.id.0);
            let area = ScrollArea::MarkdownToc;
            crate::view::draw_scrollbar(app, frame, area, owner, list_frame.track, scroll, content);
        }
        // 横线不随卡片淡入，始终不透明：展开的第一帧与指示条完全重合。
        for (i, mid) in dashes {
            let (color, thick) = toc_dash(p, i, current);
            let len = if current == Some(i) {
                TOC_CURRENT
            } else {
                TOC_DASH
            };
            let dash = Rect::new(card.x + dash_x, mid - thick / 2.0, len, thick);
            frame.rect_clipped(dash, color, thick / 2.0, area);
        }
        // 卡片的内边距也算在内：鼠标经过行与行之外的地方时列表不收起。
        if live {
            hits.push((card, TocTarget::Bars));
        }
    }
    hits.push((bars, TocTarget::Bars));
    app.hits.markdown_toc = hits;
    app.hits.markdown_toc_tops = tops;
}

/// 标题列表里显示的标题文字：硬换行换成空格，去掉图片占位符和控制字符，首尾不留空白。
fn heading_title(text: &str) -> String {
    let text: String = text
        .chars()
        .filter(|&c| c != winc_preview::markdown::IMAGE_MARK)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
