//! Value editors share the picker's keyboard, input method and outside-click handling.

use super::*;
use crate::layout::{MENU_PICKER_BAR, MENU_PICKER_PAD, PickerParts};
use winc_render::Rect;

pub(crate) const CHOICE_ROW: f32 = 29.0;
pub(crate) const CHOICE_CAPTION: f32 = 25.0;
pub(crate) const CALENDAR_HEIGHT: f32 = 248.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalendarPart {
    Previous,
    Next,
    Today,
    Day(u32),
    Clear,
}

#[derive(Clone, Copy)]
pub(crate) enum ChoiceRow {
    Option(usize),
    Create,
    Clear,
}

pub(crate) struct PropertyPopup {
    edit: PropertyEdit,
    anchor: Rect,
    pub(crate) name: String,
    pub(crate) original: String,
    pub(crate) options: Vec<String>,
    pub(crate) chosen: Vec<String>,
    pub(crate) rows: Vec<ChoiceRow>,
    pub(crate) month: (i32, u32),
}

impl PropertyPopup {
    pub(crate) fn kind(&self) -> Kind {
        self.edit.kind
    }

    pub(crate) fn parts(&self, rect: Rect) -> PickerParts {
        let caption = if self.kind().is_choice() {
            CHOICE_CAPTION
        } else {
            0.0
        };
        let mut parts = PickerParts::menu(rect, caption, 0.0);
        parts.row_height = CHOICE_ROW;
        parts
    }
}

/// The weekday of the first of a Gregorian month (Sunday = 0).
pub(crate) fn first_weekday(year: i32, month: u32) -> u32 {
    let offsets = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = year - i32::from(month < 3);
    (y + y.div_euclid(4) - y.div_euclid(100) + y.div_euclid(400) + offsets[month as usize - 1] + 1)
        .rem_euclid(7) as u32
}

pub(crate) fn today() -> (i32, u32, u32) {
    let (y, m, d, ..) = crate::system::local_time();
    (
        y.clamp(1, 9999),
        m.clamp(1, 12) as u32,
        d.clamp(1, 31) as u32,
    )
}

fn step_month((year, month): (i32, u32), delta: i32) -> (i32, u32) {
    let index = (year * 12 + month as i32 - 1 + delta).clamp(0, 9999 * 12 + 11);
    (index / 12, (index % 12 + 1) as u32)
}

