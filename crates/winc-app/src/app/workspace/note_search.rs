//! Notebook-only search. Use the existing picker for keyboard, IME and scrolling,
//! but never expose project files or command/symbol prefixes in this dialog.

use std::collections::VecDeque;
use std::io::Read as _;
use std::path::Path;
use std::sync::Arc;

use super::*;
use crate::layout::PickerParts;

const PER_NOTE: u64 = 128 * 1024;
const BODY_BUDGET: u64 = 24 * 1024 * 1024;
const NOTE_LIMIT: usize = 10_000;
const DIRECTORY_LIMIT: usize = 4_000;
const RESULT_LIMIT: usize = 200;
const PREVIEW_MIN: f32 = 580.0;

pub(crate) struct SearchNote {
    pub(crate) path: PathBuf,
    pub(crate) title: String,
    pub(crate) folder: String,
    pub(crate) preview: String,
    title_key: String,
    body: String,
}

#[derive(Default)]
pub(crate) struct NoteIndex {
    pub(crate) notes: Vec<SearchNote>,
    pub(crate) limited: bool,
}

#[derive(Clone)]
pub(crate) enum Purpose {
    Open,
    FollowLink(Option<String>),
}

#[derive(Default)]
pub(crate) struct CachedNoteIndex {
    root: Option<PathBuf>,
    index: Option<Arc<NoteIndex>>,
    dirty: bool,
}

pub(crate) struct NoteSearch {
    pub(crate) index: Arc<NoteIndex>,
    response: Option<mpsc::Receiver<NoteIndex>>,
    pub(crate) titles_only: bool,
    pub(crate) purpose: Purpose,
    pub(crate) preview: bool,
    pub(crate) keyboard_mouse: (f32, f32),
}

impl NoteSearch {
    pub(crate) fn loading(&self) -> bool {
        self.response.is_some()
    }

    pub(crate) fn title_picker(&self) -> bool {
        !matches!(self.purpose, Purpose::Open)
    }
}

fn index_notes(root: &Path) -> NoteIndex {
    let mut out = NoteIndex::default();
    let mut pending = VecDeque::from([root.to_path_buf()]);
    let (mut directories, mut budget) = (0, BODY_BUDGET);
    while let Some(dir) = pending.pop_front() {
        directories += 1;
        if directories > DIRECTORY_LIMIT {
            out.limited = true;
            break;
        }
        for entry in super::library::entries(&dir).unwrap_or_default() {
            if entry.kind == winc_editor::EntryKind::Dir {
                pending.push_back(entry.path);
                continue;
            }
            if out.notes.len() == NOTE_LIMIT {
                out.limited = true;
                return out;
            }
            let title = entry
                .path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(&entry.name)
                .to_owned();
            let folder = dir
                .strip_prefix(root)
                .unwrap_or(Path::new(""))
                .to_string_lossy()
                .into_owned();
            let limit = budget.min(PER_NOTE);
            let bytes = read_start(&entry.path, limit + 1);
            out.limited |= bytes.len() as u64 > limit;
            let valid = &bytes[..bytes.len().min(limit as usize)];
            budget -= valid.len() as u64;
            let text = String::from_utf8_lossy(valid);
            let preview: String = text.chars().take(3000).collect();
            out.notes.push(SearchNote {
                title_key: title.to_lowercase(),
                title,
                folder,
                path: entry.path,
                body: text.to_lowercase(),
                preview,
            });
        }
    }
    out
}

#[allow(
    clippy::disallowed_methods,
    reason = "bounded reads of local notebook files"
)]
fn read_start(path: &Path, limit: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
        && let Ok(file) = std::fs::File::open(path)
    {
        let _ = file.take(limit).read_to_end(&mut bytes);
    }
    bytes
}

pub(crate) fn search_parts(card: Rect, preview: bool) -> PickerParts {
    let mut parts = PickerParts::menu(card, 37.0, 33.0);
    parts.row_height = 34.0;
    parts.bar = Rect::new(card.x, card.y, card.w, 48.0_f32.min(card.h));
    parts.actions.y = parts.bar.bottom();
    let top = parts.actions.bottom() + 25.0;
    let width = if preview && card.w >= PREVIEW_MIN {
        card.w * 0.60
    } else {
        card.w
    };
    parts.list = Rect::new(
        card.x + 10.0,
        top,
        (width - 20.0).max(0.0),
        (parts.footer.y - top - 8.0).max(0.0),
    );
    parts
}

pub(crate) fn preview_rect(card: Rect, parts: PickerParts) -> Rect {
    let x = parts.list.right() + 10.0;
    Rect::new(
        x,
        parts.list.y - 22.0,
        (card.right() - x - 12.0).max(0.0),
        parts.list.h + 22.0,
    )
}

impl App {
    pub(in crate::app) fn notebook_search_changed(&mut self, rescan: bool, paths: &[PathBuf]) {
        let Some(root) = self.library_root().map(Path::to_path_buf) else {
            return;
        };
        let cache = &mut self.overlays.picker.notebook_cache;
        if cache.root.as_deref() != Some(root.as_path()) || cache.index.is_none() {
            return;
        }
        if rescan
            || paths.iter().any(|path| {
                path.strip_prefix(&root).is_ok_and(|rel| {
                    rel.components()
                        .all(|part| !part.as_os_str().to_string_lossy().starts_with('.'))
                })
            })
        {
            cache.dirty = true;
        }
    }

