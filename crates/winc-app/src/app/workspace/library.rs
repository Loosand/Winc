//! 本地笔记库：浏览真实目录，在后台读取卡片的摘要，不导入或改写文件。

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};

use winc_editor::{DirEntry, EntryKind, Reading, list_dir};
use winc_i18n::t;
use winc_preview::markdown::{self, Block, IMAGE_MARK, is_markdown};

use super::*;
use crate::app::scroll::ScrollTarget;

mod attachment;
mod motion;
pub(crate) use attachment::{AttachmentKind, AttachmentPreview, JsonExcerpt};
use motion::FolderMotion;
pub(crate) use motion::{
    FolderPose, FolderTransition, IncomingLayer, NavDirection, OutgoingLayer, OutgoingLibrary,
};

pub(in crate::app) const HOME_ROW: f32 = 34.0;
const GRID_TOP: f32 = 78.0;
const FIRST_BATCH: usize = 24;
const PREVIEW_BYTES: u64 = 4096;
const PREVIEW_LINES: usize = 14;
const LINE_CHARS: usize = 200;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum LibraryTarget {
    Root,
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

    fn attachment(&self) -> Option<(&Path, AttachmentKind)> {
        match &self.content {
            CardContent::Attachment(kind) => Some((&self.path, *kind)),
            CardContent::Folder(Some(folder)) => folder.front.as_deref()?.attachment(),
            _ => None,
        }
    }
}

pub(crate) enum CardContent {
    Note(Vec<PreviewLine>),
    /// 渐进加载第一阶段：已拿到笔记标题，正等待后台读取正文摘要。
    PendingNote,
    Attachment(AttachmentKind),
    File,
    Folder(Option<FolderPreview>),
}

pub(crate) struct FolderPreview {
    pub(crate) count: usize,
    /// 叠放的纸张最多三张，只预览最前面的一份文件。
    pub(crate) papers: usize,
    pub(crate) seeds: [u64; 3],
    pub(crate) front: Option<Box<Card>>,
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

#[derive(Default)]
struct Snapshot {
    cards: Vec<Card>,
    attachments: HashMap<PathBuf, AttachmentPreview>,
}

enum ScanUpdate {
    /// 第一阶段：拿到当前目录的条目列表后立刻上屏，用户可马上浏览和点击。
    Listing(Vec<Card>),
    /// 渐进批次：将第 `start` 项起已就绪的摘要、文件夹封面与缩略图原地填回卡片网格。
    Batch {
        start: usize,
        cards: Vec<Card>,
        attachments: HashMap<PathBuf, AttachmentPreview>,
    },
    /// 渐进扫描全部完成。
    Done,
    /// 非渐进刷新完成或发生错误：一次性替换整份快照，刷新途中保留旧网格。
    Complete(io::Result<Snapshot>),
}

type Scan = (u64, ScanUpdate);

pub(crate) struct Library {
    pub(crate) folder: Option<PathBuf>,
    pub(crate) cards: Vec<Card>,
    attachments: HashMap<PathBuf, AttachmentPreview>,
    pub(crate) error: Option<String>,
    pub(crate) scroll: f32,
    folders: FolderMotion,
    /// 进入、退出或切换文件夹时的双快照焦点缩放过渡；结束时立即释放快照内存。
    pub(crate) transition: Option<FolderTransition>,
    /// 鼠标按下文件夹卡片瞬间顺手保存的画面快照，松开进入时直接复用，免去首帧重绘 9-12 张文件夹卡片。
    pub(crate) press_shot: Option<(Rect, winc_render::Snapshot)>,
    pub(crate) pressed: Option<PathBuf>,
    /// Changes only on navigation. Same-directory refreshes are queued without canceling the scan.
    generation: Arc<AtomicU64>,
    queued: bool,
    running: bool,
    loading_since: Option<std::time::Instant>,
    scans: (mpsc::Sender<Scan>, mpsc::Receiver<Scan>),
}

impl Default for Library {
    fn default() -> Self {
        Self {
            folder: None,
            cards: Vec::new(),
            attachments: HashMap::new(),
            error: None,
            scroll: 0.0,
            folders: FolderMotion::default(),
            transition: None,
            press_shot: None,
            pressed: None,
            generation: Arc::default(),
            queued: false,
            running: false,
            loading_since: None,
            scans: mpsc::channel(),
        }
    }
}

impl Library {
    pub(crate) fn loading(&self) -> bool {
        self.running || self.queued
    }

