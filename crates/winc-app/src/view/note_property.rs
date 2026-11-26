//! Compact property editors anchored to the value that opened them.
//!
//! 选择列表与日期日历浮层使用 [`crate::ui`] 声明式 Flexbox 布局构建，内嵌既有 `TextField`
//! 输入槽，并自动产出日历按钮的命中区域。

use super::*;
use crate::app::Focus;
use crate::app::git::branch::picker::PickerPart;
use crate::app::preview::markdown::properties::popup::{
    CALENDAR_FOOTER, CALENDAR_HEADER, CALENDAR_PAD, CALENDAR_WEEK, CALENDAR_WEEKDAYS, CHOICE_ROW,
    CalendarPart, ChoiceRow, PropertyInput, PropertyPopup, first_weekday, today,
};
use crate::layout::PickerParts;
use crate::ui::widgets;
use crate::ui::{self, Node, TextAlign, TextBasis};
use winc_preview::markdown::properties::{date_parts, days_in_month};

pub(super) fn draw(app: &mut App, frame: &mut Frame<'_>) {
    let rect = app.layout.picker;
    app.hits.picker_parts.clear();
    if rect.w <= 0.0 || rect.h <= 0.0 {
        return;
    }
    let Some(popup) = &app.overlays.picker.property else {
        return;
    };
    let (input, anchor, choices, calendar) =
        (popup.input, popup.anchor, popup.choices(), popup.calendar());
    let placeholder = match input {
        PropertyInput::Name => t!("properties-name-placeholder"),
        PropertyInput::ListItem => t!("properties-item-placeholder"),
        PropertyInput::Source => "",
        PropertyInput::Value if choices => t!("properties-choice-placeholder"),
        PropertyInput::Value => popup.kind().sample(),
    };
    if !choices {
        let source = input == PropertyInput::Source;
        let field = if source {
            anchor.inset(10.0, 5.0)
        } else {
            let p = app.palette;
            let shell = anchor.inset(0.0, 2.0);
            frame.rect_clipped(shell, p.accent.alpha(0.75), 6.0, anchor);
            let inside = shell.inset(1.0, 1.0);
            frame.rect_clipped(inside, p.editor_bg, 5.0, anchor);
            inside.inset(4.0, 0.0)
        };
        let mut text = std::mem::take(&mut app.overlays.picker.input);
        let focused = app.focus == Focus::Picker;
        app.ime.cursor = if source {
            let base = (app.fonts.code * crate::app::preview::markdown::laid::BASE_RATIO).round();
            let size = base * 0.83;
            let wrap = super::fields::wrap_area_field_font(
                app,
                frame,
                field.w,
                &mut text,
                focused,
                FontKind::Mono,
                size,
            );
            super::fields::draw_area_field(app, frame, field, &mut text, wrap, placeholder, focused)
        } else {
            draw_bar_field(app, frame, field, &mut text, placeholder, focused)
        };
        app.overlays.picker.input = text;
        if !calendar {
            return;
        }
    }

    let p = app.palette;
    draw_card(frame, p, rect, 8.0, p.popup_bg);
    let basis = ui::text_basis(app.fonts.ui, app.metrics.ui);
    let parts = app
        .overlays
        .picker
        .property
        .as_ref()
        .expect("property popup is open")
        .parts(rect);

    if calendar {
        let popup = app
            .overlays
            .picker
            .property
            .as_ref()
            .expect("property popup is open");
        let hits = render_calendar(app, frame, basis, popup, parts);
        app.hits.picker_parts.extend(hits);
        return;
    }

    render_choices_card(app, frame, basis, rect, parts, placeholder);
}

fn hover_tint(p: &Palette) -> Color {
    p.text.alpha(if p.dark { 0.1 } else { 0.06 })
}

