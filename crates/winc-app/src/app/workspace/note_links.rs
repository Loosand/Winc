//! Saved note links for the current local library. Preview navigation and backlinks share the resolved targets.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use winc_editor::EntryKind;
use winc_preview::note_links::{LinkSyntax, extract};

use super::*;
use crate::app::preview::markdown::local_target;

#[cfg(test)]
mod tests;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Wiki<'a> {
    Heading(&'a str),
    Note(&'a str, Option<&'a str>),
    Missing,
}

pub(crate) fn wiki_target(raw: &str) -> Wiki<'_> {
    let (name, anchor) = raw.split_once('#').map_or((raw, None), |(name, anchor)| {
        (name, Some(anchor.trim()).filter(|s| !s.is_empty()))
    });
    match (name.trim(), anchor) {
        ("", Some(anchor)) => Wiki::Heading(anchor),
        ("", None) => Wiki::Missing,
        (name, anchor) => Wiki::Note(name, anchor),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WikiResult<'a> {
    Found(&'a Path),
    Ambiguous(&'a [PathBuf]),
    Missing,
}

/// One parsed note link citation.
#[derive(Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct Citation {
    pub(crate) range: Range<usize>,
    pub(crate) before: String,
    pub(crate) label: String,
    pub(crate) after: String,
    pub(crate) syntax: LinkSyntax,
    pub(crate) target: String,
}

pub(crate) type Backlinks = BTreeMap<PathBuf, Vec<Citation>>;

#[derive(Default)]
pub(crate) struct Index {
    notes: BTreeMap<PathBuf, Arc<Vec<Citation>>>,
    names: BTreeMap<String, Vec<PathBuf>>,
    #[cfg_attr(not(test), allow(dead_code))]
    incoming: BTreeMap<PathBuf, Backlinks>,
}

impl Index {
    pub(crate) fn has_note(&self, path: &Path) -> bool {
        self.notes.contains_key(path)
    }

    #[cfg(test)]
    pub(crate) fn backlinks(&self, path: &Path) -> Option<&Backlinks> {
        self.incoming.get(path)
    }

    pub(crate) fn resolve_wiki(&self, root: &Path, from: &Path, name: &str) -> WikiResult<'_> {
        let path = Path::new(name);
        if path.is_absolute() {
            return WikiResult::Missing;
        }
        if name.contains('/') {
            let base = if name.starts_with("./") || name.starts_with("../") {
                from.parent().unwrap_or(root)
            } else {
                root
            };
            let Some(path) = normalize(&base.join(path)).filter(|path| path.starts_with(root))
            else {
                return WikiResult::Missing;
            };
            if let Some((path, _)) = self.notes.get_key_value(&path) {
                return WikiResult::Found(path);
            }
            return match self.notes.get_key_value(&path.with_file_name(format!(
                "{}.md",
                path.file_name().unwrap_or_default().to_string_lossy()
            ))) {
                Some((path, _)) => WikiResult::Found(path),
                None => WikiResult::Missing,
            };
        }
        match self.names.get(name).map(Vec::as_slice) {
            Some([path]) => WikiResult::Found(path),
            Some(paths) => nearest_note(from.parent().unwrap_or(root), paths),
            None => WikiResult::Missing,
        }
    }
}

fn nearest_note<'a>(source_dir: &Path, paths: &'a [PathBuf]) -> WikiResult<'a> {
    let source_depth = source_dir.components().count();
    let mut best = None;
    let mut distance = usize::MAX;
    for path in paths {
        let Some(dir) = path.parent() else {
            continue;
        };
        // Count steps up to the shared directory and down to the candidate. Only an
        // unambiguous minimum may contribute a backlink; alphabetical order is not evidence.
        let common = source_dir
            .components()
            .zip(dir.components())
            .take_while(|(a, b)| a == b)
            .count();
        let steps = (source_depth - common) + (dir.components().count() - common);
        match steps.cmp(&distance) {
            std::cmp::Ordering::Less => {
                distance = steps;
                best = Some(path.as_path());
            }
            std::cmp::Ordering::Equal => best = None,
            std::cmp::Ordering::Greater => {}
        }
    }
    best.map_or(WikiResult::Ambiguous(paths), WikiResult::Found)
}

