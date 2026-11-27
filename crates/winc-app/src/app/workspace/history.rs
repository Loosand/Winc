//! The file and directory history of a single-document writing window.

use super::*;

const LIMIT: usize = 50;

#[derive(Clone)]
struct Visit {
    path: PathBuf,
    folder: bool,
}

#[derive(Default)]
pub(in crate::app) struct PathHistory {
    visits: Vec<Visit>,
    current: usize,
    replaying: bool,
}

impl PathHistory {
    fn next(&self, forward: bool) -> Option<usize> {
        if forward {
            let next = self.current + 1;
            (next < self.visits.len()).then_some(next)
        } else {
            self.current.checked_sub(1)
        }
    }

    pub(super) fn relocate(&mut self, from: &Path, to: &Path) {
        for visit in &mut self.visits {
            if let Ok(rest) = visit.path.strip_prefix(from) {
                visit.path = if rest.as_os_str().is_empty() {
                    to.to_path_buf()
                } else {
                    to.join(rest)
                };
            }
        }
    }
}

impl App {
    fn workspace_visit(&self) -> Option<(&Path, bool)> {
        if !self.writing_workspace() || self.editor.documents.len() > 1 {
            return None;
        }
        if self.library_shown() {
            let folder = self
                .library
                .folder
                .as_deref()
                .or_else(|| self.library_root())?;
            return Some((folder, true));
        }
        let path = self.editor.active()?.path.as_deref()?;
        Some((path, false))
    }

    pub(in crate::app) fn remember_workspace_visit(&mut self) {
        let history = &self.jumps.paths;
        let Some((path, folder)) = self.workspace_visit() else {
            return;
        };
        if history.replaying
            || history
                .visits
                .get(history.current)
                .is_some_and(|visit| visit.path == path && visit.folder == folder)
        {
            return;
        }
        let visit = Visit {
            path: path.to_path_buf(),
            folder,
        };
        let history = &mut self.jumps.paths;
        history.visits.truncate(history.current + 1);
        history.visits.push(visit);
        if history.visits.len() > LIMIT {
            history.visits.remove(0);
        }
        history.current = history.visits.len() - 1;
        self.request_redraw();
    }

    pub(crate) fn can_step_workspace_path(&self, forward: bool) -> bool {
        self.workspace_visit().is_some() && self.jumps.paths.next(forward).is_some()
    }

    pub(in crate::app) fn step_workspace_path(&mut self, forward: bool) {
        if !self.can_step_workspace_path(forward) {
            return;
        }
        self.remember_workspace_visit();
        while let Some(index) = self.jumps.paths.next(forward) {
            let visit = self.jumps.paths.visits[index].clone();
            let path = &visit.path;
            let exists = if visit.folder {
                self.library_root()
                    .is_some_and(|root| path.starts_with(root))
                    && self.is_folder(path)
            } else {
                self.may_be_file(path)
            };
            if !exists {
                let history = &mut self.jumps.paths;
                history.visits.remove(index);
                if index < history.current {
                    history.current -= 1;
                }
                continue;
            }

            self.jumps.paths.replaying = true;
            if visit.folder {
                self.browse_library_folder(visit.path.clone(), Focus::Editor);
            } else {
                self.open_file(path, false);
            }
            self.jumps.paths.replaying = false;
            // A canceled save or a write conflict leaves both the view and the history in place.
            if self.workspace_visit() == Some((path.as_path(), visit.folder)) {
                self.jumps.paths.current = index;
            }
            break;
        }
        self.request_redraw();
    }
}
