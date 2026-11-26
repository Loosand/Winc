//! Value editors share the picker's keyboard, input method and outside-click handling.

use super::*;
use crate::layout::{MENU_PICKER_BAR, MENU_PICKER_PAD, PickerParts};
use winc_render::Rect;

pub(crate) const CHOICE_ROW: f32 = 29.0;
pub(crate) const CHOICE_CAPTION: f32 = 25.0;
pub(crate) const CALENDAR_HEADER: f32 = 32.0;
pub(crate) const CALENDAR_WEEKDAYS: f32 = 21.0;
pub(crate) const CALENDAR_WEEK: f32 = 25.0;
pub(crate) const CALENDAR_FOOTER: f32 = 32.0;
pub(crate) const CALENDAR_PAD: f32 = 5.0;

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

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PropertyInput {
    Value,
    Name,
    ListItem,
    Source,
}

pub(crate) struct PropertyPopup {
    edit: PropertyEdit,
    pub(crate) anchor: Rect,
    pub(crate) input: PropertyInput,
    target: String,
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

    pub(crate) fn choices(&self) -> bool {
        self.input == PropertyInput::Value && self.kind().is_choice()
    }

    pub(crate) fn calendar(&self) -> bool {
        self.input == PropertyInput::Value && self.kind() == Kind::Date
    }

    pub(crate) fn calendar_weeks(&self) -> u32 {
        let (year, month) = self.month;
        (first_weekday(year, month) + properties::days_in_month(year, month)).div_ceil(7)
    }

    pub(crate) fn calendar_height(&self) -> f32 {
        CALENDAR_PAD * 2.0
            + CALENDAR_HEADER
            + CALENDAR_WEEKDAYS
            + self.calendar_weeks() as f32 * CALENDAR_WEEK
            + CALENDAR_FOOTER
    }

    pub(crate) fn row(&self) -> Option<&str> {
        let target = self.target.strip_prefix(properties::PROPERTY)?;
        match self.input {
            PropertyInput::Value => Some(target),
            PropertyInput::Name => target.strip_prefix("name:"),
            PropertyInput::ListItem => target.strip_prefix("list:"),
            PropertyInput::Source => None,
        }
    }