/// 用声明式布局绘制单选/多选属性浮层：顶部搜索输入槽、分割线、说明栏与可滚动选项列表。
fn render_choices_card(
    app: &mut App,
    frame: &mut Frame<'_>,
    basis: TextBasis,
    rect: Rect,
    parts: PickerParts,
    placeholder: &str,
) {
    let p = app.palette;
    let ui_size = app.fonts.ui;
    let small = app.fonts.ui_small;
    let focused = app.focus == Focus::Picker;
    let shell_bg = p.text.alpha(if p.dark { 0.075 } else { 0.047 });

    // 先从 `app` 拆出 `TextField` 所有权，以便安全嵌入声明式 `field_slot` 回调。
    let mut input = std::mem::take(&mut app.overlays.picker.input);
    let mut ime_cursor = app.ime.cursor;
    let input_text = input.text().trim().to_owned();

    let (selected, scroll, hover, rows, options, chosen) = {
        let picker = &app.overlays.picker;
        let popup = picker.property.as_ref().expect("property popup is open");
        (
            picker.selected,
            picker.scroll,
            app.pointer.hover,
            popup.rows.clone(),
            popup.options.clone(),
            popup.chosen.clone(),
        )
    };

    let list_node = build_choices_list(
        p,
        (ui_size, small),
        parts,
        (selected, scroll, hover),
        &rows,
        &options,
        &chosen,
        &input_text,
    );

    let bar_h = parts.bar.h;
    let card_clip = rect;
    let search_bar = widgets::field_slot(bar_h, |frame, bar_rect, _| {
        let shell = bar_rect.inset(6.0, 4.0);
        frame.rect_clipped(shell, shell_bg, 5.0, card_clip);
        let field = shell.inset(9.0, 0.0);
        ime_cursor = draw_bar_field(app, frame, field, &mut input, placeholder, focused);
    });

    let caption_bar = ui::hstack()
        .w_full()
        .h(parts.actions.h)
        .px(12.0)
        .items_center()
        .flex_shrink(0.0)
        .child(
            ui::text(t!("properties-choice-caption"))
                .text_size(small)
                .line_height(parts.actions.h)
                .color(p.text_dim),
        );

    let sep_line = ui::hstack()
        .w_full()
        .px(1.0)
        .flex_shrink(0.0)
        .child(ui::divider(p.border));

    let pad_top = (parts.bar.y - rect.y).max(0.0);
    let tree: Node<'_, ()> = ui::vstack()
        .w_full()
        .h_full()
        .pt(pad_top)
        .child(search_bar)
        .child(sep_line)
        .child(caption_bar)
        .child(list_node);

    let _ = ui::render(frame, basis, rect, tree);
    app.overlays.picker.input = input;
    app.ime.cursor = ime_cursor;
}

/// 构造虚拟滚动的选项列表子树：只生成可见区间的行节点，行位置与 `PickerParts::row` 严格对齐。
#[allow(clippy::too_many_arguments)]
fn build_choices_list<'a>(
    p: &Palette,
    (ui_size, small): (f32, f32),
    parts: PickerParts,
    (selected, scroll, hover): (usize, f32, Hover),
    rows: &[ChoiceRow],
    options: &'a [String],
    chosen: &[String],
    input_text: &str,
) -> Node<'a, ()> {
    let list_h = parts.list.h.max(0.0);
    if rows.is_empty() {
        return ui::vstack()
            .w_full()
            .h(list_h)
            .px(12.0)
            .overflow_hidden()
            .child(
                ui::text(t!("properties-choice-empty"))
                    .text_size(small)
                    .line_height(CHOICE_ROW)
                    .color(p.text_dim),
            );
    }

    let window = ui::VisibleWindow::fixed(rows.len(), CHOICE_ROW, list_h, scroll);
    let highlight_bg = hover_tint(p);
    let chip_h = (CHOICE_ROW - 8.0).max(0.0);

    ui::virtual_list(window, |i| {
        let active = i == selected || hover == Hover::PickerRow(i);
        let inner = match rows[i] {
            ChoiceRow::Option(index) => {
                let label = &options[index];
                let (fg, bg) = winc_typeset::property_chip_colors(label, p.dark);
                let checked = chosen.contains(label);
                ui::hstack()
                    .w_full()
                    .h_full()
                    .pl(6.0)
                    .pr(5.0)
                    .items_center()
                    .justify_between()
                    .gap(6.0)
                    .rounded(5.0)
                    .when(active, |n| n.bg(highlight_bg))
                    .child(widgets::chip(label.as_str(), fg, bg, ui_size, chip_h))
                    .when(checked, |n| {
                        n.child(
                            ui::text("✓")
                                .w(24.0)
                                .flex_shrink(0.0)
                                .text_size(small)
                                .line_height(CHOICE_ROW - 2.0)
                                .color(p.accent)
                                .text_align(TextAlign::Center),
                        )
                    })
            }
            ChoiceRow::Create => {
                let label = t!("properties-choice-create", name = input_text.to_owned());
                ui::hstack()
                    .w_full()
                    .h_full()
                    .px(8.0)
                    .items_center()
                    .rounded(5.0)
                    .overflow_hidden()
                    .when(active, |n| n.bg(highlight_bg))
                    .child(
                        ui::text(label)
                            .min_w(0.0)
                            .text_size(ui_size)
                            .line_height(CHOICE_ROW - 2.0)
                            .color(p.text),
                    )
            }
            ChoiceRow::Clear => ui::hstack()
                .w_full()
                .h_full()
                .px(8.0)
                .items_center()
                .rounded(5.0)
                .when(active, |n| n.bg(highlight_bg))
                .child(
                    ui::text(t!("properties-clear"))
                        .text_size(small)
                        .line_height(CHOICE_ROW - 2.0)
                        .color(p.text_dim),
                ),
        };

        ui::hstack()
            .w_full()
            .h(CHOICE_ROW)
            .px(5.0)
            .py(1.0)
            .flex_shrink(0.0)
            .child(inner)
    })
    .h(list_h)
}

