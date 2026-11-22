//! Editing notebook properties through the normal document undo/save path.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;

use winc_config::Json;
use winc_i18n::t;
use winc_preview::markdown::properties::{self, Field, Header, Kind};

use super::*;
use crate::app::git::form::{FormDialog, FormKind, FormPlan};
use crate::app::git::question::{QuestionInput, QuestionValue, QuestionView};
use crate::app::input::context_menu::{MenuCommand, MenuEntry, MenuTarget};
use crate::field::TextField;
use crate::layout::DialogPart;

pub(crate) mod popup;

const OBSIDIAN_TYPES: &str = ".obsidian/types.json";

// A missing file is cached too: editing a note must not do filesystem I/O on every layout.
#[derive(Default)]
pub(crate) struct ImportedTypes(RefCell<Option<CachedTypes>>);

struct CachedTypes {
    root: PathBuf,
    json: Option<Arc<Json>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PropertyEdit {
    pub(crate) doc: DocumentId,
    pub(crate) revision: u64,
    pub(crate) field: Option<String>,
    pub(crate) kind: Kind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropertyMenu {
    doc: DocumentId,
    revision: u64,
    pub(crate) index: Option<usize>,
    pub(crate) options: Vec<String>,
    pub(crate) chosen: Vec<String>,
}

impl PropertyEdit {
    pub(crate) fn view(&self) -> QuestionView {
        let mut values = Vec::new();
        let mut inputs = Vec::new();
        if let Some(name) = &self.field {
            values.push(QuestionValue {
                label: t!("properties-name"),
                text: name.clone(),
                mono: false,
            });
        } else {
            inputs.push(QuestionInput {
                label: t!("properties-name"),
                placeholder: t!("properties-name-placeholder"),
            });
        }
        values.push(QuestionValue {
            label: t!("properties-type"),
            text: self.kind.label().to_owned(),
            mono: false,
        });
        inputs.push(QuestionInput {
            label: t!("properties-value"),
            placeholder: self.kind.sample(),
        });
        if self.kind.is_choice() {
            inputs.push(QuestionInput {
                label: t!("properties-options"),
                placeholder: Kind::List.sample(),
            });
        }
        QuestionView {
            title: if self.field.is_some() {
                t!("properties-edit")
            } else {
                t!("properties-add")
            }
            .into(),
            message: None,
            message_lines: 0,
            values,
            inputs,
            confirm: t!("properties-apply"),
        }
    }

    fn input<'a>(&'a self, dialog: &'a FormDialog) -> (&'a str, &'a str, &'a str) {
        match &self.field {
            Some(name) => (name, dialog.name.text(), dialog.second.text()),
            None => (
                dialog.name.text().trim(),
                dialog.second.text(),
                dialog.third.text(),
            ),
        }
    }
}

fn option_input(text: &str) -> Option<Vec<String>> {
    properties::input_value(Kind::MultiSelect, text).map(|(_, options)| options)
}

fn option_text(options: &[String]) -> String {
    let escape = |text: &str| format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""));
    if options.is_empty() {
        String::new()
    } else {
        format!(
            "[{}]",
            options
                .iter()
                .map(|s| escape(s))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

impl App {
    fn property_prefix(&self) -> Option<String> {
        let rope = self.editor.active()?.text().rope()?;
        let end = rope.char_to_byte(rope.byte_to_char(rope.len_bytes().min(properties::LIMIT + 1)));
        Some(rope.byte_slice(..end).to_string())
    }

    fn property_field(&self, index: usize) -> Option<Field> {
        let Header::Valid { fields, .. } = properties::header(&self.property_prefix()?) else {
            return None;
        };
        fields.into_iter().nth(index)
    }

    pub(in crate::app) fn property_body_start(&self) -> Option<usize> {
        if !self.note_properties_available() {
            return None;
        }
        let doc = self.editor.active()?;
        let md = &self.preview.markdown;
        let parsed = md
            .parsed
            .as_ref()
            .filter(|_| md.doc == Some(doc.id) && md.checked == Some(doc.revision))?;
        let (end, _) = parsed.properties.as_ref()?;
        let rope = doc.text().rope()?;
        // Blank rows after the closing delimiter are already part of the editable body.
        Some(rope.byte_to_char((*end).min(rope.len_bytes())))
    }

    pub(crate) fn note_properties_available(&self) -> bool {
        self.markdown_live()
            && self
                .editor
                .active()
                .and_then(|doc| doc.path.as_deref())
                .is_none_or(|p| !winc_backend::is_remote(p))
    }

    fn property_scope(&self) -> String {
        self.library_root()
            .or_else(|| {
                self.editor
                    .active()
                    .and_then(|d| d.path.as_deref())
                    .and_then(Path::parent)
            })
            .map_or_else(String::new, |p| p.to_string_lossy().into_owned())
    }

    fn property_spec(&self, name: &str) -> Option<&Json> {
        self.state
            .note_property_schema
            .get(&self.property_scope())?
            .as_object()?
            .get(name)
    }

    fn imported_property_types(&self) -> Option<Arc<Json>> {
        let root = self.library_root()?;
        let mut cache = self.preview.markdown.imported_property_types.0.borrow_mut();
        let entry = cache.get_or_insert_with(|| CachedTypes {
            root: PathBuf::new(),
            json: None,
        });
        if entry.root != root {
            let read = || {
                let file = root.join(OBSIDIAN_TYPES);
                if !winc_config::files::canonicalize(&file)
                    .ok()?
                    .starts_with(root)
                {
                    return None;
                }
                let meta = winc_backend::fs::symlink_metadata(&file).ok()?;
                if !meta.is_file() || meta.len > properties::LIMIT as u64 {
                    return None;
                }
                Json::parse(&winc_backend::fs::read_to_string(file).ok()?).map(Arc::new)
            };
            *entry = CachedTypes {
                root: root.to_owned(),
                json: read(),
            };
        }
        entry.json.clone()
    }

    pub(in crate::app) fn property_types_changed(&mut self, rescan: bool, paths: &[PathBuf]) {
        let dirty = self.library_root().is_some_and(|root| {
            rescan || {
                let file = root.join(OBSIDIAN_TYPES);
                paths.iter().any(|path| file.starts_with(path))
            }
        });
        if dirty {
            self.preview
                .markdown
                .imported_property_types
                .0
                .get_mut()
                .take();
            self.preview.markdown.property_layout_dirty = true;
        }
    }

    fn configured_property_kind(&self, name: &str) -> Option<Kind> {
        Kind::from_id(
            self.property_spec(name)?
                .as_object()?
                .get("kind")?
                .as_str()?,
        )
    }

    fn imported_property_kind(types: Option<&Json>, name: &str) -> Option<Kind> {
        Kind::from_id(
            types?
                .as_object()?
                .get("types")?
                .as_object()?
                .get(name)?
                .as_str()?,
        )
    }

    fn property_kind(&self, field: &Field) -> Kind {
        self.configured_property_kind(&field.name)
            .or_else(|| {
                Self::imported_property_kind(self.imported_property_types().as_deref(), &field.name)
            })
            .unwrap_or(field.kind)
    }

    pub(super) fn property_panel(&self, fields: &[Field]) -> winc_preview::markdown::Block {
        let imported = self.imported_property_types();
        let mut fields = fields.to_owned();
        for field in &mut fields {
            if field.editable {
                field.kind = self
                    .configured_property_kind(&field.name)
                    .or_else(|| Self::imported_property_kind(imported.as_deref(), &field.name))
                    .unwrap_or(field.kind);
            }
        }
        properties::panel(&fields)
    }

    fn property_options(&self, name: &str) -> Vec<String> {
        self.property_spec(name)
            .and_then(|s| s.as_object()?.get("options"))
            .and_then(Json::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Json::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(in crate::app) fn open_property_menu(&mut self, index: Option<usize>, at: (f32, f32)) {
        if !self.note_properties_available() {
            return;
        }
        let Some(doc) = self.editor.active() else {
            return;
        };
        let mut options = Vec::new();
        let mut chosen = Vec::new();
        if let Some(field) = index
            .and_then(|i| self.property_field(i))
            .filter(|f| f.editable)
        {
            let kind = self.property_kind(&field);
            if kind.is_choice() {
                options = self.property_options(&field.name);
                if let Some((_, values)) = properties::input_value(kind, &field.input) {
                    chosen = values;
                    for value in &chosen {
                        if !options.contains(value) {
                            options.push(value.clone());
                        }
                    }
                }
                options.truncate(30);
            }
        }
        self.open_context_menu(
            MenuTarget::NoteProperty(PropertyMenu {
                doc: doc.id,
                revision: doc.revision,
                index,
                options,
                chosen,
            }),
            at,
        );
    }

    pub(in crate::app) fn property_menu(&self, index: Option<usize>) -> Vec<MenuEntry> {
        let kinds = Kind::ALL
            .map(|kind| MenuEntry::Item(MenuCommand::PropertyType(kind), true))
            .to_vec();
        match index {
            None => kinds,
            Some(index) => {
                let Some(field) = self.property_field(index) else {
                    return Vec::new();
                };
                let mut menu = vec![MenuEntry::Item(MenuCommand::EditProperty, field.editable)];
                if field.editable {
                    menu.push(MenuEntry::Submenu(t!("properties-type").into(), kinds));
                    if self.property_kind(&field) == Kind::Checkbox {
                        menu.extend([
                            MenuEntry::Item(MenuCommand::PropertyCheck(true), true),
                            MenuEntry::Item(MenuCommand::PropertyCheck(false), true),
                        ]);
                    }
                }
                menu
            }
        }
    }

    pub(in crate::app) fn select_property_menu(&mut self, menu: PropertyMenu, cmd: MenuCommand) {
        if let MenuCommand::PropertyOption(i) = cmd {
            let Some((field, option)) = menu
                .index
                .and_then(|i| self.property_field(i))
                .zip(menu.options.get(i))
            else {
                return;
            };
            let kind = self.property_kind(&field);
            let value = match kind {
                Kind::Select => option.to_owned(),
                Kind::MultiSelect => {
                    let mut selected = menu.chosen;
                    if selected.contains(option) {
                        selected.retain(|value| value != option);
                    } else {
                        selected.push(option.clone());
                    }
                    option_text(&selected)
                }
                _ => return,
            };
            let edit = PropertyEdit {
                doc: menu.doc,
                revision: menu.revision,
                field: Some(field.name.clone()),
                kind,
            };
            let options = option_text(&self.property_options(&field.name));
            if !self.apply_property(&edit, &field.name, &value, &options) {
                self.flash(t!("properties-cannot-edit"));
            }
        } else {
            self.run_property_menu(menu.doc, menu.revision, menu.index, cmd);
        }
    }

    pub(in crate::app) fn run_property_menu(
        &mut self,
        doc: DocumentId,
        revision: u64,
        index: Option<usize>,
        cmd: MenuCommand,
    ) {
        if !self.note_properties_available()
            || !self
                .editor
                .active()
                .is_some_and(|d| d.id == doc && d.revision == revision)
        {
            return;
        }
        let field = match index {
            None => None,
            Some(i) => match self.property_field(i).filter(|field| field.editable) {
                Some(field) => Some(field),
                None => return,
            },
        };
        let kind = match cmd {
            MenuCommand::PropertyType(kind) => kind,
            MenuCommand::EditProperty => {
                field.as_ref().map_or(Kind::Text, |f| self.property_kind(f))
            }
            MenuCommand::PropertyCheck(on) => {
                if let Some(field) = field {
                    let edit = PropertyEdit {
                        doc,
                        revision,
                        field: Some(field.name.clone()),
                        kind: Kind::Checkbox,
                    };
                    let _ = self.apply_property(
                        &edit,
                        &field.name,
                        if on { "true" } else { "false" },
                        "",
                    );
                }
                return;
            }
            _ => return,
        };
        let edit = PropertyEdit {
            doc,
            revision,
            field: field.as_ref().map(|f| f.name.clone()),
            kind,
        };
        let mut first = TextField::default();
        let mut second = TextField::default();
        if let Some(field) = field {
            first.set_text(&field.input);
            first.select_all();
            if kind.is_choice() {
                second.set_text(&option_text(&self.property_options(&field.name)));
            }
        }
        self.show_form_dialog(FormKind::Property(edit), first, DialogPart::Branch);
        if let Some(dialog) = &mut self.overlays.form_dialog {
            dialog.second = second;
        }
    }

    pub(in crate::app) fn property_plan(
        &self,
        edit: &PropertyEdit,
        dialog: &FormDialog,
    ) -> FormPlan {
        let (name, value, options) = edit.input(dialog);
        let enabled = !name.is_empty()
            && properties::input_value(edit.kind, value).is_some()
            && (!edit.kind.is_choice() || option_input(options).is_some());
        FormPlan {
            problem: None,
            confirm: t!("properties-apply"),
            enabled,
            base: String::new(),
            base_enabled: false,
            dir: None,
        }
    }

    pub(in crate::app) fn submit_property(&mut self, edit: PropertyEdit) {
        let Some(dialog) = self.overlays.form_dialog.as_ref() else {
            return;
        };
        let (name, value, options) = edit.input(dialog);
        let (name, value, options) = (name.to_owned(), value.to_owned(), options.to_owned());
        if self.apply_property(&edit, &name, &value, &options) {
            self.close_form_dialog();
        } else {
            self.flash(t!("properties-cannot-edit"));
            if let Some(dialog) = &mut self.overlays.form_dialog {
                dialog.error = Some(t!("properties-cannot-edit").into());
            }
            self.request_redraw();
        }
    }

    fn apply_property(
        &mut self,
        edit: &PropertyEdit,
        name: &str,
        value: &str,
        options: &str,
    ) -> bool {
        let Some(doc) = self.editor.active() else {
            return false;
        };
        if doc.id != edit.doc || doc.revision != edit.revision || !self.note_properties_available()
        {
            return false;
        }
        let Some(prefix) = self.property_prefix() else {
            return false;
        };
        let Some((span, replacement)) =
            properties::patch(&prefix, name, edit.kind, value, edit.field.is_none())
        else {
            return false;
        };
        let scope = self.property_scope();
        let mut selected = if edit.kind.is_choice() {
            option_input(options).unwrap_or_default()
        } else {
            Vec::new()
        };
        if let Some((_, values)) = properties::input_value(edit.kind, value) {
            for value in values {
                if !selected.contains(&value) {
                    selected.push(value);
                }
            }
        }
        if !span.is_empty() || !replacement.is_empty() {
            let replaced = replacement.bytes().filter(|&b| b == b'\n').count();
            let inserted =
                replacement.chars().count() + replaced * (doc.line_ending.as_str().len() - 1);
            let command = winc_editor::Command::Insert(replacement);
            if doc.edit_blocked(&command).is_some() {
                return false;
            }
            let doc = self.editor.active_mut().unwrap();
            let rope = doc.text().rope().unwrap();
            let range = rope.byte_to_char(span.start)..rope.byte_to_char(span.end);
            let prior = doc.selection;
            let original = range.len();
            let pos = range.start;
            doc.collapse_selections();
            doc.selection = winc_core::Selection::new(range.start, range.end);
            self.run_note_property_patch(command);
            let mapped = |p: usize| {
                if p >= range.end {
                    p + inserted - original
                } else if p >= pos {
                    pos + inserted
                } else {
                    p
                }
            };
            if let Some(doc) = self.editor.active_mut() {
                doc.selection = winc_core::Selection::new(mapped(prior.anchor), mapped(prior.head));
            }
        }
        if !scope.is_empty() {
            self.update_state(|state| {
                let section = state
                    .note_property_schema
                    .entry(scope)
                    .or_insert_with(|| Json::Object(BTreeMap::new()));
                if let Json::Object(fields) = section {
                    fields.insert(
                        name.to_owned(),
                        Json::Object(BTreeMap::from([
                            ("kind".into(), Json::String(edit.kind.id().into())),
                            (
                                "options".into(),
                                Json::Array(selected.into_iter().map(Json::String).collect()),
                            ),
                        ])),
                    );
                }
            });
        }
        // A type-only edit can change the row without changing the Markdown revision.
        self.preview.markdown.property_layout_dirty = true;
        self.request_redraw();
        true
    }
}
