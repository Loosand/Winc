//! 本地笔记库：浏览真实目录，在后台读取卡片的摘要，不导入或改写文件。

use std::fs::File;
use std::io::{self, Read};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use winc_editor::{EntryKind, Reading, list_dir};
use winc_i18n::t;
use winc_preview::markdown::{self, Block, IMAGE_MARK, is_markdown};

use super::*;
use crate::app::scroll::ScrollTarget;

pub(in crate::app) const MIN_REFRESH: Duration = Duration::from_millis(500);

mod motion;
use motion::FolderMotion;
pub(crate) use motion::FolderPose;

pub(in crate::app) const HOME_ROW: f32 = 34.0;
const GRID_TOP: f32 = 104.0;
const PREVIEW_BYTES: u64 = 4096;
const PREVIEW_LINES: usize = 14;
const LINE_CHARS: usize = 200;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum LibraryTarget {
    Root,
    Parent,
    Refresh,
    Card(usize),
    NewInFolder(usize),
    FolderMenu(usize),
}

/// 响应式网格的几何；绘图和滚动共用同一组尺寸。
pub(crate) struct Grid {
    pub(crate) body: Rect,
    pub(crate) cols: usize,
    width: f32,
    paper: f32,
    height: f32,
    gap: f32,
}

impl Grid {
    pub(crate) fn new(area: Rect, cards: &[Card]) -> Self {
        Self::for_cards(area, cards.first().is_some_and(Card::is_folder))
    }

    pub(crate) fn for_cards(area: Rect, has_folders: bool) -> Self {
        // Quiet MD home.rs: 24px page margins, a 5:6 paper, and extra room around folders.
        let inset = (area.w * 0.25).min(24.0);
        let body = Rect::new(
            area.x + inset,
            area.y + GRID_TOP,
            (area.w - 2.0 * inset).max(0.0),
            (area.h - GRID_TOP).max(0.0),
        );
        let gap = if has_folders { 24.0 } else { 16.0 };
        let capacity = |width: f32| ((body.w + gap) / (width + gap)).floor() as usize;
        let cols = if capacity(200.0) >= 4 {
            capacity(240.0).max(4)
        } else {
            capacity(160.0).clamp(1, 3)
        };
        let width = ((body.w - gap * (cols - 1) as f32) / cols as f32).floor();
        let paper = (width * 1.2).max(1.0);
        let height = if has_folders { paper / 0.94 } else { paper };
        Self {
            body,
            cols,
            width,
            paper,
            height,
            gap,
        }
    }

    pub(crate) fn card(&self, index: usize, scroll: f32, folder: bool) -> Rect {
        let height = if folder { self.height } else { self.paper };
        Rect::new(
            self.body.x + (index % self.cols) as f32 * (self.width + self.gap),
            self.body.y + (index / self.cols) as f32 * (self.height + self.gap) + self.height
                - height
                - scroll,
            self.width,
            height,
        )
    }

    pub(crate) fn visible(&self, count: usize, scroll: f32) -> std::ops::Range<usize> {
        let stride = self.height + self.gap;
        let first = (scroll / stride).floor() as usize * self.cols;
        let end = ((scroll + self.body.h) / stride).ceil() as usize * self.cols;
        first.min(count)..end.min(count)
    }

    /// In a single-column shelf, show a full card and part of the next one on short screens.
    pub(crate) fn content_height(&self, count: usize) -> f32 {
        count.div_ceil(self.cols) as f32 * (self.height + self.gap)
    }

    pub(crate) fn max_scroll(&self, count: usize) -> f32 {
        (self.content_height(count) - self.body.h).max(0.0)
    }

    /// Covers the same vertical span as the tiles but hugs the editor edge.
    pub(crate) fn scroll_track(&self, editor: Rect) -> Rect {
        Rect::new(editor.x, self.body.y, editor.w, self.body.h)
    }
}

pub(crate) struct Card {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) content: CardContent,
}

impl Card {
    fn is_folder(&self) -> bool {
        matches!(self.content, CardContent::Folder(_))
    }
}

pub(crate) enum CardContent {
    Note(Vec<PreviewLine>),
    Folder(Option<FolderPreview>),
}