    pub(crate) fn parts(&self, rect: Rect) -> PickerParts {
        let caption = if self.choices() { CHOICE_CAPTION } else { 0.0 };
        let mut parts = PickerParts::menu(rect, caption, 0.0);
        parts.row_height = CHOICE_ROW;
        if self.calendar() {
            parts.list = rect;
        }
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
    pub(in crate::app) fn open_property_source(&mut self, anchor: Rect) {
        let Some((doc, original)) = self.editor.active().zip(self.property_source_text()) else {
            return;
        };
        self.start_property_picker(PropertyPopup {
            edit: PropertyEdit {
                doc: doc.id,
                revision: doc.revision,
                field: None,
                kind: Kind::Text,
            },
            anchor,
            input: PropertyInput::Source,
            target: properties::RAW.into(),
            name: String::new(),
            original,
            options: Vec::new(),
            chosen: Vec::new(),
            rows: Vec::new(),
            month: (0, 1),
        });
    }

    pub(in crate::app) fn open_property_value(
        &mut self,
        index: usize,
        anchor: Rect,
        input: PropertyInput,
    ) {
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
        let input = if kind == Kind::List && input == PropertyInput::Value {
            PropertyInput::ListItem
        } else {
            input
        };
        let edit = PropertyEdit {
            doc: doc.id,
            revision: doc.revision,
            field: Some(field.name.clone()),
            kind,
        };
        if input == PropertyInput::Value && kind == Kind::Checkbox {
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
        let (target, original) = match input {
            PropertyInput::Value => (format!("{}{index}", properties::PROPERTY), field.input),
            PropertyInput::Name => (
                format!("{}name:{index}", properties::PROPERTY),
                field.name.clone(),
            ),
            PropertyInput::ListItem => (
                format!("{}list:{index}", properties::PROPERTY),
                String::new(),
            ),
            PropertyInput::Source => return,
        };
        self.start_property_picker(PropertyPopup {
            edit,
            anchor,
            input,
            target,
            name: field.name,
            original,
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
        if popup.choices() {
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
        } else if popup.calendar()
            && let Some((y, m, _)) = properties::date_parts(query)
        {
            popup.month = (y, m);
        }
        picker.hits = (0..popup.rows.len()).collect();
    }

    pub(crate) fn follow_property_picker(&mut self) {
        let Some(popup) = &mut self.overlays.picker.property else {
            return;
        };
        popup.anchor = self
            .hits
            .markdown_links
            .iter()
            .find(|(_, url)| url == &popup.target)
            .map_or_else(Rect::default, |(rect, _)| *rect);
        if popup.input == PropertyInput::ListItem
            && popup.anchor.w > 0.0
            && let Some(index) = popup
                .target
                .strip_prefix(properties::PROPERTY)
                .and_then(|target| target.strip_prefix("list:"))
            && let Some((cell, _)) = self
                .hits
                .markdown_links
                .iter()
                .find(|(_, url)| url.strip_prefix(properties::PROPERTY) == Some(index))
        {
            popup.anchor.w = (cell.right() - popup.anchor.x).max(0.0);
        }
        if popup.calendar() {
            popup.anchor.w = popup.anchor.w.min(self.fonts.ui * 8.4);
        }
        self.place_property_picker();
    }

    pub(crate) fn property_input_at(&self, x: f32, y: f32) -> bool {
        self.overlays
            .picker
            .property
            .as_ref()
            .is_some_and(|popup| popup.calendar() && popup.anchor.contains(x, y))
    }

    pub(in crate::app) fn place_property_picker(&mut self) {
        let Some(popup) = &self.overlays.picker.property else {
            return;
        };
        let anchor = popup.anchor;
        if anchor.w <= 0.0 || anchor.h <= 0.0 {
            self.layout.picker = Rect::default();
            return;
        }
        if !popup.calendar() && !popup.choices() {
            self.layout.picker = anchor;
            return;
        }
        let area = self.layout.editor.inset(8.0, 6.0);
        let w = if popup.calendar() {
            224.0
        } else {
            anchor.w.clamp(240.0, 430.0)
        };
        let wanted = if popup.calendar() {
            popup.calendar_height()
        } else {
            MENU_PICKER_BAR
                + MENU_PICKER_PAD
                + CHOICE_CAPTION
                + popup.rows.len().clamp(1, 7) as f32 * CHOICE_ROW
        };
        // Keep the whole calendar together even in a short window, overlapping the anchor if needed.
        self.layout.picker = crate::ui::place_below_or_above(
            anchor,
            area,
            (w, wanted),
            crate::ui::PopoverOptions {
                margin: 0.0,
                gap: 4.0,
                offset_x: 0.0,
                prefer_larger_side: true,
                min_height: (!popup.calendar()).then_some(MENU_PICKER_BAR),
            },
        );
    }

    pub(in crate::app) fn accept_property_picker(&mut self) {
        let picker = &self.overlays.picker;
        let Some(popup) = &picker.property else {
            return;
        };
        let query = picker.input.text();
        if !popup.choices() {
            self.finish_property_picker();
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

    /// Commit inline edits on Enter or before giving focus to another control.
    pub(in crate::app) fn finish_property_picker(&mut self) -> bool {
        let picker = &self.overlays.picker;
        let Some(popup) = &picker.property else {
            return true;
        };
        let query = picker.input.text();
        if popup.choices() || query == popup.original {
            self.close_picker();
            return true;
        }
        let (input, edit, name, query) = (
            popup.input,
            popup.edit.clone(),
            popup.name.clone(),
            query.to_owned(),
        );
        let applied = match input {
            PropertyInput::Value => {
                if properties::input_value(edit.kind, &query).is_none() {
                    self.flash(t!("properties-invalid-value"));
                    return false;
                }
                self.apply_property(&edit, &name, &query, "")
            }
            PropertyInput::Name => self.apply_property_key(&edit, Some(&query)),
            PropertyInput::ListItem => {
                let mut labels = self
                    .property_field_by_name(&name)
                    .and_then(|field| properties::list_items(&field.input))
                    .unwrap_or_default();
                let label = query.trim();
                if label.is_empty() || labels.iter().any(|item| item == label) {
                    true
                } else {
                    labels.push(label.into());
                    let options = option_text(&self.property_options(&name));
                    self.apply_property(&edit, &name, &properties::list_input(&labels), &options)
                }
            }
            PropertyInput::Source => {
                let patch = self
                    .property_prefix()
                    .and_then(|prefix| properties::source_patch(&prefix, &query));
                if let Some((span, replacement)) = patch {
                    self.property_edit_current(&edit)
                        && self.apply_property_patch(span, replacement)
                } else {
                    self.flash(t!("properties-invalid-yaml"));
                    return false;
                }
            }
        };
        if applied {
            self.close_picker();
        } else {
            self.flash(t!("properties-cannot-edit"));
        }
        applied
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
mod tests;
