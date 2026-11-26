//! 链接地址和写作命令的补全列表：光标下方的卡片，文件行为图标、文件名和所在目录，命令行为
//! 名字和快捷键；下方放不下时显示在光标上方。
//!
//! 列表卡片、分组分隔线、图标自绘槽与各行命中区域均由 [`crate::ui`] 声明式布局与弹出层定位
//! 原语构建。

use super::popup_menu::{MARGIN, ROW_INSET, band_ink};
use super::*;
use crate::app::preview::markdown::suggest::Hit;
use crate::app::preview::markdown::suggest::SUGGEST_VISIBLE;
use crate::app::{MenuIcon, POPUP_PAD, ScrollArea};
use crate::ui::{self, Node, PopoverOptions, ScrollMetrics, TextBasis};

/// 构造补全卡片所需的不可变视图参数。
struct SuggestCardArgs {
    palette: &'static Palette,
    fonts: (f32, f32),
    basis: TextBasis,
    selected: usize,
    hovered: Option<usize>,
    card: Rect,
    commands: bool,
}

/// 列表的最小、最大宽度（逻辑像素）。
const WIDTH: (f32, f32) = (240.0, 420.0);
/// 每行的高度（逻辑像素）：比菜单的行高一些，带图标的各行之间留出间隙。
const LIST_ROW: f32 = 28.0;
/// 每行底色上下各缩进的距离（逻辑像素）：相邻两行的底色之间相隔两倍于此。
const BAND_INSET: f32 = 2.0;
/// 行内左右两侧的留白（逻辑像素）：图标离底色左边、快捷键离底色右边的距离相同。
const PAD: f32 = 8.0;
/// 图标所占的宽度（逻辑像素），等于写作命令图标与文件图标的宽度。
const ICON: f32 = 12.0;
/// 图标与文字之间的距离（逻辑像素）。
const ICON_GAP: f32 = 8.0;
/// 文件名之后到目录的间距（逻辑像素）。
const DIR_GAP: f32 = 8.0;

/// 单行补全条目预处理后的展示数据（用于构造声明式行节点）。
struct SuggestRow {
    index: usize,
    name: String,
    icon_file: String,
    secondary: Option<String>,
    hit: Option<Hit>,
    has_separator: bool,
}

/// 绘制补全列表（开着、有文件时），记下卡片和各行的位置。
pub(super) fn draw_suggest(app: &mut App, frame: &mut Frame<'_>) {
    let suggest = &app.preview.markdown.suggest;
    let Some(caret) = app.ime.cursor else {
        return;
    };
    if suggest.at.is_none() || suggest.hits.is_empty() {
        return;
    }
    // 文件显示文件名和所在目录，标题显示文字和级别（`H2`）。
    let rows: Vec<(String, String)> = (suggest.hits.iter())
        .map(|hit| match hit {
            Hit::File(path, _) => match path.rsplit_once('/') {
                Some((dir, name)) => (name.to_owned(), dir.to_owned()),
                None => (path.clone(), String::new()),
            },
            Hit::Heading(level, text, _) => (text.clone(), format!("H{level}")),
            Hit::Command(command) => {
                let key = command.action().and_then(crate::keymap::shortcut);
                (command.label().to_owned(), key.unwrap_or_default())
            }
        })
        .collect();
    let commands = matches!(suggest.hits.first(), Some(Hit::Command(_)));
    let headings = matches!(suggest.hits.first(), Some(Hit::Heading(..)));
    let (size, small) = (app.fonts.ui, app.fonts.ui_small);
    let text_x = ROW_INSET + PAD + ICON + ICON_GAP;
    let widest = (rows.iter())
        .map(|(name, dir)| {
            let dir = if dir.is_empty() {
                0.0
            } else {
                DIR_GAP + frame.measure(FontKind::Ui, small, dir)
            };
            frame.measure(FontKind::Ui, size, name) + dir
        })
        .fold(0.0, f32::max);
    let area = app.layout.editor;
    let w = (text_x + widest + PAD + ROW_INSET).clamp(WIDTH.0, WIDTH.1);
    let shown = rows.len().min(SUGGEST_VISIBLE);
    let scrolls = rows.len() > shown;
    let h = 2.0 * POPUP_PAD + shown as f32 * LIST_ROW;
    let placed = ui::place_below_or_above(
        caret,
        area,
        (w, h),
        PopoverOptions {
            margin: MARGIN,
            gap: 4.0,
            offset_x: -text_x,
            prefer_larger_side: false,
            min_height: None,
        },
    );
    let card = Rect::new(
        placed.x.round(),
        placed.y.round(),
        placed.w.round(),
        placed.h,
    );
    let first = ScrollMetrics::new(
        shown as f32,
        rows.len() as f32,
        app.preview.markdown.suggest.first as f32,
    )
    .clamp(app.preview.markdown.suggest.first as f32) as usize;
    app.preview.markdown.suggest.first = first;

    let room = (card.w - text_x - ROW_INSET - PAD).max(0.0);
    let visible_rows: Vec<SuggestRow> = rows
        .iter()
        .enumerate()
        .skip(first)
        .take(shown)
        .map(|(i, (name, dir))| {
            let shown_name = ellipsize(frame, name, size, room).into_owned();
            let rest = room - frame.measure(FontKind::Ui, size, &shown_name) - DIR_GAP;
            let secondary = if !dir.is_empty() && rest > 24.0 {
                Some(ellipsize(frame, dir, small, rest).into_owned())
            } else {
                None
            };
            SuggestRow {
                index: i,
                name: shown_name,
                icon_file: if headings {
                    String::from("a.md")
                } else {
                    name.clone()
                },
                secondary,
                hit: suggest_hit(app, i),
                has_separator: i > first && group_of(app, i) != group_of(app, i - 1),
            }
        })
        .collect();

    let basis = ui::text_basis(app.fonts.ui, app.metrics.ui);
    let hovered = match app.pointer.hover {
        Hover::Suggest(i) => Some(i),
        _ => None,
    };
    let tree = build_card(
        SuggestCardArgs {
            palette: app.palette,
            fonts: (size, small),
            basis,
            selected: app.preview.markdown.suggest.selected,
            hovered,
            card,
            commands,
        },
        visible_rows,
    );
    let hits = ui::render(frame, basis, card, tree);

    // 项数多于可见行数时画滚动条，样式与其他列表相同。
    if scrolls {
        let track = Rect::new(card.x, card.y + POPUP_PAD, card.w, shown as f32 * LIST_ROW);
        let (scroll, content) = (first as f32 * LIST_ROW, rows.len() as f32 * LIST_ROW);
        let owner = app.editor.active().map_or(0, |doc| doc.id.0);
        draw_scrollbar(
            app,
            frame,
            ScrollArea::Suggest,
            owner,
            track,
            scroll,
            content,
        );
    }
    app.hits.suggest = hits.into_entries();
    app.hits.suggest_card = Some(card);
}

