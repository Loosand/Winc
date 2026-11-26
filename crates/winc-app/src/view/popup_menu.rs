//! 窗口内绘制的菜单（没有系统菜单时，见 `app/input/context_menu/popup.rs`）：一张和其他浮层
//! 同样的卡片，各行左边留出打勾的一列，右边是快捷键或子菜单的箭头。各级菜单的位置在这里算好，
//! 记回菜单里，悬停和点击按它判断。

use super::*;
use crate::app::{CHECK_COLUMN, ICON_COLUMN, MenuIcon, POPUP_PAD, PopupLevel, PopupRow};
use crate::icons::Direction;
use crate::ui::{self, CardFrame, Node, TextBasis};

/// 一行的高度（逻辑像素）。
pub(super) const ROW: f32 = 24.0;
/// 标题下面还有一行小字时，一行的高度（逻辑像素）。
const ROW_TALL: f32 = 40.0;
const WORKSPACE_ROW: f32 = 28.0;
const WORKSPACE_INSET: f32 = 14.0;
/// 分隔线那一行的高度（逻辑像素）。
const SEPARATOR: f32 = 9.0;
/// 文字右边到快捷键、箭头的最小间距（逻辑像素）。
const KEY_GAP: f32 = 28.0;
/// 右边留白（逻辑像素）。
const RIGHT_PAD: f32 = 12.0;
/// 菜单的最小、最大宽度（逻辑像素）。
const WIDTH: (f32, f32) = (160.0, 420.0);
/// 菜单离窗口边缘至少留出的距离（逻辑像素）。
pub(super) const MARGIN: f32 = 8.0;
/// 行底色左右缩进的距离（逻辑像素）。
pub(super) const ROW_INSET: f32 = 4.0;
/// 选中底色上的文字颜色和次要文字颜色。
pub(super) fn band_ink(p: &Palette) -> (Color, Color) {
    let ink = p.text_on_accent;
    (ink, ink.alpha(ink.a * 0.8))
}

/// 绘制窗口内的菜单（打开着时），并把各级菜单的位置记回菜单。
pub(super) fn draw(app: &mut App, frame: &mut Frame<'_>) {
    let workspace =
        app.notebook_menu_open(crate::app::workspace::notebook::SidebarTarget::FolderMenu);
    let row_base = if workspace { WORKSPACE_ROW } else { ROW };
    let Some(mut popup) = app
        .overlays
        .context_menu
        .as_mut()
        .and_then(|menu| menu.popup.take())
    else {
        return;
    };
    let window = app.layout.window;
    for l in 0..popup.levels.len() {
        let (w, content) = measure(app, frame, &popup.levels[l].rows, row_base);
        let w = if l == 0 && workspace {
            popup.min_width
        } else if l == 0 {
            w.max(popup.min_width)
        } else {
            w
        };
        let rect = if l == 0 {
            let anchor_y_offset = if popup.at_checked {
                checked_offset(&popup.levels[0].rows).unwrap_or(0.0)
            } else {
                0.0
            };
            crate::ui::place_at_point(
                window,
                (w, content),
                crate::ui::PointPlacement {
                    point: popup.at,
                    margin: MARGIN,
                    right_aligned: popup.right_aligned,
                    anchor_y_offset,
                },
            )
        } else {
            // 子菜单贴着展开它的那一行，在上一级的右边；放不下时放到左边。
            let parent = &popup.levels[l - 1];
            let trigger_row = popup.levels[l]
                .parent
                .and_then(|r| parent.row_rects.get(r))
                .copied()
                .unwrap_or(parent.rect);
            crate::ui::place_submenu(
                window,
                (w, content),
                crate::ui::SubmenuPlacement {
                    parent_card: parent.rect,
                    trigger_row,
                    margin: MARGIN,
                    overlap: 2.0,
                    pad_y: POPUP_PAD,
                },
            )
        };
        let level = &mut popup.levels[l];
        level.rect = rect;
        level.content = content;
        level.scroll =
            crate::ui::ScrollMetrics::new(rect.h, content, level.scroll).clamp(level.scroll);
        let mut top = rect.y + POPUP_PAD - drawn_scroll(level.scroll);
        level.row_rects = level
            .rows
            .iter()
            .map(|row| {
                let row_rect = Rect::new(rect.x, top, rect.w, row_height(row, row_base));
                top += row_rect.h;
                row_rect
            })
            .collect();
        draw_level(app, frame, level, workspace, popup.app_icon.as_ref());
    }
    if let Some(menu) = app.overlays.context_menu.as_mut() {
        menu.popup = Some(popup);
    }
}