    pub(in crate::app) fn open_notebook_search(&mut self) {
        let Some(root) = self.library_root().map(Path::to_path_buf) else {
            return;
        };
        let (cached, needs_scan) = {
            let cache = &self.overlays.picker.notebook_cache;
            if cache.root.as_deref() == Some(root.as_path())
                && let Some(index) = cache.index.clone()
            {
                (index, cache.dirty)
            } else {
                (Arc::new(NoteIndex::default()), true)
            }
        };
        let (send, receive) = if needs_scan {
            let (send, receive) = mpsc::channel();
            (Some(send), Some(receive))
        } else {
            (None, None)
        };
        self.open_picker_mode(Some(NoteSearch {
            index: cached,
            response: receive,
            titles_only: false,
            purpose: Purpose::Open,
            preview: true,
            keyboard_mouse: self.pointer.mouse,
        }));
        if let Some(send) = send {
            self.spawn_then(UserEvent::NotebookSearchScanned, move || {
                let _ = send.send(index_notes(&root));
            });
        }
    }

    pub(in crate::app) fn choose_wiki_note(&mut self, paths: &[PathBuf], anchor: Option<String>) {
        self.open_title_picker(paths, Purpose::FollowLink(anchor));
    }

    fn open_title_picker(&mut self, paths: &[PathBuf], purpose: Purpose) {
        let Some(root) = self.library_root() else {
            return;
        };
        let notes = paths
            .iter()
            .map(|path| {
                let title = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let folder = path
                    .parent()
                    .and_then(|p| p.strip_prefix(root).ok())
                    .unwrap_or(Path::new(""))
                    .to_string_lossy()
                    .into_owned();
                SearchNote {
                    title_key: title.to_lowercase(),
                    path: path.clone(),
                    title,
                    folder,
                    body: String::new(),
                    preview: String::new(),
                }
            })
            .collect();
        self.open_picker_mode(Some(NoteSearch {
            index: Arc::new(NoteIndex {
                notes,
                limited: false,
            }),
            response: None,
            titles_only: true,
            purpose,
            preview: false,
            keyboard_mouse: self.pointer.mouse,
        }));
    }

    pub(in crate::app) fn poll_notebook_search(&mut self) {
        let received = self
            .overlays
            .picker
            .notebook
            .as_mut()
            .and_then(|search| search.response.as_ref()?.try_recv().ok());
        let Some(index) = received else {
            return;
        };
        let index = Arc::new(index);
        if let Some(root) = self.library_root().map(Path::to_path_buf) {
            self.overlays.picker.notebook_cache = CachedNoteIndex {
                root: Some(root),
                index: Some(Arc::clone(&index)),
                dirty: false,
            };
        }
        if let Some(search) = self.overlays.picker.notebook.as_mut() {
            search.index = index;
            search.response = None;
        }
        self.picker_refresh();
        if self.picker_backdrop_eligible() {
            self.request_overlay_redraw();
        } else {
            self.request_redraw();
        }
    }

    pub(in crate::app) fn place_notebook_search(&mut self, (w, h): (f32, f32)) {
        if self.overlays.picker.open && self.overlays.picker.notebook.is_some() {
            let width = (w - 48.0).clamp(0.0, 920.0);
            let height = (h - 104.0).clamp(0.0, 594.0);
            self.layout.picker = Rect::new(
                (w - width) / 2.0,
                ((h - height) * 0.39).max(32.0),
                width,
                height,
            );
        }
    }

    pub(in crate::app) fn refresh_notebook_search(&mut self) {
        let picker = &mut self.overlays.picker;
        let Some(search) = &mut picker.notebook else {
            return;
        };
        search.keyboard_mouse = self.pointer.mouse;
        let query = picker.input.text().trim().to_lowercase();
        let terms: Vec<_> = query.split_whitespace().collect();
        let mut ranked = Vec::new();
        for (i, note) in search.index.notes.iter().enumerate() {
            let rank = if terms.is_empty() || terms.iter().all(|t| note.title_key.contains(t)) {
                if note.title_key.starts_with(&query) {
                    0
                } else {
                    1
                }
            } else if !search.titles_only
                && terms
                    .iter()
                    .all(|t| note.title_key.contains(t) || note.body.contains(t))
            {
                2
            } else {
                continue;
            };
            ranked.push((rank, note.title_key.as_str(), &note.path, i));
        }
        ranked.sort_unstable();
        picker.hits = ranked
            .into_iter()
            .take(RESULT_LIMIT)
            .map(|(_, _, _, i)| i)
            .collect();
        picker.goto = None;
    }

    pub(crate) fn notebook_search_selection(&self) -> usize {
        let picker = &self.overlays.picker;
        match self.pointer.hover {
            Hover::PickerRow(i)
                if picker
                    .notebook
                    .as_ref()
                    .is_some_and(|search| search.keyboard_mouse != self.pointer.mouse) =>
            {
                i
            }
            _ => picker.selected,
        }
    }

    pub(in crate::app) fn accept_notebook_search(&mut self) {
        let picker = &self.overlays.picker;
        let chosen = picker.notebook.as_ref().and_then(|search| {
            picker
                .hits
                .get(picker.selected)
                .and_then(|&i| search.index.notes.get(i))
        });
        let Some(path) = chosen.map(|note| note.path.clone()) else {
            return;
        };
        let purpose = picker
            .notebook
            .as_ref()
            .map(|search| search.purpose.clone());
        self.close_picker();
        if !self.note_in_library(&path) {
            return;
        }
        match purpose {
            Some(Purpose::FollowLink(anchor)) => {
                self.open_markdown_file(&path, anchor.as_deref().unwrap_or_default())
            }
            Some(Purpose::Open) => self.open_path(&path),
            None => {}
        }
    }
}
