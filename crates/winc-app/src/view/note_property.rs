//! Compact property editors anchored to the value that opened them.

use super::*;
use crate::app::Focus;
use crate::app::git::branch::picker::PickerPart;
use crate::app::preview::markdown::properties::popup::{
    CALENDAR_FOOTER, CALENDAR_HEADER, CALENDAR_PAD, CALENDAR_WEEK, CALENDAR_WEEKDAYS, CalendarPart,
    ChoiceRow, PropertyInput, PropertyPopup, first_weekday, today,
};
use crate::layout::PickerParts;
use winc_preview::markdown::properties::{date_parts, days_in_month};
use winc_render::Stroke;

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
        let shell = if source {
            anchor
        } else {
            anchor.inset(0.0, 2.0)
        };
        let p = app.palette;
        frame.rect_clipped(
            shell,
            p.accent.alpha(if source { 0.42 } else { 0.75 }),
            6.0,
            anchor,
        );
        let inside = shell.inset(1.0, 1.0);
        frame.rect_clipped(inside, p.editor_bg, 5.0, anchor);
        if source {
            frame.rect_clipped(inside, p.group_bg, 5.0, anchor);
        }
        let field = if source {
            inside.inset(9.0, 4.0)
        } else {
            inside.inset(4.0, 0.0)
        };
        let mut text = std::mem::take(&mut app.overlays.picker.input);
        let focused = app.focus == Focus::Picker;
        app.ime.cursor = if input == PropertyInput::Source {
            let wrap = super::fields::wrap_area_field_font(
                app,
                frame,
                field.w,
                &mut text,
                focused,
                FontKind::Mono,
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
    let popup = app.overlays.picker.property.as_ref().unwrap();
    let parts = popup.parts(rect);
    if calendar {
        app.hits
            .picker_parts
            .extend(draw_calendar(app, frame, popup, parts));
        return;
    }
    let shell = parts.bar.inset(6.0, 4.0);
    frame.rect_clipped(
        shell,
        p.text.alpha(if p.dark { 0.075 } else { 0.047 }),
        5.0,
        rect,
    );
    let field = shell.inset(9.0, 0.0);
    let mut text = std::mem::take(&mut app.overlays.picker.input);
    app.ime.cursor = draw_bar_field(
        app,
        frame,
        field,
        &mut text,
        placeholder,
        app.focus == Focus::Picker,
    );
    app.overlays.picker.input = text;
    frame.rect_clipped(
        Rect::new(rect.x + 1.0, parts.bar.bottom(), rect.w - 2.0, 1.0),
        p.border,
        0.0,
        rect,
    );
    draw_choices(
        app,
        frame,
        app.overlays.picker.property.as_ref().unwrap(),
        parts,
    );
}

fn highlight(frame: &mut Frame<'_>, palette: &Palette, r: Rect, clip: Rect) {
    frame.rect_clipped(
        r,
        palette.text.alpha(if palette.dark { 0.1 } else { 0.06 }),
        5.0,
        clip,
    );
}

fn centered(frame: &mut Frame<'_>, app: &App, rect: Rect, text: &str, color: Color, clip: Rect) {
    let width = frame.measure(FontKind::Ui, app.fonts.ui_small, text);
    ui_text_small(
        frame,
        app,
        rect.x + (rect.w - width) * 0.5,
        rect,
        text,
        color,
        clip,
    );
}

fn draw_choices(app: &App, frame: &mut Frame<'_>, popup: &PropertyPopup, parts: PickerParts) {
    let p = app.palette;
    let picker = &app.overlays.picker;
    ui_text_small(
        frame,
        app,
        parts.actions.x + 12.0,
        parts.actions,
        t!("properties-choice-caption"),
        p.text_dim,
        parts.actions,
    );
    if popup.rows.is_empty() {
        ui_text_small(
            frame,
            app,
            parts.list.x + 12.0,
            parts.list,
            t!("properties-choice-empty"),
            p.text_dim,
            parts.list,
        );
    }
    for i in parts.visible(picker.scroll, popup.rows.len()) {
        let row = parts.row(i, picker.scroll).inset(5.0, 1.0);
        let clip = row.intersect(parts.list);
        if i == picker.selected || app.pointer.hover == Hover::PickerRow(i) {
            highlight(frame, p, row, parts.list);
        }
        let flag = Rect::new(row.right() - 29.0, row.y, 24.0, row.h);
        match popup.rows[i] {
            ChoiceRow::Option(index) => {
                let label = &popup.options[index];
                let (fg, bg) = winc_typeset::property_chip_colors(label, p.dark);
                let width = frame.measure(FontKind::Ui, app.fonts.ui, label) + 14.0;
                let chip = Rect::new(
                    row.x + 6.0,
                    row.y + 3.0,
                    width.min((row.w - 40.0).max(0.0)),
                    (row.h - 6.0).max(0.0),
                );
                frame.rect_clipped(chip, bg, 4.0, clip);
                ui_text(
                    frame,
                    app,
                    chip.x + 7.0,
                    chip,
                    label,
                    fg,
                    clip.intersect(chip.inset(3.0, 0.0)),
                );
                if popup.chosen.contains(label) {
                    centered(frame, app, flag, "✓", p.accent, clip);
                }
            }
            ChoiceRow::Create => {
                let text = t!(
                    "properties-choice-create",
                    name = picker.input.text().trim().to_owned()
                );
                ui_text(frame, app, row.x + 8.0, row, &text, p.text, clip);
            }
            ChoiceRow::Clear => {
                ui_text_small(
                    frame,
                    app,
                    row.x + 8.0,
                    row,
                    t!("properties-clear"),
                    p.text_dim,
                    clip,
                );
            }
        }
    }
}

fn draw_calendar(
    app: &App,
    frame: &mut Frame<'_>,
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
    let day_w = area.w / 7.0;
    let header = Rect {
        h: header_h,
        ..area
    };
    let grid_y = header.bottom() + weekday_h;
    let cell = |col: u32, top: f32, height: f32| {
        Rect::new(area.x + col as f32 * day_w, top, day_w, height)
    };
    let mut hits = Vec::with_capacity(35);
    let mut button = |frame: &mut Frame<'_>,
                      rect: Rect,
                      text: &str,
                      part: CalendarPart,
                      selected: bool,
                      is_today: bool| {
        if rect.w <= 0.0 || rect.h <= 0.0 {
            return;
        }
        let part = PickerPart::PropertyCalendar(part);
        let hovered = app.pointer.hover == Hover::PickerPart(part);
        let paint = rect.inset(2.0, 1.0);
        if selected {
            frame.rect_clipped(paint, p.accent_fill, 5.0, area);
        } else if hovered {
            highlight(frame, p, paint, area);
        }
        let ink = if selected {
            p.text_on_accent
        } else if is_today {
            p.accent
        } else {
            p.text
        };
        match part {
            PickerPart::PropertyCalendar(CalendarPart::Previous | CalendarPart::Next) => {
                let x = (rect.x + rect.w * 0.5).round();
                let y = (rect.y + rect.h * 0.5).round();
                let dx = if part == PickerPart::PropertyCalendar(CalendarPart::Previous) {
                    -2.0
                } else {
                    2.0
                };
                frame.stroke(
                    &[
                        Stroke::Line(x - dx, y - 4.0, x + dx, y),
                        Stroke::Line(x + dx, y, x - dx, y + 4.0),
                    ],
                    1.35,
                    ink,
                    area,
                );
            }
            _ => centered(frame, app, rect, text, ink, area),
        }
        hits.push((rect, part));
    };
    let month_label = t!(
        "properties-calendar-month",
        year = format!("{year:04}"),
        month = format!("{month:02}")
    );
    let month_box = Rect {
        w: day_w * 4.0,
        ..header
    };
    ui_text(
        frame,
        app,
        month_box.x + 5.0,
        month_box,
        &month_label,
        p.text,
        month_box,
    );
    button(
        frame,
        cell(4, header.y, header.h),
        t!("properties-today"),
        CalendarPart::Today,
        false,
        true,
    );
    button(
        frame,
        cell(5, header.y, header.h),
        "",
        CalendarPart::Previous,
        false,
        false,
    );
    button(
        frame,
        cell(6, header.y, header.h),
        "",
        CalendarPart::Next,
        false,
        false,
    );
    for (col, label) in t!("properties-weekdays").split('|').enumerate() {
        centered(
            frame,
            app,
            cell(col as u32, header.bottom(), weekday_h),
            label,
            p.text_muted,
            area,
        );
    }
    let start = first_weekday(year, month);
    for day in 1..=days_in_month(year, month) {
        let i = start + day - 1;
        let rect = cell(i % 7, grid_y + (i / 7) as f32 * week_h, week_h);
        button(
            frame,
            rect,
            &day.to_string(),
            CalendarPart::Day(day),
            selected_date == Some((year, month, day)),
            local_today == (year, month, day),
        );
    }
    let footer_y = grid_y + popup.calendar_weeks() as f32 * week_h;
    frame.rect_clipped(
        Rect::new(area.x, footer_y + 2.0, area.w, 1.0),
        p.separator,
        0.0,
        area,
    );
    let clear = Rect::new(
        area.right() - day_w * 2.0,
        footer_y + 4.0,
        day_w * 2.0,
        (CALENDAR_FOOTER * scale - 4.0).max(0.0),
    );
    button(
        frame,
        clear,
        t!("properties-clear"),
        CalendarPart::Clear,
        false,
        false,
    );
    hits
}
