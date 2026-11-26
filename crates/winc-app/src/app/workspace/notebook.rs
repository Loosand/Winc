//! Navigation for the current local workspace.

use super::*;
use std::path::{Path, PathBuf};

const ROW: f32 = 30.0;
const FOOTER: f32 = 34.0;
const RECENT_LIMIT: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SidebarTarget {
    Search,
    BackToFiles,
    TreeMenu,
    FolderMenu,
    NewFile,
    OpenFolder,
    Recent(usize),
    MoreFolders,
    FolderNew(usize),
    FolderContext(usize),
}

#[derive(Default)]
pub(crate) struct Navigation {
    pub(crate) search: Rect,
    pub(crate) back: Rect,
    pub(crate) tree_menu: Rect,
    pub(crate) folder_menu: Rect,
    pub(crate) new_file: Rect,
    pub(crate) open_folder: Rect,
    pub(crate) recent: Vec<(Rect, usize)>,
    pub(crate) more: Rect,
}

pub(crate) fn note_in(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root).is_ok_and(|relative| {
        !relative.as_os_str().is_empty()
            && relative
                .components()
                .all(|part| !part.as_os_str().to_string_lossy().starts_with('.'))
            && winc_preview::markdown::is_markdown(path)
    })
}

impl App {
    /// Local writing windows use one document instead of a tab strip.
    pub(crate) fn writing_workspace(&self) -> bool {
        #[cfg(test)]
        if self.panels.sidebar.legacy_chrome {
            return false;
        }
        self.editor
            .root()
            .is_none_or(|root| !winc_backend::is_remote(root))
    }

    pub(crate) fn notebook_sidebar(&self) -> bool {
        self.writing_workspace() && self.library_root().is_some()
    }

    pub(crate) fn note_in_library(&self, path: &Path) -> bool {
        self.library_root().is_some_and(|root| note_in(root, path))
    }

    pub(crate) fn place_notebook_sidebar(&mut self) {
        let notebook = self.notebook_sidebar();
        if let Some(ws) = &mut self.editor.workspace {
            ws.set_hide_dot_entries(notebook && !self.panels.sidebar.show_hidden_files);
            ws.set_reverse_name_order(notebook && self.panels.sidebar.reverse_name_order);
        }
        self.panels.sidebar.navigation = Navigation::default();
        if !self.writing_workspace() {
            if self.panels.sidebar.view == SidebarView::Files && self.library_root().is_some() {
                let row = super::library::HOME_ROW;
                self.layout.tree.y += row;
                self.layout.tree.h = (self.layout.tree.h - row).max(0.0);
            }
            return;
        }

        let side = self.layout.sidebar;
        let bar = self.layout.sidebar_header;
        let header = Rect::new(bar.x + 8.0, bar.y, bar.w - 16.0, 25.0);
        let search_close = if self.panels.sidebar.view == SidebarView::Search {
            self.panel_parts().close
        } else {
            Rect::default()
        };
        let nav = &mut self.panels.sidebar.navigation;
        let square = |x| Rect::new(x, header.y, header.h, header.h).intersect(side);

        match (notebook, self.panels.sidebar.view) {
            (true, SidebarView::Files) => {
                use crate::theme::BUTTON_GAP;
                let toggle = self.layout.sidebar_button;
                let step = toggle.w + BUTTON_GAP;
                let right = header.right();
                let tool = |offset| {
                    let x = right - offset;
                    if x >= toggle.right() + BUTTON_GAP {
                        Rect::new(x, toggle.y, toggle.w, toggle.h)
                    } else {
                        Rect::default()
                    }
                };
                nav.tree_menu = tool(toggle.w);
                nav.search = tool(toggle.w + step);
                nav.folder_menu = Rect::new(
                    header.x,
                    bar.y + (bar.h - header.h) / 2.0,
                    header.w,
                    header.h,
                )
                .intersect(side);
                let tree = &mut self.layout.tree;
                let footer_height = tree.h.min(FOOTER);
                tree.h -= footer_height;
                nav.new_file = Rect::new(
                    header.x,
                    tree.bottom() + 3.0,
                    header.w,
                    (footer_height - 6.0).max(0.0),
                )
                .intersect(side);
            }
            (true, SidebarView::Search) => nav.back = search_close,
            (true, SidebarView::Outline) => nav.back = square(header.x),
            (false, SidebarView::Files) => {
                let width = (side.w - 44.0).clamp(0.0, 212.0);
                let x = side.x + (side.w - width) / 2.0;
                let recent = self
                    .state
                    .recent
                    .iter()
                    .enumerate()
                    .filter_map(|(i, path)| (!winc_backend::is_remote(path)).then_some(i))
                    .take(RECENT_LIMIT)
                    .collect::<Vec<_>>();
                let top = header.bottom();
                let height = 64.0 + ROW * (recent.len() + 2) as f32;
                let mut y = top + ((side.bottom() - top - height) * 0.46).max(0.0) + 56.0;
                let clipped = |y| Rect::new(x, y, width, ROW).intersect(side);
                let button_width = 133.0_f32.min(width);
                nav.open_folder = Rect::new(x + (width - button_width) / 2.0, y, button_width, ROW)
                    .intersect(side);
                y += ROW + 14.0;
                for i in recent {
                    nav.recent.push((clipped(y), i));
                    y += ROW;
                }
                nav.more = clipped(y);
                self.layout.tree.h = 0.0;
            }
            _ => {}
        }
    }

    pub(crate) fn notebook_menu_open(&self, target: SidebarTarget) -> bool {
        use crate::app::input::context_menu::MenuTarget;
        let menu = self.overlays.context_menu.as_ref().map(|menu| &menu.target);
        matches!(
            (target, menu),
            (
                SidebarTarget::FolderMenu,
                Some(MenuTarget::NotebookWorkspaces { .. })
            ) | (SidebarTarget::TreeMenu, Some(MenuTarget::TreeTools))
        )
    }