    /// 当前是否处于需要显示骨架/加载指示的状态（空网格等待目录，或卡片仍在补齐摘要）。
    pub(crate) fn loading_visual(&self) -> bool {
        self.loading()
            && (self.cards.is_empty()
                || self
                    .cards
                    .iter()
                    .any(|card| matches!(card.content, CardContent::PendingNote)))
    }

    /// 骨架呼吸动画的已运行秒数。
    pub(crate) fn loading_phase(&self) -> f32 {
        self.loading_since
            .map_or(0.0, |since| since.elapsed().as_secs_f32())
    }

    pub(crate) fn folder_pose(&self, card: usize) -> FolderPose {
        self.folders.pose(card)
    }

    pub(crate) fn attachment_preview(&self, path: &Path) -> Option<&AttachmentPreview> {
        self.attachments.get(path)
    }

    pub(in crate::app) fn animating(&self) -> bool {
        self.folders.moving() || self.transition.is_some()
    }

    pub(in crate::app) fn settle_animations(&mut self) {
        if self.transition.as_ref().is_some_and(FolderTransition::done) {
            self.transition = None;
        }
    }

    /// 首屏目录内容是否已就绪（扫描完成，或首批可见卡片均已填好真实摘要而非占位卡片）。
    pub(crate) fn incoming_ready(&self) -> bool {
        if !self.loading() {
            return true;
        }
        !self.cards.is_empty()
            && !self
                .cards
                .iter()
                .take(12)
                .any(|c| matches!(c.content, CardContent::PendingNote))
    }

    #[cfg(test)]
    pub(in crate::app) fn freeze_folder_motion(&mut self, card: usize, open: bool, millis: u64) {
        self.folders.freeze(card, open, millis);
    }

    #[cfg(test)]
    pub(in crate::app) fn start_test_transition(
        &mut self,
        direction: NavDirection,
        elapsed: std::time::Duration,
        origin: Option<(f32, f32)>,
        from_folder: Option<PathBuf>,
        outgoing: OutgoingLayer,
    ) {
        let clock = crate::app::animation::Clock::stopped(direction.duration(), elapsed);
        self.transition = Some(FolderTransition::new_running(
            direction,
            clock,
            origin,
            from_folder,
            outgoing,
        ));
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
        if self.writing_workspace()
            || self.library_root().is_none()
            || self.panels.sidebar.view != SidebarView::Files
        {
            return Rect::default();
        }
        let r = self.layout.sidebar_header;
        Rect::new(
            r.x + 8.0,
            r.bottom() + 2.0,
            (r.w - 16.0).max(0.0),
            HOME_ROW - 6.0,
        )
        .intersect(self.layout.sidebar)
    }

    /// 回到笔记库根目录。写作窗口先保存并关闭当前笔记。
    pub(in crate::app) fn browse_library(&mut self) {
        let Some(root) = self.library_root().map(Path::to_path_buf) else {
            return;
        };
        self.browse_library_folder(root, Focus::Editor);
    }