/// 用声明式 7 列 Flexbox 网格绘制日期选择器，并返回自动收集的按钮命中列表。
fn render_calendar(
    app: &App,
    frame: &mut Frame<'_>,
    basis: TextBasis,
    popup: &PropertyPopup,
    parts: PickerParts,
) -> Vec<(Rect, PickerPart)> {
    let p = app.palette;
    let area = parts.list.inset(8.0, CALENDAR_PAD);
    let (year, month) = popup.month;
    let selected_date = date_parts(app.overlays.picker.input.text().trim());
    let local_today = today();
    let scale = (area.h / (popup.calendar_height() - 2.0 * CALENDAR_PAD)).min(1.0);
    let header_h = CALENDAR_HEADER * scale;
    let weekday_h = CALENDAR_WEEKDAYS * scale;
    let week_h = CALENDAR_WEEK * scale;
    let footer_h = (CALENDAR_FOOTER * scale).max(0.0);
    let small = app.fonts.ui_small;
    let hover = app.pointer.hover;

    let cell_btn =
        |part: CalendarPart, weight: f32, content: Node<'static, PickerPart>, selected: bool| {
            let target = PickerPart::PropertyCalendar(part);
            let hovered = hover == Hover::PickerPart(target);
            let bg = if selected {
                Some(p.accent_fill)
            } else if hovered {
                Some(hover_tint(p))
            } else {
                None
            };
            ui::hstack()
                .flex_weight(weight)
                .h_full()
                .px(2.0)
                .py(1.0)
                .hit(target)
                .child(
                    ui::hstack()
                        .w_full()
                        .h_full()
                        .items_center()
                        .justify_center()
                        .rounded(5.0)
                        .when_some(bg, |n, color| n.bg(color))
                        .child(content),
                )
        };

    let cell_label = |label: String, ink: Color, height: f32| {
        ui::text(label)
            .w_full()
            .text_size(small)
            .line_height(height)
            .color(ink)
            .text_align(TextAlign::Center)
    };

    let month_label = t!(
        "properties-calendar-month",
        year = format!("{year:04}"),
        month = format!("{month:02}")
    );
    let header = ui::hstack()
        .w_full()
        .h(header_h)
        .flex_shrink(0.0)
        .child(
            ui::hstack()
                .flex_weight(4.0)
                .h_full()
                .pl(5.0)
                .items_center()
                .overflow_hidden()
                .child(
                    ui::text(month_label)
                        .min_w(0.0)
                        .text_size(app.fonts.ui)
                        .line_height(header_h)
                        .color(p.text),
                ),
        )
        .child(cell_btn(
            CalendarPart::Today,
            1.0,
            cell_label(t!("properties-today").to_owned(), p.accent, header_h),
            false,
        ))
        .child(cell_btn(
            CalendarPart::Previous,
            1.0,
            widgets::chevron(true, p.text),
            false,
        ))
        .child(cell_btn(
            CalendarPart::Next,
            1.0,
            widgets::chevron(false, p.text),
            false,
        ));

    let weekdays = ui::hstack()
        .w_full()
        .h(weekday_h)
        .flex_shrink(0.0)
        .children(t!("properties-weekdays").split('|').map(|label| {
            ui::hstack()
                .flex_1()
                .h_full()
                .items_center()
                .justify_center()
                .child(cell_label(label.to_owned(), p.text_muted, weekday_h))
        }));

    let start = first_weekday(year, month);
    let total_days = days_in_month(year, month);
    let weeks = (0..popup.calendar_weeks()).map(|week| {
        let cells = (0..7u32).map(|col| {
            let slot = week * 7 + col;
            if slot < start || slot >= start + total_days {
                return ui::box_().flex_1().h_full();
            }
            let day = slot - start + 1;
            let selected = selected_date == Some((year, month, day));
            let is_today = local_today == (year, month, day);
            let ink = if selected {
                p.text_on_accent
            } else if is_today {
                p.accent
            } else {
                p.text
            };
            cell_btn(
                CalendarPart::Day(day),
                1.0,
                cell_label(day.to_string(), ink, week_h),
                selected,
            )
        });
        ui::hstack()
            .w_full()
            .h(week_h)
            .flex_shrink(0.0)
            .children(cells)
    });

    let clear_h = (footer_h - 4.0).max(0.0);
    let footer = ui::vstack()
        .w_full()
        .h(footer_h)
        .pt(2.0)
        .gap(1.0)
        .flex_shrink(0.0)
        .child(ui::divider(p.separator))
        .child(
            ui::hstack()
                .w_full()
                .h(clear_h)
                .child(ui::spacer().flex_weight(5.0))
                .child(cell_btn(
                    CalendarPart::Clear,
                    2.0,
                    cell_label(t!("properties-clear").to_owned(), p.text, clear_h),
                    false,
                )),
        );

    let tree = ui::vstack()
        .w_full()
        .h_full()
        .child(header)
        .child(weekdays)
        .children(weeks)
        .child(footer);

    ui::render(frame, basis, area, tree).into_entries()
}
