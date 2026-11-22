//! Compact property editors anchored to the value that opened them.

use super::*;
use crate::app::Focus;
use crate::app::git::branch::picker::PickerPart;
use crate::app::preview::markdown::properties::popup::{
    CalendarPart, ChoiceRow, PropertyPopup, first_weekday, today,
};
use crate::layout::PickerParts;
use winc_preview::markdown::properties::{Kind, date_parts, days_in_month};

pub(super) fn draw(app: &mut App, frame: &mut Frame<'_>) {
    let rect = app.layout.picker;
    app.hits.picker_parts.clear();
    if rect.w <= 0.0 || rect.h <= 0.0 {
        return;
    }
    let Some(popup) = &app.overlays.picker.property else {
        return;
    };
    let kind = popup.kind();
    let parts = popup.parts(rect);
    let p = app.palette;
    draw_card(frame, p, rect, 8.0, p.popup_bg);

    let shell = parts.bar.inset(6.0, 4.0);
    frame.rect_clipped(
        shell,
        p.text.alpha(if p.dark { 0.075 } else { 0.047 }),
        5.0,
        rect,
    );
    let submit = if kind.is_choice() {
        0.0
    } else {
        29.0_f32.min(shell.w * 0.24)
    };
    let field = Rect::new(
        shell.x + 10.0,
        shell.y,
        (shell.w - 19.0 - submit).max(0.0),
        shell.h,
    );
    let placeholder = if kind.is_choice() {
        t!("properties-choice-placeholder")
    } else {
        kind.sample()
    };
    let mut input = std::mem::take(&mut app.overlays.picker.input);
    if let Some(caret) = draw_bar_field(
        app,
        frame,
        field,
        &mut input,
        placeholder,
        app.focus == Focus::Picker,
    ) {
        app.ime.cursor = Some(caret);
    }
    app.overlays.picker.input = input;
    if submit > 0.0 {
        let button = Rect::new(shell.right() - submit, shell.y, submit, shell.h);
        let part = PickerPart::PropertyApply;
        if app.pointer.hover == Hover::PickerPart(part) {
            highlight(frame, p, button, rect);
        }
        centered(frame, app, button, "↵", p.text_dim, rect);
        app.hits.picker_parts.push((button, part));
    }

    let Some(popup) = &app.overlays.picker.property else {
        return;
    };
    if kind.is_choice() || kind == Kind::Date {
        frame.rect_clipped(
            Rect::new(rect.x + 1.0, parts.bar.bottom(), rect.w - 2.0, 1.0),
            p.border,
            0.0,
            rect,
        );
    }
    if kind.is_choice() {
        draw_choices(app, frame, popup, parts);
    } else if kind == Kind::Date {
        app.hits
            .picker_parts
            .extend(draw_calendar(app, frame, popup, parts));
    }
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
    let heading = parts.list;
    let p = app.palette;
    let area = Rect::new(
        heading.x + 8.0,
        heading.y + 5.0,
        (heading.w - 16.0).max(0.0),
        (heading.h - 5.0).max(0.0),
    );
    let (year, month) = popup.month;
    let current = date_parts(app.overlays.picker.input.text().trim());
    let local_today = today();
    let row_h = (area.h / 9.0).min(27.0);
    let day_w = area.w / 7.0;
    let cell = |col: u32, row: u32| {
        Rect::new(
            area.x + col as f32 * day_w,
            area.y + row as f32 * row_h,
            day_w,
            row_h,
        )
    };
    let mut hits = Vec::with_capacity(35);
    let mut button = |frame: &mut Frame<'_>,
                      rect: Rect,
                      text: &str,
                      part: CalendarPart,
                      selected: bool,
                      is_today: bool| {
        let part = PickerPart::PropertyCalendar(part);
        let paint = rect.inset(2.0, 1.0);
        if selected {
            frame.rect_clipped(
                paint,
                p.accent.alpha(if p.dark { 0.24 } else { 0.17 }),
                5.0,
                area,
            );
        } else if app.pointer.hover == Hover::PickerPart(part) {
            highlight(frame, p, paint, area);
        }
        centered(
            frame,
            app,
            rect,
            text,
            if selected || is_today {
                p.accent
            } else {
                p.text
            },
            area,
        );
        if rect.w > 0.0 && rect.h > 0.0 {
            hits.push((rect, part));
        }
    };
    let month_label = t!(
        "properties-calendar-month",
        year = format!("{year:04}"),
        month = format!("{month:02}")
    );
    let top = Rect::new(area.x, area.y, day_w * 4.0, row_h);
    ui_text(frame, app, top.x + 5.0, top, &month_label, p.text, top);
    button(
        frame,
        cell(4, 0),
        t!("properties-today"),
        CalendarPart::Today,
        false,
        false,
    );
    button(frame, cell(5, 0), "‹", CalendarPart::Previous, false, false);
    button(frame, cell(6, 0), "›", CalendarPart::Next, false, false);
    for (col, label) in t!("properties-weekdays").split('|').enumerate() {
        centered(frame, app, cell(col as u32, 1), label, p.text_dim, area);
    }
    let start = first_weekday(year, month);
    for day in 1..=days_in_month(year, month) {
        let i = start + day - 1;
        button(
            frame,
            cell(i % 7, 2 + i / 7),
            &day.to_string(),
            CalendarPart::Day(day),
            current == Some((year, month, day)),
            local_today == (year, month, day),
        );
    }
    frame.rect_clipped(
        Rect::new(area.x, cell(0, 8).y, area.w, 1.0),
        p.border,
        0.0,
        area,
    );
    button(
        frame,
        Rect::new(area.x, cell(0, 8).y, day_w * 2.0, row_h),
        t!("properties-clear"),
        CalendarPart::Clear,
        false,
        false,
    );
    hits
}
