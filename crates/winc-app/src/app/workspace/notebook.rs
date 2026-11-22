//! Navigation for the current local workspace.

use super::*;
use std::path::{Path, PathBuf};

const ROW: f32 = 30.0;
const HEADING: f32 = 25.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SidebarTarget {
    Search,
    Graph,
    Documents,
    FolderMenu,
}

#[derive(Default)]
pub(crate) struct Navigation {
    pub(crate) reader: Rect,
    pub(crate) search: Rect,
    pub(crate) graph: Rect,
    pub(crate) folder_menu: Rect,
    pub(crate) documents: Rect,
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

    pub(crate) fn notebook_documents_open(&self) -> bool {
        !self.panels.sidebar.documents_closed
    }

    pub(crate) fn place_notebook_sidebar(&mut self) {
        let hide_dot_entries = self.notebook_sidebar();
        if let Some(ws) = &mut self.editor.workspace {
            ws.set_hide_dot_entries(hide_dot_entries);
        }
        self.panels.sidebar.navigation = Navigation::default();
        if self.panels.sidebar.view != SidebarView::Files || self.library_root().is_none() {
            return;
        }
        if !self.notebook_sidebar() {
            let row = 2.0 * super::library::HOME_ROW;
            self.layout.tree.y += row;
            self.layout.tree.h = (self.layout.tree.h - row).max(0.0);
            return;
        }
        let sidebar = self.layout.sidebar;
        let home = self.library_home_button();
        if home.w <= 0.0 {
            return;
        }
        let row = |y, h| Rect::new(home.x, y, home.w, h).intersect(sidebar);
        let reader = row(home.bottom() + 2.0, ROW);
        let search = row(reader.bottom() + 2.0, ROW);
        let graph = row(search.bottom() + 2.0, ROW);
        let documents = row(graph.bottom() + 8.0, HEADING);
        let tree = &mut self.layout.tree;
        let bottom = tree.bottom();
        tree.y = documents.bottom().min(bottom);
        tree.h = if !self.panels.sidebar.documents_closed {
            (bottom - tree.y).max(0.0)
        } else {
            0.0
        };
        let menu_w = (documents.w - 88.0).clamp(26.0, 156.0);
        let folder_menu = Rect::new(
            documents.right() - menu_w - 2.0,
            documents.y + 1.0,
            menu_w,
            HEADING - 2.0,
        )
        .intersect(documents);
        self.panels.sidebar.navigation = Navigation {
            reader,
            search,
            graph,
            folder_menu,
            documents,
        };
    }

    pub(crate) fn notebook_folder_heading(&self) -> Option<Rect> {
        let rect = self.panels.sidebar.navigation.documents;
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
        self.open_context_menu(MenuTarget::NotebookWorkspaces(roots), at);
    }

    pub(in crate::app) fn notebook_sidebar_at(&self, x: f32, y: f32) -> Option<SidebarTarget> {
        let nav = &self.panels.sidebar.navigation;
        [
            (nav.search, SidebarTarget::Search),
            (nav.graph, SidebarTarget::Graph),
            (nav.folder_menu, SidebarTarget::FolderMenu),
            (nav.documents, SidebarTarget::Documents),
        ]
        .into_iter()
        .find_map(|(rect, target)| rect.contains(x, y).then_some(target))
    }

    pub(in crate::app) fn press_notebook_sidebar(&mut self, target: SidebarTarget) {
        match target {
            SidebarTarget::Search => self.open_notebook_search(),
            SidebarTarget::Graph => self.open_note_graph(None),
            SidebarTarget::FolderMenu => {
                let menu = self.panels.sidebar.navigation.folder_menu;
                self.open_notebook_workspaces((menu.x, menu.bottom()));
            }
            SidebarTarget::Documents => {
                self.panels.sidebar.documents_closed ^= true;
                self.relayout();
                self.clamp_tree_scroll();
                self.request_redraw();
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
        }
    }
}