fn normalize(path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir if !normalized.pop() => return None,
            Component::ParentDir => {}
            part => normalized.push(part.as_os_str()),
        }
    }
    Some(normalized)
}

fn citations(text: &str) -> impl Iterator<Item = Citation> + '_ {
    extract(text).map(|link| {
        let start = &text[..link.source.start];
        let end = &text[link.source.end..];
        let from = start
            .char_indices()
            .rev()
            .take(72)
            .take_while(|&(_, c)| !matches!(c, '\n' | '\r'))
            .last()
            .map_or(start.len(), |(i, _)| i);
        let after: String = end
            .chars()
            .take(72)
            .take_while(|&c| !matches!(c, '\n' | '\r'))
            .collect();
        let label = if link.label.is_empty() {
            &link.target
        } else {
            &link.label
        };
        Citation {
            range: link.source,
            before: start[from..].trim_start().to_owned(),
            label: label.chars().take(100).collect(),
            after: after.trim_end().to_owned(),
            syntax: link.syntax,
            target: link.target,
        }
    })
}

fn scan(root: &Path, notes: &mut BTreeMap<PathBuf, Arc<Vec<Citation>>>, start: &Path) {
    if !start.starts_with(root) {
        return;
    }
    let mut dirs = vec![start.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in library::entries(&dir).unwrap_or_default() {
            if entry.kind == EntryKind::Dir {
                dirs.push(entry.path);
            } else {
                read_note(notes, entry.path);
            }
        }
    }
}

#[allow(
    clippy::disallowed_methods,
    reason = "the local note index must not download iCloud placeholders"
)]
fn read_note(notes: &mut BTreeMap<PathBuf, Arc<Vec<Citation>>>, path: PathBuf) {
    #[cfg(target_os = "macos")]
    if path
        .symlink_metadata()
        .is_ok_and(|metadata| crate::system::is_dataless(&metadata))
    {
        // Keep the note discoverable by its name so opening a link can download it.
        notes.insert(path, Arc::default());
        return;
    }
    if let Ok(bytes) = winc_backend::fs::read(&path) {
        let (text, encoding) = winc_editor::encoding::decode_unknown(&bytes);
        if encoding != winc_editor::Encoding::Binary {
            notes.insert(path, Arc::new(citations(&text).collect()));
        }
    }
}

#[allow(
    clippy::disallowed_methods,
    reason = "inspect local paths without following symlinks"
)]
fn update(root: &Path, previous: Option<&Index>, changes: Changes) -> Index {
    let mut notes = BTreeMap::new();
    match (previous, changes) {
        (Some(previous), Changes::Paths(paths)) => {
            notes.clone_from(&previous.notes);
            for path in paths {
                if !path.starts_with(root) {
                    continue;
                }
                if notes.remove(&path).is_none() {
                    notes.retain(|existing, _| !existing.starts_with(&path));
                }
                // Walk from the root boundary without following hidden or symlink directories.
                if !path
                    .ancestors()
                    .take_while(|parent| *parent != root)
                    .all(|parent| {
                        parent
                            .file_name()
                            .is_some_and(|name| !name.to_string_lossy().starts_with('.'))
                            && std::fs::symlink_metadata(parent)
                                .is_ok_and(|info| !info.is_symlink())
                    })
                {
                    continue;
                }
                if path.is_dir() {
                    scan(root, &mut notes, &path);
                } else if notebook::note_in(root, &path) && path.is_file() {
                    read_note(&mut notes, path);
                }
            }
        }
        _ => scan(root, &mut notes, root),
    }
    let mut index = Index {
        notes,
        ..Index::default()
    };
    for path in index.notes.keys() {
        for name in [path.file_stem(), path.file_name()].into_iter().flatten() {
            let entries = index
                .names
                .entry(name.to_string_lossy().into_owned())
                .or_default();
            if entries.last() != Some(path) {
                entries.push(path.clone());
            }
        }
    }
    for (source, links) in &index.notes {
        for link in links.iter() {
            let target = match link.syntax {
                LinkSyntax::Markdown => local_target(source, &link.target)
                    .as_deref()
                    .and_then(normalize)
                    .filter(|target| index.notes.contains_key(target)),
                LinkSyntax::Wiki => match wiki_target(&link.target) {
                    Wiki::Note(name, _) => match index.resolve_wiki(root, source, name) {
                        WikiResult::Found(path) => Some(path.to_path_buf()),
                        _ => None,
                    },
                    _ => None,
                },
            };
            if let Some(target) = target.filter(|target| target != source) {
                index
                    .incoming
                    .entry(target)
                    .or_default()
                    .entry(source.clone())
                    .or_default()
                    .push(link.clone());
            }
        }
    }
    index
}