    pub(crate) fn notebook_folder_heading(&self) -> Option<Rect> {
        let rect = self.panels.sidebar.navigation.folder_menu;
        (rect.w > 0.0 && rect.h > 0.0).then_some(rect)
    }

    pub(in crate::app) fn open_notebook_workspaces(&mut self, at: (f32, f32)) {
        use crate::app::input::context_menu::MenuTarget;
        let Some(root) = self.library_root() else {
            return;
        };
        let roots = std::iter::once(root)
            .chain(
                self.state
                    .notebook_workspaces
                    .iter()
                    .map(PathBuf::as_path)
                    .filter(|path| *path != root),
            )
            .map(Path::to_path_buf)
            .collect();
        self.open_context_menu(
            MenuTarget::NotebookWorkspaces {
                roots,
                typora: self
                    .window
                    .handle
                    .as_ref()
                    .and_then(|_| crate::system::typora_app()),
            },
            at,
        );
    }

    pub(crate) fn notebook_folder_actions(&self, x: f32, y: f32) -> Option<(usize, Rect, Rect)> {
        let tree = self.layout.tree;
        if !self.notebook_sidebar()
            || self.panels.sidebar.view != SidebarView::Files
            || !tree.contains(x, y)
            || !matches!(self.pointer.drag, Drag::None)
            || self.panels.name_input.is_some()
        {
            return None;
        }
        let index = self.tree_row_at(y)?;
        let workspace = self.editor.workspace.as_ref()?;
        let entry = workspace.rows().get(index)?;
        if entry.kind != winc_editor::EntryKind::Dir {
            return None;
        }
        let top = self
            .tree_sticky_at(y)
            .map_or_else(|| self.tree_row_top(index), |row| row.rect.y);
        let button = |right| {
            Rect::new(
                right - 20.0,
                top + (self.tree_row_height() - 20.0) / 2.0,
                20.0,
                20.0,
            )
            .intersect(tree)
        };
        let more = button(tree.right() - 14.0);
        let add = button(more.x - 1.0);
        Some((index, add, more))
    }

    pub(in crate::app) fn notebook_sidebar_at(&self, x: f32, y: f32) -> Option<SidebarTarget> {
        let nav = &self.panels.sidebar.navigation;
        [
            (nav.search, SidebarTarget::Search),
            (nav.back, SidebarTarget::BackToFiles),
            (nav.tree_menu, SidebarTarget::TreeMenu),
            (nav.folder_menu, SidebarTarget::FolderMenu),
            (nav.new_file, SidebarTarget::NewFile),
            (nav.open_folder, SidebarTarget::OpenFolder),
            (nav.more, SidebarTarget::MoreFolders),
        ]
        .into_iter()
        .chain(
            nav.recent
                .iter()
                .map(|&(r, i)| (r, SidebarTarget::Recent(i))),
        )
        .find_map(|(rect, target)| rect.contains(x, y).then_some(target))
        .or_else(|| {
            let (index, new, more) = self.notebook_folder_actions(x, y)?;
            if new.contains(x, y) {
                Some(SidebarTarget::FolderNew(index))
            } else {
                more.contains(x, y)
                    .then_some(SidebarTarget::FolderContext(index))
            }
        })
    }

    pub(in crate::app) fn press_notebook_sidebar(&mut self, target: SidebarTarget) {
        use crate::app::input::context_menu::{MenuTarget, menu_below};
        match target {
            SidebarTarget::Search => self.focus_global_search(false, None),
            SidebarTarget::BackToFiles => self.show_files(),
            SidebarTarget::FolderMenu => {
                let menu = self.panels.sidebar.navigation.folder_menu;
                self.open_notebook_workspaces((menu.x, menu.bottom() + 5.0));
            }
            SidebarTarget::TreeMenu => {
                let menu = self.panels.sidebar.navigation.tree_menu;
                self.open_context_menu(MenuTarget::TreeTools, menu_below(menu));
            }
            SidebarTarget::NewFile => {
                if let Some(ws) = &mut self.editor.workspace {
                    ws.select(None);
                }
                self.start_new_entry(false);
            }
            SidebarTarget::OpenFolder => self.pick(Pick::OpenFolder),
            SidebarTarget::Recent(i) => self.run_start_action(StartAction::Recent(i)),
            SidebarTarget::MoreFolders => {
                let button = self.panels.sidebar.navigation.more;
                self.open_context_menu(MenuTarget::Project, menu_below(button));
            }
            SidebarTarget::FolderNew(index) | SidebarTarget::FolderContext(index) => {
                let Some(ws) = &mut self.editor.workspace else {
                    return;
                };
                let Some(entry) = ws.rows().get(index) else {
                    return;
                };
                let path = entry.path.clone();
                ws.select(Some(index));
                self.focus = Focus::Tree;
                if target == SidebarTarget::FolderNew(index) {
                    self.start_new_entry(false);
                } else {
                    self.open_context_menu(MenuTarget::Tree(Some(path)), self.pointer.mouse);
                }
            }
        }
    }

    pub(super) fn note_edit_saved(&mut self, id: DocumentId, edited: bool) {
        if !edited {
            return;
        }
        if let Some(path) = self
            .editor
            .document(id)
            .and_then(|doc| doc.path.clone())
            .filter(|path| self.note_in_library(path))
        {
            self.note_links_changed(false, std::slice::from_ref(&path));
            self.notebook_search_changed(false, std::slice::from_ref(&path));
        }
    }
}