pub(crate) struct FolderPreview {
    pub(crate) count: usize,
    /// 叠放的纸张最多三张，只读最前面一份文档的开头。
    pub(crate) papers: usize,
    pub(crate) seeds: [u64; 3],
    pub(crate) front: Option<(String, Vec<PreviewLine>)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PreviewLine {
    Text(String),
    Heading(String),
    List(String),
    Quote(String),
    Code(String),
    Rule,
}

type Scan = (u64, io::Result<Vec<Card>>);

pub(crate) struct Library {
    pub(crate) folder: Option<PathBuf>,
    pub(crate) cards: Vec<Card>,
    pub(crate) error: Option<String>,
    pub(crate) scroll: f32,
    pub(crate) manual_refresh: Option<Instant>,
    folders: FolderMotion,
    pressed: Option<PathBuf>,
    generation: u64,
    queued: bool,
    running: bool,
    scans: (mpsc::Sender<Scan>, mpsc::Receiver<Scan>),
}

impl Default for Library {
    fn default() -> Self {
        Self {
            folder: None,
            cards: Vec::new(),
            error: None,
            scroll: 0.0,
            manual_refresh: None,
            folders: FolderMotion::default(),
            pressed: None,
            generation: 0,
            queued: false,
            running: false,
            scans: mpsc::channel(),
        }
    }
}

impl Library {
    pub(crate) fn loading(&self) -> bool {
        self.running || self.queued
    }

    pub(crate) fn folder_pose(&self, card: usize) -> FolderPose {
        self.folders.pose(card)
    }

    pub(in crate::app) fn animating(&self) -> bool {
        self.folders.moving()
    }

    #[cfg(test)]
    pub(in crate::app) fn freeze_folder_motion(&mut self, card: usize, open: bool, millis: u64) {
        self.folders.freeze(card, open, millis);
    }
}

impl App {
    /// 主页切换或鼠标离开时收回纸张；其他卡片不播放动画。
    pub(crate) fn track_library_motion(&mut self) {
        let hot = match self.pointer.hover {
            Hover::Library(
                LibraryTarget::Card(index)
                | LibraryTarget::NewInFolder(index)
                | LibraryTarget::FolderMenu(index),
            ) if self.library_shown()
                && self.library.cards.get(index).is_some_and(Card::is_folder) =>
            {
                Some(index)
            }
            _ => None,
        };
        let animate = self.library_shown() && self.window.handle.is_some() && self.animations_on();
        self.library
            .folders
            .update(hot, animate, std::time::Instant::now());
    }

    pub(crate) fn library_root(&self) -> Option<&Path> {
        self.editor
            .root()
            .filter(|root| !winc_backend::is_remote(root))
    }

    pub(crate) fn library_shown(&self) -> bool {
        self.library_root().is_some()
            && self
                .editor
                .active()
                .is_none_or(|d| d.reading == Some(Reading::Library))
    }

    pub(crate) fn library_home_button(&self) -> Rect {
        if self.library_root().is_none() || self.panels.sidebar.view != SidebarView::Files {
            return Rect::default();
        }
        let r = self.layout.sidebar_header;
        Rect::new(
            r.x + 8.0,
            if self.notebook_sidebar() {
                r.y + 5.0
            } else {
                r.bottom() + 2.0
            },
            (r.w - 16.0).max(0.0),
            if self.notebook_sidebar() {
                30.0
            } else {
                HOME_ROW - 6.0
            },
        )
        .intersect(self.layout.sidebar)
    }

    /// 回到笔记库根目录。写作窗口先保存并关闭当前笔记。
    pub(in crate::app) fn browse_library(&mut self) {
        let Some(root) = self.library_root().map(Path::to_path_buf) else {
            return;
        };
        if self.writing_workspace() && !self.library_shown() && !self.prepare_writing_switch(None) {
            return;
        }
        self.set_library_folder(root);
        if !self.library_shown() {
            let tab = self
                .editor
                .documents
                .iter()
                .position(|d| d.reading == Some(Reading::Library));
            if let Some(index) = tab {
                self.activate_tab(index);
            } else {
                self.auto_save_active();
                self.editor
                    .show_reading(t!("workspace-library-home"), Reading::Library);
            }
        }
        self.focus = Focus::Editor;
        self.relayout();
        self.request_redraw();
    }