impl App {
    pub(in crate::app) fn open_property_value(&mut self, index: usize, anchor: Rect) {
        if !self.note_properties_available() {
            return;
        }
        let Some(doc) = self.editor.active() else {
            return;
        };
        let Some(field) = self.property_field(index).filter(|f| f.editable) else {
            return;
        };
        let kind = self.property_kind(&field);
        let edit = PropertyEdit {
            doc: doc.id,
            revision: doc.revision,
            field: Some(field.name.clone()),
            kind,
        };
        if kind == Kind::Checkbox {
            let next = if field.input == "true" {
                "false"
            } else {
                "true"
            };
            if !self.apply_property(&edit, &field.name, next, "") {
                self.flash(t!("properties-cannot-edit"));
            }
            return;
        }
        let chosen = if kind.is_choice() {
            properties::input_value(kind, &field.input)
                .map(|(_, labels)| labels)
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let mut options = self.property_options(&field.name);
        for selected in &chosen {
            if !options.contains(selected) {
                options.push(selected.clone());
            }
        }
        let (year, month, _) = properties::date_parts(&field.input).unwrap_or_else(today);
        self.start_property_picker(PropertyPopup {
            edit,
            anchor,
            name: field.name,
            original: field.input,
            options,
            chosen,
            rows: Vec::new(),
            month: (year, month),
        });
    }

    pub(in crate::app) fn refresh_property_picker(&mut self) {
        let picker = &mut self.overlays.picker;
        let Some(popup) = &mut picker.property else {
            return;
        };
        picker.goto = None;
        picker.default = None;
        popup.rows.clear();
        let query = picker.input.text().trim();
        if popup.kind().is_choice() {
            let folded = query.to_lowercase();
            popup.rows.extend(
                popup
                    .options
                    .iter()
                    .enumerate()
                    .filter(|(_, option)| option.to_lowercase().contains(&folded))
                    .map(|(i, _)| ChoiceRow::Option(i)),
            );
            if !query.is_empty() && !popup.options.iter().any(|option| option == query) {
                // Exact existing labels are never duplicated. Prefer a match over creating.
                popup.rows.push(ChoiceRow::Create);
            }
            if query.is_empty() && !popup.chosen.is_empty() {
                popup.rows.push(ChoiceRow::Clear);
            }
        } else if popup.kind() == Kind::Date
            && let Some((y, m, _)) = properties::date_parts(query)
        {
            popup.month = (y, m);
        }
        picker.hits = (0..popup.rows.len()).collect();
    }

    pub(in crate::app) fn place_property_picker(&mut self) {
        let Some(popup) = &self.overlays.picker.property else {
            return;
        };
        let area = self.layout.editor.inset(8.0, 6.0);
        let kind = popup.kind();
        let anchor = popup.anchor;
        let w = (if kind == Kind::Date {
            282.0
        } else {
            anchor.w.clamp(260.0, 430.0)
        })
        .min(area.w);
        let wanted = MENU_PICKER_BAR
            + MENU_PICKER_PAD
            + if kind == Kind::Date {
                CALENDAR_HEIGHT
            } else if kind.is_choice() {
                CHOICE_CAPTION + popup.rows.len().clamp(1, 7) as f32 * CHOICE_ROW
            } else {
                0.0
            };
        let below = area.bottom() - anchor.bottom() - 4.0;
        let above = anchor.y - area.y - 4.0;
        let up = below < wanted && above > below;
        let space = if up { above } else { below };
        // Keep the whole calendar together even in a short window, overlapping the anchor if needed.
        let h = if kind == Kind::Date {
            wanted.min(area.h)
        } else {
            wanted.min(space.max(MENU_PICKER_BAR)).min(area.h)
        };
        let x = anchor.x.clamp(area.x, (area.right() - w).max(area.x));
        let y = (if up {
            anchor.y - 4.0 - h
        } else {
            anchor.bottom() + 4.0
        })
        .clamp(area.y, (area.bottom() - h).max(area.y));
        self.layout.picker = Rect::new(x, y, w, h);
    }

    pub(in crate::app) fn accept_property_picker(&mut self) {
        let picker = &self.overlays.picker;
        let Some(popup) = &picker.property else {
            return;
        };
        let query = picker.input.text();
        if !popup.kind().is_choice() {
            if properties::input_value(popup.kind(), query).is_some() {
                self.commit_property_picker(query.to_owned(), false);
            } else {
                self.flash(t!("properties-invalid-value"));
            }
            return;
        }
        let Some(row) = popup.rows.get(picker.selected) else {
            return;
        };
        let multi = popup.kind() == Kind::MultiSelect;
        let label = match row {
            ChoiceRow::Option(i) => &popup.options[*i],
            ChoiceRow::Create => query.trim(),
            ChoiceRow::Clear => {
                return self.commit_property_picker(
                    if multi { "[]".into() } else { String::new() },
                    false,
                );
            }
        };
        let value = if multi {
            let mut labels = popup.chosen.clone();
            if labels.iter().any(|s| s == label) {
                labels.retain(|s| s != label);
            } else {
                labels.push(label.to_owned());
            }
            // An empty multi-select is still a list, not a scalar.
            if labels.is_empty() {
                "[]".into()
            } else {
                option_text(&labels)
            }
        } else {
            label.to_owned()
        };
        self.commit_property_picker(value, multi);
    }

    fn commit_property_picker(&mut self, value: String, keep_open: bool) {
        let Some(popup) = &self.overlays.picker.property else {
            return;
        };
        let (edit, name, options) = (
            popup.edit.clone(),
            popup.name.clone(),
            option_text(&popup.options),
        );
        if !self.apply_property(&edit, &name, &value, &options) {
            self.flash(t!("properties-cannot-edit"));
            return;
        }
        if !keep_open {
            self.close_picker();
            return;
        }
        let revision = self.editor.active().map_or(edit.revision, |d| d.revision);
        let picker = &mut self.overlays.picker;
        let Some(popup) = &mut picker.property else {
            return;
        };
        popup.edit.revision = revision;
        popup.chosen = properties::input_value(edit.kind, &value)
            .map(|(_, labels)| labels)
            .unwrap_or_default();
        let created = matches!(popup.rows.get(picker.selected), Some(ChoiceRow::Create));
        for label in &popup.chosen {
            if !popup.options.contains(label) {
                popup.options.push(label.clone());
            }
        }
        // Retain the filter and selection while toggling matches; clear only a newly created label.
        if created {
            picker.input.set_text("");
            self.picker_refresh();
        } else {
            self.refresh_property_picker();
            let picker = &mut self.overlays.picker;
            picker.selected = picker.selected.min(picker.hits.len().saturating_sub(1));
        }
        self.relayout();
        self.picker_reveal();
    }

    pub(in crate::app) fn press_property_calendar(&mut self, part: CalendarPart) {
        let Some(popup) = &mut self.overlays.picker.property else {
            return;
        };
        let (year, month) = popup.month;
        let (year, month, day) = match part {
            CalendarPart::Previous | CalendarPart::Next => {
                popup.month = step_month(
                    popup.month,
                    if part == CalendarPart::Previous {
                        -1
                    } else {
                        1
                    },
                );
                self.request_redraw();
                return;
            }
            CalendarPart::Today => today(),
            CalendarPart::Day(day) => (year, month, day),
            CalendarPart::Clear => return self.commit_property_picker(String::new(), false),
        };
        self.commit_property_picker(format!("{year:04}-{month:02}-{day:02}"), false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_weekday_and_year_rollover() {
        assert_eq!(first_weekday(2026, 11), 0);
        assert_eq!(first_weekday(2024, 2), 4);
        assert_eq!(first_weekday(2000, 1), 6);
        assert_eq!(step_month((2026, 12), 1), (2027, 1));
        assert_eq!(step_month((2027, 1), -1), (2026, 12));
        assert_eq!(step_month((0, 1), -1), (0, 1));
    }
}