/// 一级菜单需要的宽度和内容高度（逻辑像素）。
fn measure(app: &App, frame: &mut Frame<'_>, rows: &[PopupRow], row_base: f32) -> (f32, f32) {
    let size = app.fonts.ui;
    let small = app.fonts.ui_small;
    let mut w: f32 = 0.0;
    let mut h = 2.0 * POPUP_PAD;
    let lead = CHECK_COLUMN + icon_column(rows);
    for row in rows {
        h += row_height(row, row_base);
        let text = match row {
            PopupRow::Separator => continue,
            PopupRow::Heading(text) => frame.measure(FontKind::Ui, small, text),
            PopupRow::Item { label, key, .. } => {
                let title = frame.measure(FontKind::Ui, size, &label.title);
                let subtitle = label
                    .subtitle
                    .as_deref()
                    .map_or(0.0, |s| frame.measure(FontKind::Ui, small, s));
                let key = if key.is_empty() {
                    0.0
                } else {
                    KEY_GAP + frame.measure(FontKind::Ui, small, key)
                };
                title.max(subtitle) + key
            }
            PopupRow::Submenu { label, .. } => {
                frame.measure(FontKind::Ui, size, &label.title) + KEY_GAP
            }
            PopupRow::Entry { name, dir, .. } => {
                let arrow = if *dir { KEY_GAP } else { 0.0 };
                frame.measure(FontKind::Ui, size, name) + arrow
            }
            PopupRow::Query(query) => frame.measure(FontKind::Ui, size, query),
        };
        w = w.max(lead + text + RIGHT_PAD);
    }
    (w.clamp(WIDTH.0, WIDTH.1), h)
}

/// 图标那一列的宽度：有一行带图标时为 [`ICON_COLUMN`]，否则为 0，文字紧接打勾的一列。
fn icon_column(rows: &[PopupRow]) -> f32 {
    let any = rows.iter().any(|row| {
        matches!(
            row,
            PopupRow::Item { icon: Some(_), .. }
                | PopupRow::Submenu { icon: Some(_), .. }
                | PopupRow::Entry { .. }
        )
    });
    if any { ICON_COLUMN } else { 0.0 }
}

/// 一行的高度。
fn row_height(row: &PopupRow, row_base: f32) -> f32 {
    match row {
        PopupRow::Separator => SEPARATOR,
        PopupRow::Heading(_) => 20.0,
        PopupRow::Item { label, .. } if label.subtitle.is_some() => ROW_TALL,
        _ => row_base,
    }
}

/// 打勾的那一行的中线离菜单顶边的距离；没有打勾的行时为 `None`。
fn checked_offset(rows: &[PopupRow]) -> Option<f32> {
    let mut top = POPUP_PAD;
    for row in rows {
        if matches!(
            row,
            PopupRow::Item { checked: true, .. } | PopupRow::Entry { checked: true, .. }
        ) {
            return Some(top + row_height(row, ROW) / 2.0);
        }
        top += row_height(row, ROW);
    }
    None
}