    pub(in crate::app) fn press_library(&mut self, target: LibraryTarget) {
        if target == LibraryTarget::Root {
            self.browse_library();
            return;
        }
        if !self.library_shown() {
            return;
        }
        match target {
            LibraryTarget::Root => unreachable!(),
            LibraryTarget::Parent => {
                let parent = self
                    .library
                    .folder
                    .as_deref()
                    .and_then(Path::parent)
                    .filter(|p| self.library_root().is_some_and(|root| p.starts_with(root)));
                if let Some(parent) = parent {
                    self.set_library_folder(parent.to_path_buf());
                }
            }
            LibraryTarget::Refresh => {
                if self.library.manual_refresh.is_none() {
                    self.library.manual_refresh = Some(Instant::now() + MIN_REFRESH);
                    self.refresh_library();
                }
            }
            LibraryTarget::NewInFolder(index) => {
                if let Some(card) = self.library.cards.get(index).filter(|c| c.is_folder()) {
                    self.create_folder_note(card.path.clone());
                }
            }
            LibraryTarget::FolderMenu(index) => {
                if let Some(card) = self.library.cards.get(index).filter(|c| c.is_folder()) {
                    let path = card.path.clone();
                    self.reveal_path(&path);
                    let at = self
                        .hits
                        .library
                        .iter()
                        .find(|(_, t)| *t == target)
                        .map_or(self.mouse(), |(r, _)| (r.x, r.bottom()));
                    use crate::app::input::context_menu::MenuTarget;
                    self.open_context_menu(MenuTarget::Tree(Some(path)), at);
                }
            }
            LibraryTarget::Card(index) => {
                let Some(card) = self.library.cards.get(index) else {
                    return;
                };
                let path = card.path.clone();
                if card.is_folder() {
                    self.library.pressed = Some(path);
                    self.library.folders.press(index);
                } else {
                    self.open_path(&path);
                }
            }
        }
        self.request_redraw();
    }

    pub(in crate::app) fn library_refresh_tick(&mut self, now: Instant) -> Option<Instant> {
        let earliest = self.library.manual_refresh?;
        if now < earliest {
            return Some(earliest);
        }
        if !self.library.loading() {
            self.library.manual_refresh = None;
            if let Some(error) = &self.library.error {
                self.notify(
                    NoticeKind::Warning,
                    t!("workspace-library-failed", error = error.as_str()),
                );
            } else {
                self.notify(NoticeKind::Done, t!("workspace-library-refreshed"));
            }
            self.request_redraw();
        }
        None
    }

    pub(in crate::app) fn take_pressed_folder(&mut self) -> Option<PathBuf> {
        self.library.pressed.take()
    }

    pub(in crate::app) fn release_library_folder(&mut self, index: usize, pressed: &Path) {
        if self
            .library
            .cards
            .get(index)
            .is_some_and(|c| c.is_folder() && c.path == pressed)
        {
            self.set_library_folder(pressed.to_path_buf());
            self.request_redraw();
        }
    }

    /// 同 Quiet MD 的卡片加号：直接在该目录创建并打开一篇笔记。
    fn create_folder_note(&mut self, dir: PathBuf) {
        let Some(root) = self.library_root().map(Path::to_path_buf) else {
            return;
        };
        let where_ = dir.clone();
        self.file_work(
            &where_,
            move || {
                if winc_config::files::canonicalize(&dir)? != dir || !dir.starts_with(&root) {
                    return Err(io::Error::from(io::ErrorKind::PermissionDenied));
                }
                for n in 1..=1000 {
                    let name = if n == 1 {
                        "untitled.md".to_owned()
                    } else {
                        format!("untitled-{n}.md")
                    };
                    match winc_editor::create_at(&root, &dir, &name, EntryKind::File, &[]) {
                        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                        result => return result,
                    }
                }
                Err(io::Error::from(io::ErrorKind::AlreadyExists))
            },
            move |app, created| match created {
                Ok(path) => {
                    if let Some(ws) = &mut app.editor.workspace {
                        ws.created(&path);
                    }
                    app.picker_touched(vec![path.clone()]);
                    app.library_paths_changed(false, std::slice::from_ref(&path));
                    app.open_path_then(&path, |app| {
                        if app.markdown_live() {
                            app.start_live_editing();
                        }
                    });
                }
                Err(error) => app.flash(t!(
                    "workspace-library-create-failed",
                    error = error.to_string()
                )),
            },
        );
    }

    /// 在左侧文件树点击目录时，如果正在主页浏览，也在右侧显示同一个目录。
    pub(in crate::app) fn browse_library_tree_folder(&mut self, path: &Path) {
        if self.library_shown()
            && self
                .library_root()
                .is_some_and(|root| path.starts_with(root))
        {
            self.set_library_folder(path.to_path_buf());
        }
    }

    pub(in crate::app) fn sync_library(&mut self) {
        if self.library_shown()
            && let Some(root) = self.library_root()
            && self
                .library
                .folder
                .as_deref()
                .is_none_or(|path| !path.starts_with(root))
        {
            self.set_library_folder(root.to_path_buf());
        }
    }