    pub(in crate::app) fn browse_library_folder(&mut self, folder: PathBuf, focus: Focus) {
        self.remember_workspace_visit();
        if self.writing_workspace() && !self.library_shown() && !self.prepare_writing_switch(None) {
            return;
        }
        if self.notebook_sidebar() && focus == Focus::Editor {
            self.reveal_path(&folder);
        }
        let was_shown = self.library_shown();
        self.set_library_folder(folder, was_shown);
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
        self.focus = focus;
        self.relayout();
        self.remember_workspace_visit();
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
            self.browse_library_folder(pressed.to_path_buf(), Focus::Editor);
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

    /// 切换目录时保留展开状态，再次选中当前目录时只切换侧栏的展开状态。
    pub(in crate::app) fn activate_tree_folder(&mut self, index: usize) {
        let Some(ws) = &self.editor.workspace else {
            return;
        };
        let Some(entry) = ws.rows().get(index) else {
            return;
        };
        let path = ws
            .segment()
            .filter(|_| ws.selected == Some(index))
            .unwrap_or(&entry.path);
        let browsing = self.library_shown();
        let notebook = self.notebook_sidebar();
        let can_browse = (notebook || browsing)
            && self
                .library_root()
                .is_some_and(|root| path.starts_with(root));
        let current = browsing && self.library.folder.as_deref() == Some(path);
        let destination = (can_browse && !current).then(|| path.to_path_buf());
        self.set_dir_expanded(index, (notebook && destination.is_some()).then_some(true));
        if let Some(folder) = destination {
            self.browse_library_folder(folder, Focus::Tree);
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
            self.set_library_folder(root.to_path_buf(), false);
        }
    }

    pub(in crate::app) fn reset_library(&mut self) {
        self.note_links = Default::default();
        self.library.folder = None;
        self.library.cards.clear();
        self.library.attachments.clear();
        self.library.error = None;
        self.library.scroll = 0.0;
        self.library.folders = FolderMotion::default();
        self.library.transition = None;
        self.library.press_shot = None;
        self.library.pressed = None;
        self.library.generation.fetch_add(1, Ordering::Relaxed);
        self.library.queued = false;
        self.library.loading_since = None;
    }

    fn set_library_folder(&mut self, folder: PathBuf, animate_from_visible: bool) {
        if self.library.folder.as_ref() == Some(&folder) {
            return;
        }
        let direction = NavDirection::between(self.library.folder.as_deref(), &folder);
        // 借用 `self.clock` 校验动态效果开关并同步当前显示器的帧间隔（如 120Hz ProMotion）。
        let animate = self.clock(direction.duration()).is_some();
        let old_folder = self.library.folder.take();
        let old_cards = std::mem::take(&mut self.library.cards);
        let old_attachments = std::mem::take(&mut self.library.attachments);
        let old_error = self.library.error.take();
        let old_scroll = self.library.scroll;
        let old_folders = std::mem::take(&mut self.library.folders);
        let press_shot = self.library.press_shot.take();

        self.library.transition = animate.then(|| {
            let mut origin = None;
            let outgoing = if animate_from_visible && old_folder.is_some() {
                if direction == NavDirection::Enter && self.layout.editor.w > 0.0 {
                    let grid = Grid::new(self.layout.editor, &old_cards);
                    let clamped = old_scroll.min(grid.max_scroll(old_cards.len()));
                    origin = old_cards
                        .iter()
                        .enumerate()
                        .find(|(_, c)| {
                            c.is_folder() && (c.path == folder || folder.starts_with(&c.path))
                        })
                        .map(|(i, _)| {
                            let r = grid.card(i, clamped, true);
                            (r.x + r.w * 0.5, r.y + r.h * 0.5)
                        });
                }
                if let Some((rect, snap)) = press_shot
                    && rect == self.layout.editor
                {
                    OutgoingLayer::Captured(snap)
                } else {
                    let poses = (0..old_cards.len()).map(|i| old_folders.pose(i)).collect();
                    OutgoingLayer::Pending(Box::new(OutgoingLibrary {
                        folder: old_folder.clone(),
                        cards: old_cards,
                        attachments: old_attachments,
                        scroll: old_scroll,
                        poses,
                        error: old_error,
                    }))
                }
            } else {
                OutgoingLayer::None
            };
            FolderTransition::new_waiting(direction, origin, old_folder, outgoing)
        });

        self.library.generation.fetch_add(1, Ordering::Relaxed);
        self.library.folder = Some(folder);
        self.library.scroll = 0.0;
        self.library.pressed = None;
        self.library.loading_since = Some(std::time::Instant::now());
        self.hits.library.clear();
        self.refresh_library();
    }

    pub(in crate::app) fn library_paths_changed(&mut self, rescan: bool, paths: &[PathBuf]) {
        self.note_links_changed(rescan, paths);
        if self.library.folder.as_deref().is_some_and(|folder| {
            rescan
                || paths.iter().any(|p| {
                    folder.starts_with(p)
                        || p.parent() == Some(folder)
                        || p.parent().and_then(Path::parent) == Some(folder)
                        || (p.starts_with(folder)
                            && (is_markdown(p) || AttachmentKind::of(p).is_some()))
                })
        }) {
            self.refresh_library();
        }
    }

    pub(in crate::app) fn refresh_library(&mut self) {
        if self.library.folder.is_some() {
            self.library.queued = true;
            self.start_library_scan();
        }
    }

    /// Replace the grid and its previews together, keeping the previous grid during a refresh.
    fn show_library_snapshot(&mut self, snapshot: io::Result<Snapshot>) {
        let library = &mut self.library;
        let snapshot = match snapshot {
            Ok(snapshot) => {
                library.error = None;
                snapshot
            }
            Err(error) => {
                library.error = Some(error.to_string());
                Snapshot::default()
            }
        };
        if !library
            .cards
            .iter()
            .map(|c| &c.path)
            .eq(snapshot.cards.iter().map(|c| &c.path))
        {
            library.folders = FolderMotion::default();
            library.pressed = None;
            self.hits.library.clear();
        }
        library.cards = snapshot.cards;
        library.attachments = snapshot.attachments;
        if let Some(transition) = library.transition.as_mut() {
            transition.invalidate_incoming();
        }
        self.set_pane_scroll(ScrollTarget::Library, self.library.scroll);
        self.request_redraw();
    }

    fn apply_library_batch(
        &mut self,
        start: usize,
        cards: Vec<Card>,
        attachments: HashMap<PathBuf, AttachmentPreview>,
    ) {
        for (offset, card) in cards.into_iter().enumerate() {
            if let Some(slot) = self.library.cards.get_mut(start + offset)
                && slot.path == card.path
            {
                *slot = card;
            }
        }
        self.library.attachments.extend(attachments);
        if let Some(transition) = self.library.transition.as_mut() {
            transition.invalidate_incoming();
        }
        self.request_redraw();
    }

    fn start_library_scan(&mut self) {
        if self.library.running || !self.library.queued {
            return;
        }
        let Some(folder) = self.library.folder.clone() else {
            return;
        };
        let progressive = self.library.cards.is_empty();
        if progressive {
            self.library
                .loading_since
                .get_or_insert_with(std::time::Instant::now);
        }
        self.library.queued = false;
        self.library.running = true;
        let generation = Arc::clone(&self.library.generation);
        let version = generation.load(Ordering::Relaxed);
        let root = self.library_root().map(Path::to_path_buf);
        let show_hidden = self.panels.sidebar.show_hidden_files;
        let reverse = self.panels.sidebar.reverse_name_order;
        let sender = self.library.scans.0.clone();
        let proxy = self.link.proxy.clone();
        self.spawn_then(UserEvent::LibraryScanned, move || {
            let emit = |update: ScanUpdate| {
                let _ = sender.send((version, update));
                if let Some(proxy) = &proxy {
                    let _ = proxy.send_event(UserEvent::LibraryScanned);
                }
            };
            let result = match (root, winc_config::files::canonicalize(&folder)) {
                (Some(root), Ok(path)) if folder.starts_with(&root) && path.starts_with(&root) => {
                    if progressive {
                        scan_library_progressive(
                            &root,
                            &folder,
                            show_hidden,
                            reverse,
                            || generation.load(Ordering::Relaxed) != version,
                            &emit,
                        )
                        .map(|done| {
                            if done {
                                ScanUpdate::Done
                            } else {
                                ScanUpdate::Complete(Ok(Snapshot::default()))
                            }
                        })
                    } else {
                        scan_library(&root, &folder, show_hidden, reverse, || {
                            generation.load(Ordering::Relaxed) != version
                        })
                        .map(|snapshot| ScanUpdate::Complete(Ok(snapshot)))
                    }
                }
                (_, Err(error)) => Err(error),
                _ => Err(io::Error::from(io::ErrorKind::PermissionDenied)),
            };
            let final_update = match result {
                Ok(update) => update,
                Err(error) => ScanUpdate::Complete(Err(error)),
            };
            let _ = sender.send((version, final_update));
        });
    }

    pub(in crate::app) fn poll_library(&mut self) {
        let current = self.library.generation.load(Ordering::Relaxed);
        while let Ok((version, update)) = self.library.scans.1.try_recv() {
            match update {
                ScanUpdate::Listing(cards) => {
                    if version == current && self.library.cards.is_empty() {
                        self.library.cards = cards;
                        self.library.error = None;
                        if let Some(transition) = self.library.transition.as_mut() {
                            transition.invalidate_incoming();
                        }
                        self.set_pane_scroll(ScrollTarget::Library, self.library.scroll);
                        self.request_redraw();
                    }
                }
                ScanUpdate::Batch {
                    start,
                    cards,
                    attachments,
                } => {
                    if version == current {
                        self.apply_library_batch(start, cards, attachments);
                    }
                }
                ScanUpdate::Done => {
                    self.library.running = false;
                    if version == current {
                        self.library.loading_since = None;
                        if let Some(transition) = self.library.transition.as_mut() {
                            transition.invalidate_incoming();
                        }
                        self.request_redraw();
                    }
                }
                ScanUpdate::Complete(snapshot) => {
                    self.library.running = false;
                    if version == current {
                        self.library.loading_since = None;
                        self.show_library_snapshot(snapshot);
                    }
                }
            }
        }
        self.start_library_scan();
    }
}

fn sorted_entries(folder: &Path, show_hidden: bool, reverse: bool) -> io::Result<Vec<DirEntry>> {
    let mut entries = visible_entries(folder, show_hidden)?;
    if reverse {
        let split = entries.partition_point(|entry| entry.kind == EntryKind::Dir);
        entries[..split].reverse();
        entries[split..].reverse();
    }
    Ok(entries)
}

fn placeholder_card(entry: &DirEntry) -> Card {
    if entry.kind == EntryKind::Dir {
        Card {
            path: entry.path.clone(),
            name: entry.name.clone(),
            content: CardContent::Folder(None),
        }
    } else if !entry.symlink && is_markdown(&entry.path) {
        Card {
            path: entry.path.clone(),
            name: note_name(entry),
            content: CardContent::PendingNote,
        }
    } else if !entry.symlink
        && let Some(kind) = AttachmentKind::of(&entry.path)
    {
        Card {
            path: entry.path.clone(),
            name: entry.name.clone(),
            content: CardContent::Attachment(kind),
        }
    } else {
        Card {
            path: entry.path.clone(),
            name: entry.name.clone(),
            content: CardContent::File,
        }
    }
}

fn scan_entry(
    root: &Path,
    entry: DirEntry,
    show_hidden: bool,
    attachments: &mut HashMap<PathBuf, AttachmentPreview>,
) -> Card {
    let card = if entry.kind == EntryKind::Dir {
        let preview = (!entry.symlink
            && winc_config::files::canonicalize(&entry.path)
                .is_ok_and(|path| path.starts_with(root))
            && winc_backend::fs::symlink_metadata(&entry.path).is_ok_and(|meta| meta.is_dir()))
        .then(|| folder_preview(root, &entry.path, show_hidden))
        .flatten();
        Card {
            path: entry.path,
            name: entry.name,
            content: CardContent::Folder(preview),
        }
    } else {
        file_card(root, entry)
    };
    if let Some((path, kind)) = card.attachment()
        && let Some(preview) = attachment::load(root, path, kind)
    {
        attachments.insert(path.to_path_buf(), preview);
    }
    card
}

/// 渐进扫描：先立刻发回目录项占位卡片，再按批次流式补齐封面、摘要与附件缩略图。
///
/// 返回 `Ok(true)` 表示已发回占位并按批次完成；`Ok(false)` 表示目录为空。
fn scan_library_progressive(
    root: &Path,
    folder: &Path,
    show_hidden: bool,
    reverse: bool,
    canceled: impl Fn() -> bool,
    emit: &impl Fn(ScanUpdate),
) -> io::Result<bool> {
    let entries = sorted_entries(folder, show_hidden, reverse)?;
    if entries.is_empty() {
        return Ok(false);
    }
    let listing = entries.iter().map(placeholder_card).collect();
    emit(ScanUpdate::Listing(listing));

    let mut batch_start = 0usize;
    let mut batch_cards = Vec::with_capacity(FIRST_BATCH.min(entries.len()));
    let mut batch_attachments = HashMap::new();
    for (index, entry) in entries.into_iter().enumerate() {
        if canceled() {
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        let card = scan_entry(root, entry, show_hidden, &mut batch_attachments);
        batch_cards.push(card);
        if batch_cards.len() >= FIRST_BATCH {
            emit(ScanUpdate::Batch {
                start: batch_start,
                cards: std::mem::take(&mut batch_cards),
                attachments: std::mem::take(&mut batch_attachments),
            });
            batch_start = index + 1;
        }
    }
    if !batch_cards.is_empty() {
        emit(ScanUpdate::Batch {
            start: batch_start,
            cards: batch_cards,
            attachments: batch_attachments,
        });
    }
    Ok(true)
}

fn scan_library(
    root: &Path,
    folder: &Path,
    show_hidden: bool,
    reverse: bool,
    canceled: impl Fn() -> bool,
) -> io::Result<Snapshot> {
    let entries = sorted_entries(folder, show_hidden, reverse)?;
    let mut snapshot = Snapshot {
        cards: Vec::with_capacity(entries.len()),
        attachments: HashMap::new(),
    };
    for entry in entries {
        if canceled() {
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        let card = scan_entry(root, entry, show_hidden, &mut snapshot.attachments);
        snapshot.cards.push(card);
    }
    Ok(snapshot)
}

fn file_card(root: &Path, entry: DirEntry) -> Card {
    let (name, content) = if !entry.symlink
        && is_markdown(&entry.path)
        && let Some(lines) = read_page(root, &entry.path)
    {
        (note_name(&entry), CardContent::Note(lines))
    } else {
        let content = if !entry.symlink
            && let Some(kind) = AttachmentKind::of(&entry.path)
        {
            CardContent::Attachment(kind)
        } else {
            CardContent::File
        };
        (entry.name, content)
    };
    Card {
        path: entry.path,
        name,
        content,
    }
}

fn visible_entries(folder: &Path, show_hidden: bool) -> io::Result<Vec<DirEntry>> {
    let mut entries = list_dir(folder)?;
    if !show_hidden {
        entries.retain(|entry| !entry.name.starts_with('.'));
    }
    Ok(entries)
}

pub(super) fn entries(folder: &Path) -> io::Result<Vec<DirEntry>> {
    // 笔记索引只包含可见的 Markdown 文件，不跟随符号链接。
    Ok(visible_entries(folder, false)?
        .into_iter()
        .filter(|entry| {
            !entry.symlink && (entry.kind == EntryKind::Dir || is_markdown(&entry.path))
        })
        .collect())
}

fn note_name(entry: &DirEntry) -> String {
    entry
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&entry.name)
        .to_owned()
}

fn folder_preview(root: &Path, folder: &Path, show_hidden: bool) -> Option<FolderPreview> {
    use std::collections::VecDeque;
    use std::hash::BuildHasher;

    let children = visible_entries(folder, show_hidden).ok()?;
    let count = children.len();
    // Quiet MD picks the most recent real documents, including descendants. Bound disk work
    // in Minc because it does not keep Quiet's whole document store in memory.
    let mut pending = VecDeque::from([(children, 0)]);
    let mut files = Vec::new();
    let mut dirs = 0;
    while let Some((children, depth)) = pending.pop_front() {
        for entry in children {
            if entry.symlink {
                continue;
            }
            if entry.kind == EntryKind::Dir && dirs < 32 && depth < 8 {
                dirs += 1;
                if let Ok(children) = visible_entries(&entry.path, show_hidden) {
                    pending.push_back((children, depth + 1));
                }
            } else if entry.kind == EntryKind::File
                && (is_markdown(&entry.path) || AttachmentKind::of(&entry.path).is_some())
                && let Ok(meta) = winc_backend::fs::symlink_metadata(&entry.path)
                && meta.is_file()
            {
                files.push((meta.modified, entry));
                if files.len() >= 512 {
                    break;
                }
            }
        }
        if files.len() >= 512 {
            break;
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.path.cmp(&b.1.path)));
    let papers = files.len().min(3);
    let seeds = std::array::from_fn(|i| {
        files.get(i).map_or(1, |(_, entry)| {
            std::hash::BuildHasherDefault::<std::collections::hash_map::DefaultHasher>::default()
                .hash_one(&entry.path)
        })
    });
    let front = files
        .into_iter()
        .next()
        .map(|(_, entry)| Box::new(file_card(root, entry)));
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
fn read_page(root: &Path, path: &Path) -> Option<Vec<PreviewLine>> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file()
        || crate::system::is_dataless(&metadata)
        || !winc_config::files::canonicalize(path).is_ok_and(|path| path.starts_with(root))
    {
        return None;
    }
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(PREVIEW_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(preview_lines(&String::from_utf8_lossy(&bytes)))
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