enum Changes {
    All,
    Paths(BTreeSet<PathBuf>),
}

#[derive(Default)]
pub(crate) struct State {
    root: Option<PathBuf>,
    pub(crate) index: Option<Arc<Index>>,
    queued: Option<Changes>,
    response: Option<mpsc::Receiver<Index>>,
}

impl App {
    /// 笔记库链接索引已就绪时，用内存表快速确认库内 `.md` 路径存在，避免首帧绘制时主线程同步 `stat`。
    /// 索引中未命中、索引尚未就绪或路径不在索引覆盖范围内时返回 `None`，由调用方回退到文件系统检查，
    /// 保证新建文件尚未完成增量扫描时不会误报断链。
    pub(crate) fn indexed_note_exists(&self, target: &Path) -> Option<bool> {
        let root = self.library_root()?;
        if self.note_links.root.as_deref() != Some(root)
            || self.note_links.response.is_some()
            || self.note_links.queued.is_some()
        {
            return None;
        }
        let index = self.note_links.index.as_ref()?;
        let normalized = normalize(target)?;
        let rel = normalized.strip_prefix(root).ok()?;
        if rel
            .components()
            .any(|part| part.as_os_str().to_string_lossy().starts_with('.'))
        {
            return None;
        }
        let is_md = normalized
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
        (is_md && index.has_note(&normalized)).then_some(true)
    }

    pub(crate) fn ensure_note_links(&mut self) {
        let Some(root) = self.library_root() else {
            return;
        };
        if self.note_links.root.as_deref() != Some(root) {
            self.note_links = State {
                root: Some(root.to_path_buf()),
                queued: Some(Changes::All),
                ..State::default()
            };
            self.start_note_links();
        }
    }

    pub(super) fn note_links_changed(&mut self, rescan: bool, paths: &[PathBuf]) {
        let Some(root) = self.note_links.root.as_deref() else {
            return;
        };
        if self.library_root() != Some(root) {
            return;
        }
        if rescan {
            self.note_links.queued = Some(Changes::All);
        } else if !matches!(self.note_links.queued, Some(Changes::All)) {
            let paths: BTreeSet<_> = paths
                .iter()
                .filter(|p| {
                    p.strip_prefix(root).is_ok_and(|path| {
                        path.components()
                            .all(|part| !part.as_os_str().to_string_lossy().starts_with('.'))
                    })
                })
                .cloned()
                .collect();
            if paths.is_empty() {
                return;
            }
            match &mut self.note_links.queued {
                Some(Changes::Paths(pending)) => pending.extend(paths),
                _ => self.note_links.queued = Some(Changes::Paths(paths)),
            }
        }
        self.start_note_links();
    }

    fn start_note_links(&mut self) {
        if self.note_links.response.is_some() {
            return;
        }
        let Some(root) = self.note_links.root.clone() else {
            return;
        };
        let Some(changes) = self.note_links.queued.take() else {
            return;
        };
        let previous = self.note_links.index.clone();
        let (send, receive) = mpsc::channel();
        self.note_links.response = Some(receive);
        self.spawn_then(UserEvent::NoteLinksScanned, move || {
            let _ = send.send(update(&root, previous.as_deref(), changes));
        });
    }

    pub(in crate::app) fn poll_note_links(&mut self) {
        if let Some(index) = self
            .note_links
            .response
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            self.note_links.response = None;
            self.note_links.index = Some(Arc::new(index));
            self.preview.markdown.links.clear();
            self.request_redraw();
            self.start_note_links();
        }
    }
}