    pub(in crate::app) fn reset_library(&mut self) {
        self.note_links = Default::default();
        self.library.folder = None;
        self.library.manual_refresh = None;
        self.library.cards.clear();
        self.library.error = None;
        self.library.scroll = 0.0;
        self.library.folders = FolderMotion::default();
        self.library.pressed = None;
        self.library.generation += 1;
        self.library.queued = false;
    }

    fn set_library_folder(&mut self, folder: PathBuf) {
        if self.library.folder.as_ref() != Some(&folder) {
            self.library.folder = Some(folder);
            self.library.cards.clear();
            self.library.error = None;
            self.library.scroll = 0.0;
            self.library.folders = FolderMotion::default();
            self.library.pressed = None;
            self.hits.library.clear();
        }
        self.refresh_library();
    }

    pub(in crate::app) fn library_paths_changed(&mut self, rescan: bool, paths: &[PathBuf]) {
        self.note_links_changed(rescan, paths);
        if self.library.folder.as_deref().is_some_and(|folder| {
            rescan
                || paths.iter().any(|p| {
                    folder.starts_with(p)
                        || p.parent() == Some(folder)
                        || (p.starts_with(folder) && is_markdown(p))
                })
        }) {
            self.refresh_library();
        }
    }

    pub(in crate::app) fn refresh_library(&mut self) {
        if self.library.folder.is_some() {
            self.library.generation += 1;
            self.library.queued = true;
            self.start_library_scan();
        }
    }

    fn start_library_scan(&mut self) {
        if self.library.running || !self.library.queued {
            return;
        }
        let Some(folder) = self.library.folder.clone() else {
            return;
        };
        self.library.queued = false;
        self.library.running = true;
        let generation = self.library.generation;
        let root = self.library_root().map(Path::to_path_buf);
        let sender = self.library.scans.0.clone();
        self.spawn_then(UserEvent::LibraryScanned, move || {
            let cards = match (root, winc_config::files::canonicalize(&folder)) {
                (Some(root), Ok(path)) if path == folder && path.starts_with(&root) => {
                    scan_cards(&folder)
                }
                (_, Err(error)) => Err(error),
                _ => Err(io::Error::from(io::ErrorKind::PermissionDenied)),
            };
            let _ = sender.send((generation, cards));
        });
    }

    pub(in crate::app) fn poll_library(&mut self) {
        while let Ok((generation, cards)) = self.library.scans.1.try_recv() {
            self.library.running = false;
            if generation == self.library.generation {
                self.hits.library.clear();
                match cards {
                    Ok(cards) => {
                        if !self
                            .library
                            .cards
                            .iter()
                            .map(|c| &c.path)
                            .eq(cards.iter().map(|c| &c.path))
                        {
                            self.library.folders = FolderMotion::default();
                        }
                        self.library.cards = cards;
                        self.library.error = None;
                    }
                    Err(error) => {
                        self.library.cards.clear();
                        self.library.folders = FolderMotion::default();
                        self.library.error = Some(error.to_string());
                    }
                }
                self.set_pane_scroll(ScrollTarget::Library, self.library.scroll);
                self.request_redraw();
            }
        }
        self.start_library_scan();
    }
}

fn scan_cards(folder: &Path) -> io::Result<Vec<Card>> {
    Ok(entries(folder)?
        .into_iter()
        .map(|entry| {
            let (name, content) = if entry.kind == EntryKind::Dir {
                (entry.name, CardContent::Folder(folder_preview(&entry.path)))
            } else {
                (note_name(&entry), CardContent::Note(read_page(&entry.path)))
            };
            Card {
                path: entry.path,
                name,
                content,
            }
        })
        .collect())
}

pub(super) fn entries(folder: &Path) -> io::Result<Vec<winc_editor::DirEntry>> {
    // 不跟随符号链接，原文件树行为不变。
    Ok(list_dir(folder)?
        .into_iter()
        .filter(|entry| {
            !entry.symlink
                && !entry.name.starts_with('.')
                && (entry.kind == EntryKind::Dir || is_markdown(&entry.path))
        })
        .collect())
}

fn note_name(entry: &winc_editor::DirEntry) -> String {
    entry
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&entry.name)
        .to_owned()
}