/// 使用 [`crate::ui`] 声明式布局绘制一级菜单：卡片、可见行切片、悬停底色、勾、文字、快捷键和子菜单箭头。
fn draw_level(
    app: &App,
    frame: &mut Frame<'_>,
    level: &PopupLevel,
    workspace: bool,
    app_icon: Option<&winc_preview::image::Pixels>,
) {
    let p = app.palette;
    let card = level.rect;
    let clip = card.inset(0.0, POPUP_PAD);
    let icons = icon_column(&level.rows);
    let (size, small) = (app.fonts.ui, app.fonts.ui_small);
    let basis = TextBasis {
        ui_size: size,
        x_height: app.metrics.ui.x_height,
        cap_height: app.metrics.ui.cap_height,
    };

    let visible: Vec<usize> = level
        .row_rects
        .iter()
        .enumerate()
        .filter_map(|(i, rect)| (rect.bottom() >= clip.y && rect.y <= clip.bottom()).then_some(i))
        .collect();
    let top_scroll = visible
        .first()
        .map_or(0.0, |&first| clip.y - level.row_rects[first].y);

    let mut row_nodes: Vec<Node<'_, ()>> = Vec::with_capacity(visible.len());
    for i in visible {
        let row = &level.rows[i];
        let row_h = level.row_rects[i].h;
        let (title, subtitle, icon, checked, enabled, key, submenu) = match row {
            PopupRow::Heading(label) => {
                row_nodes.push(
                    ui::hstack()
                        .w_full()
                        .h(row_h)
                        .flex_shrink(0.0)
                        .pl(WORKSPACE_INSET)
                        .items_center()
                        .child(
                            ui::text(label.as_str())
                                .text_size(small)
                                .color(p.text_dim)
                                .flex_shrink(0.0),
                        ),
                );
                continue;
            }
            PopupRow::Separator => {
                row_nodes.push(
                    ui::box_()
                        .w_full()
                        .h(row_h)
                        .flex_shrink(0.0)
                        .px(ROW_INSET + 6.0)
                        .justify_center()
                        .child(ui::divider(p.separator)),
                );
                continue;
            }
            PopupRow::Query(query) => {
                // 键入的文字：搜索图标放在打勾的一列，文字与各行的名称对齐，下面一条分隔线。
                let search_slot = ui::custom(
                    (CHECK_COLUMN + icons, ROW.min(row_h - 1.0).max(0.0)),
                    move |frame, slot, clip| {
                        let cy = icon_center_y(app, slot, size);
                        icons::search(frame, (slot.x + 13.0, cy), 0.75, p.text_muted, clip);
                    },
                );
                row_nodes.push(
                    ui::vstack()
                        .w_full()
                        .h(row_h)
                        .flex_shrink(0.0)
                        .child(
                            ui::hstack()
                                .w_full()
                                .flex_1()
                                .items_center()
                                .child(search_slot)
                                .child(
                                    ui::text(query.as_str())
                                        .text_size(size)
                                        .color(p.text)
                                        .flex_shrink(0.0),
                                ),
                        )
                        .child(
                            ui::box_()
                                .w_full()
                                .h(1.0)
                                .px(ROW_INSET + 6.0)
                                .flex_shrink(0.0)
                                .child(ui::divider(p.separator)),
                        ),
                );
                continue;
            }
            PopupRow::Item {
                label,
                icon,
                key,
                enabled,
                checked,
                ..
            } => (
                label.title.as_str(),
                label.subtitle.as_deref(),
                *icon,
                *checked,
                *enabled,
                if workspace { "" } else { key.as_str() },
                false,
            ),
            PopupRow::Submenu {
                label,
                icon,
                checked,
                ..
            } => (label.title.as_str(), None, *icon, *checked, true, "", true),
            PopupRow::Entry {
                name, dir, checked, ..
            } => {
                let icon = if *dir {
                    MenuIcon::Folder
                } else {
                    let (glyph, colors) = crate::theme::file_kind(name);
                    MenuIcon::File(glyph, colors)
                };
                (name.as_str(), None, Some(icon), *checked, true, "", *dir)
            }
        };

        let hovered = level.hover == Some(i);
        let current_path = matches!(icon, Some(MenuIcon::NotebookFolder { current: true }));
        let (ink, dim) = if hovered && !workspace {
            band_ink(p)
        } else if current_path {
            (p.text_muted, p.text_dim)
        } else if enabled {
            (p.text, p.text_dim)
        } else {
            (p.text_dim, p.text_dim)
        };
        let title_h = if workspace { WORKSPACE_ROW } else { ROW };
        let leading_icon = icon.filter(|icon| *icon != MenuIcon::ICloud);
        let lead_w = if workspace {
            WORKSPACE_INSET
                + if leading_icon.is_some() {
                    ICON_COLUMN
                } else {
                    0.0
                }
        } else {
            CHECK_COLUMN + icons
        };
        let right_pad = if workspace {
            WORKSPACE_INSET
        } else {
            RIGHT_PAD
        };
        let mut room = card.w - lead_w - right_pad;
        if !key.is_empty() {
            let w = frame.measure(FontKind::Ui, small, key);
            room -= w + KEY_GAP / 2.0;
        }
        if submenu {
            room -= KEY_GAP / 2.0;
        }
        let icloud_w = if icon == Some(MenuIcon::ICloud) {
            let width = frame.measure(FontKind::Ui, size, "iCloud");
            room = (card.w - right_pad - width - 18.0 - lead_w - 24.0).max(0.0);
            Some(width)
        } else {
            None
        };
        let shown_title = if workspace {
            truncate_end(frame, title, size, room.max(0.0)).into_owned()
        } else {
            ellipsize(frame, title, size, room.max(0.0)).into_owned()
        };
        let shown_subtitle =
            subtitle.map(|sub| ellipsize(frame, sub, small, room.max(0.0)).into_owned());

        let band_title_h = (title_h - 2.0).max(0.0);
        let lead_slot = ui::custom(
            ((lead_w - ROW_INSET).max(0.0), band_title_h),
            move |frame, slot, clip| {
                let row_ref = Rect::new(slot.x - ROW_INSET, slot.y - 1.0, slot.w, title_h);
                let cy = icon_center_y(app, row_ref, size);
                if checked {
                    icons::check(frame, (row_ref.x + 13.0, cy), 9.0, ink, clip);
                }
                if let Some(icon) = leading_icon {
                    let (w, h) = icon.size();
                    let x = if workspace {
                        WORKSPACE_INSET
                    } else {
                        CHECK_COLUMN + (ICON_COLUMN - 6.0 - w) / 2.0
                    };
                    let at = (row_ref.x + x, cy - h / 2.0);
                    if icon == MenuIcon::Typora
                        && let Some(pixels) = app_icon
                    {
                        frame.pixels_scaled(
                            Rect::new(at.0, at.1, w, h),
                            pixels.width,
                            pixels.height,
                            &pixels.rgba,
                            clip,
                        );
                    } else {
                        app.draw_menu_icon(frame, icon, at, clip);
                    }
                }
            },
        );

        let text_col = if let Some(shown_subtitle) = shown_subtitle {
            ui::vstack()
                .flex_1()
                .h_full()
                .justify_center()
                .child(
                    ui::hstack().w_full().h(ROW - 6.0).items_center().child(
                        ui::text(shown_title)
                            .text_size(size)
                            .color(ink)
                            .flex_shrink(0.0),
                    ),
                )
                .child(
                    ui::hstack().w_full().h(ROW - 8.0).items_center().child(
                        ui::text(shown_subtitle)
                            .text_size(small)
                            .color(dim)
                            .flex_shrink(0.0),
                    ),
                )
        } else {
            ui::hstack().flex_1().h_full().items_center().child(
                ui::text(shown_title)
                    .text_size(size)
                    .color(ink)
                    .flex_shrink(0.0),
            )
        };

        let mut band = ui::hstack()
            .w_full()
            .h_full()
            .pr((right_pad - ROW_INSET).max(0.0))
            .rounded(RADIUS_ROW)
            .child(lead_slot)
            .child(text_col);
        if hovered {
            band = band.bg(if workspace { p.hover } else { p.accent_fill });
        }
        if !key.is_empty() {
            band = band.child(
                ui::hstack()
                    .h(band_title_h)
                    .items_center()
                    .flex_shrink(0.0)
                    .child(ui::text(key).text_size(small).color(dim).flex_shrink(0.0)),
            );
        }
        if submenu {
            band = band.child(ui::custom((8.0, band_title_h), move |frame, slot, clip| {
                let row_ref = Rect::new(slot.x, slot.y - 1.0, slot.w, title_h);
                let cy = icon_center_y(app, row_ref, size);
                icons::chevron(
                    frame,
                    (slot.right() - 3.0, cy),
                    6.0,
                    Direction::Right,
                    dim,
                    clip,
                );
            }));
        }
        if let Some(provider_w) = icloud_w {
            band = band.child(ui::custom(
                (provider_w + 36.0, band_title_h),
                move |frame, slot, clip| {
                    let row_ref = Rect::new(slot.x, slot.y - 1.0, slot.w, title_h);
                    let at = slot.right() - provider_w - 18.0;
                    let cy = icon_center_y(app, row_ref, size);
                    app.draw_menu_icon(frame, MenuIcon::ICloud, (at - 18.0, cy - 6.0), clip);
                    ui_text(frame, app, at, row_ref, "iCloud", p.text_muted, clip);
                    let x = slot.right() - 2.0;
                    let strokes = [
                        winc_render::Stroke::Line(x - 5.0, cy + 2.5, x, cy - 2.5),
                        winc_render::Stroke::Line(x - 5.0, cy - 2.5, x, cy - 2.5),
                        winc_render::Stroke::Line(x, cy - 2.5, x, cy + 2.5),
                    ];
                    frame.stroke(&strokes, 1.1, p.text_dim, clip);
                },
            ));
        }

        row_nodes.push(
            ui::box_()
                .w_full()
                .h(row_h)
                .flex_shrink(0.0)
                .px(ROW_INSET)
                .py(1.0)
                .child(band),
        );
    }

    let tree = ui::vstack()
        .w(card.w)
        .h(card.h)
        .py(POPUP_PAD)
        .bg(p.popup_bg)
        .rounded(RADIUS_POPUP)
        .card_frame(CardFrame {
            shadow: p.shadow,
            border_base: p.popup_bg,
            border: p.popup_border,
        })
        .child(
            ui::vstack()
                .w_full()
                .flex_1()
                .scroll_y(top_scroll)
                .children(row_nodes),
        );
    ui::render(frame, basis, card, tree);
}