/// 用 [`crate::ui`] 声明式节点构建补全列表卡片及其可见行。
fn build_card(args: SuggestCardArgs, rows: Vec<SuggestRow>) -> Node<'static, usize> {
    let SuggestCardArgs {
        palette: p,
        fonts: (size, small),
        basis,
        selected,
        hovered,
        card,
        commands,
    } = args;
    let band_h = LIST_ROW - 2.0 * BAND_INSET;
    let optical = (basis.x_height + basis.cap_height) / 4.0;

    let row_nodes = rows.into_iter().map(move |entry| {
        let i = entry.index;
        let is_selected = i == selected;
        let is_hovered = !is_selected && hovered == Some(i);
        let (ink, dim) = if is_selected {
            band_ink(p)
        } else {
            (p.text, p.text_dim)
        };

        let icon_hit = entry.hit;
        let file_key = entry.icon_file;
        let icon_slot = ui::custom((ICON, band_h), move |frame, rect, clip| {
            let cy = ((basis.baseline(rect, size) - optical) * 2.0).round() / 2.0;
            match &icon_hit {
                // 命令使用写作命令的图标。
                Some(Hit::Command(command)) => {
                    let center = (rect.x + ICON / 2.0, cy);
                    crate::icons::writing(frame, center, command.glyph(), dim, clip);
                }
                // 标题使用 Markdown 文件的图标。
                _ => {
                    let (glyph, [dark, light]) = crate::theme::file_kind(&file_key);
                    let icon = MenuIcon::File(glyph, [dark, light]);
                    let (iw, ih) = icon.size();
                    let color = Color::hex(if p.dark { dark } else { light });
                    let at = (rect.x + (ICON - iw) / 2.0, cy - ih / 2.0);
                    crate::icons::file(frame, (at.0 + 2.0, at.1 + 0.5), glyph, color, clip);
                }
            }
        });

        let mut labels = ui::hstack()
            .flex_1()
            .h_full()
            .items_center()
            .gap(DIR_GAP)
            .child(
                ui::text(entry.name)
                    .text_size(size)
                    .color(ink)
                    .flex_shrink(0.0),
            );
        if commands {
            labels = labels.justify_between();
        }
        if let Some(secondary) = entry.secondary {
            labels = labels.child(
                ui::text(secondary)
                    .text_size(small)
                    .color(dim)
                    .flex_shrink(0.0),
            );
        }

        let mut band = ui::hstack()
            .w_full()
            .h_full()
            .px(PAD)
            .items_center()
            .gap(ICON_GAP)
            .child(icon_slot)
            .child(labels);
        if is_selected {
            band = band.bg(p.accent_fill).rounded(RADIUS_ROW);
        } else if is_hovered {
            band = band.bg(p.hover).rounded(RADIUS_ROW);
        }

        let mut row = ui::vstack().w_full().h(LIST_ROW).flex_shrink(0.0).hit(i);
        if entry.has_separator {
            // 不同类的命令之间画一道分隔线，借用行顶缩进空间，不增加行高。
            row = row
                .child(
                    ui::box_()
                        .w_full()
                        .h(1.0)
                        .px(ROW_INSET + PAD)
                        .flex_shrink(0.0)
                        .child(ui::divider(p.separator)),
                )
                .child(
                    ui::box_()
                        .w_full()
                        .flex_1()
                        .px(ROW_INSET)
                        .pt(BAND_INSET - 1.0)
                        .pb(BAND_INSET)
                        .child(band),
                );
        } else {
            row = row.child(
                ui::box_()
                    .w_full()
                    .flex_1()
                    .px(ROW_INSET)
                    .py(BAND_INSET)
                    .child(band),
            );
        }
        row
    });

    ui::vstack()
        .w(card.w)
        .h(card.h)
        .py(POPUP_PAD)
        .bg(p.popup_bg)
        .rounded(RADIUS_POPUP)
        .card_frame(p.card_frame())
        .child(
            ui::vstack()
                .w_full()
                .flex_1()
                .overflow_hidden()
                .children(row_nodes),
        )
}

/// 返回补全列表第 `i` 项所属的类；不是命令时为 `None`。
fn group_of(app: &App, i: usize) -> Option<u8> {
    match app.preview.markdown.suggest.hits.get(i) {
        Some(Hit::Command(command)) => Some(command.group()),
        _ => None,
    }
}

/// 返回补全列表的第 `i` 项。
fn suggest_hit(app: &App, i: usize) -> Option<Hit> {
    app.preview.markdown.suggest.hits.get(i).cloned()
}