fn folder_preview(folder: &Path) -> Option<FolderPreview> {
    use std::collections::VecDeque;
    use std::hash::BuildHasher;

    let children = entries(folder).ok()?;
    let count = children.len();
    // Quiet MD picks the most recent real documents, including descendants. Bound disk work
    // in Minc because it does not keep Quiet's whole document store in memory.
    let mut pending = VecDeque::from([(children, 0)]);
    let mut notes = Vec::new();
    let mut dirs = 0;
    while let Some((children, depth)) = pending.pop_front() {
        for entry in children {
            if entry.kind == EntryKind::Dir && dirs < 32 && depth < 8 {
                dirs += 1;
                if let Ok(children) = entries(&entry.path) {
                    pending.push_back((children, depth + 1));
                }
            } else if entry.kind == EntryKind::File
                && let Ok(meta) = winc_backend::fs::symlink_metadata(&entry.path)
                && meta.is_file()
            {
                notes.push((meta.modified, entry));
                if notes.len() >= 512 {
                    break;
                }
            }
        }
        if notes.len() >= 512 {
            break;
        }
    }
    notes.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.path.cmp(&b.1.path)));
    let papers = notes.len().min(3);
    let seeds = std::array::from_fn(|i| {
        notes.get(i).map_or(1, |(_, entry)| {
            std::hash::BuildHasherDefault::<std::collections::hash_map::DefaultHasher>::default()
                .hash_one(&entry.path)
        })
    });
    let front = notes
        .first()
        .map(|(_, entry)| (note_name(entry), read_page(&entry.path)));
    Some(FolderPreview {
        count,
        papers,
        seeds,
        front,
    })
}

#[allow(
    clippy::disallowed_methods,
    reason = "笔记库只浏览本机目录，按字节上限读取缩略页"
)]
fn read_page(path: &Path) -> Vec<PreviewLine> {
    if !std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file()) {
        return Vec::new();
    }
    let mut bytes = Vec::new();
    if File::open(path)
        .and_then(|f| f.take(PREVIEW_BYTES).read_to_end(&mut bytes))
        .is_err()
    {
        return Vec::new();
    }
    preview_lines(&String::from_utf8_lossy(&bytes))
}

pub(crate) fn preview_lines(text: &str) -> Vec<PreviewLine> {
    let text = text.trim_start_matches('\u{feff}');
    let mut lines = text.split_inclusive('\n');
    let mut skip = 0;
    if let Some(first) = lines.next().filter(|line| line.trim() == "---") {
        let mut end = first.len();
        for line in lines {
            end += line.len();
            if matches!(line.trim(), "---" | "...") {
                skip = end;
                break;
            }
        }
    }
    let md = markdown::parse(&text[skip..]);
    let mut out = Vec::new();
    collect_lines(&md.blocks, &mut out, 0);
    out
}

fn collect_lines(blocks: &[Block], out: &mut Vec<PreviewLine>, depth: usize) {
    if depth >= 5 {
        return;
    }
    for block in blocks {
        if out.len() >= PREVIEW_LINES {
            return;
        }
        let push = |out: &mut Vec<_>, text: &str, kind: fn(String) -> PreviewLine| {
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            let mut chars = text.chars().filter(|&c| c != IMAGE_MARK && !c.is_control());
            let mut s: String = chars.by_ref().take(LINE_CHARS).collect();
            if chars.next().is_some() {
                s.push('…');
            }
            if !s.trim().is_empty() && out.len() < PREVIEW_LINES {
                out.push(kind(s));
            }
        };
        match block {
            Block::Paragraph(text) => push(out, &text.text, PreviewLine::Text),
            Block::Heading(_, text) => push(out, &text.text, PreviewLine::Heading),
            Block::Code { text, .. } => {
                for line in text.lines().take(3) {
                    push(out, line, PreviewLine::Code);
                }
            }
            Block::Quote(_, children) => {
                let from = out.len();
                collect_lines(children, out, depth + 1);
                for line in &mut out[from..] {
                    if let PreviewLine::Text(text) = line {
                        *line = PreviewLine::Quote(std::mem::take(text));
                    }
                }
            }
            Block::List(_, items) => {
                for item in items.iter().take(5) {
                    let from = out.len();
                    collect_lines(&item.blocks, out, depth + 1);
                    if let Some(line @ PreviewLine::Text(_)) = out.get_mut(from)
                        && let PreviewLine::Text(text) = line
                    {
                        *line = PreviewLine::List(std::mem::take(text));
                    }
                }
            }
            Block::Table(_, rows, _) => {
                for row in rows.iter().take(3) {
                    let text = row
                        .iter()
                        .map(|cell| cell.text.as_str())
                        .collect::<Vec<_>>()
                        .join("  ·  ");
                    push(out, &text, PreviewLine::Code);
                }
            }
            Block::Rule(_) => out.push(PreviewLine::Rule),
            Block::Math(text, _) => push(out, text, PreviewLine::Code),
            Block::Details { summary, .. } => push(out, &summary.text, PreviewLine::Heading),
            Block::Footnote(..) => {}
        }
    }
}
