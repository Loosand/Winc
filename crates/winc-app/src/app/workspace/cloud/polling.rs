use std::sync::mpsc;
use std::time::Duration;

use super::*;

const INTERVAL: Duration = Duration::from_secs(1);

struct Sample {
    root: PathBuf,
    cloud: bool,
    statuses: HashMap<PathBuf, CloudStatus>,
}

pub(super) struct Probe {
    root: Option<PathBuf>,
    cloud: Option<bool>,
    due: Option<Instant>,
    running: bool,
    results: (mpsc::Sender<Sample>, mpsc::Receiver<Sample>),
}

impl Default for Probe {
    fn default() -> Self {
        Self {
            root: None,
            cloud: None,
            due: None,
            running: false,
            results: mpsc::channel(),
        }
    }
}

impl App {
    pub(in crate::app) fn cloud_rows_tick(&mut self, now: Instant) -> Option<Instant> {
        if !self.window.focused
            || self.unseen()
            || !self.sidebar_open()
            || self.panels.sidebar.view != crate::layout::SidebarView::Files
            || !self.notebook_sidebar()
        {
            return None;
        }
        let tree = &mut self.panels.tree;
        let ws = self.editor.workspace.as_ref()?;
        let rows = &mut tree.cloud;
        let probe = &mut rows.probe;
        if probe.root.as_ref() != Some(&ws.root) {
            probe.root = Some(ws.root.clone());
            probe.cloud = None;
            probe.due = None;
            rows.statuses.clear();
        }
        if probe.running || probe.cloud == Some(false) {
            return None;
        }
        if let Some(due) = probe.due.filter(|due| *due > now) {
            return Some(due);
        }

        // The tree already lays out a flat slice of the expanded rows, so use the same scroll
        // offset and row height to cap the metadata work at roughly one screenful.
        let height = self.tree_row_height();
        let first = (self.panels.tree.scroll / height).floor() as usize;
        let count = (self.layout.tree.h / height).ceil() as usize + 2;
        let paths: Vec<_> = ws
            .rows()
            .iter()
            .skip(first)
            .take(count)
            .filter(|entry| entry.kind == winc_editor::EntryKind::File && !entry.symlink)
            .map(|entry| entry.path.clone())
            .collect();
        let root = ws.root.clone();
        let probe = &mut self.panels.tree.cloud.probe;
        let cloud = probe.cloud;
        probe.running = true;
        let due = now + INTERVAL;
        probe.due = Some(due);
        let sender = probe.results.0.clone();
        self.spawn_blocking_then(UserEvent::CloudRows, move || {
            let cloud = cloud.unwrap_or_else(|| {
                crate::system::is_icloud_folder(&root) || crate::macos::is_icloud_item(&root)
            });
            let statuses = if cloud {
                paths
                    .into_iter()
                    .filter_map(|path| {
                        let status = crate::macos::icloud_file_status(&path)?;
                        Some((path, status))
                    })
                    .collect()
            } else {
                HashMap::new()
            };
            let _ = sender.send(Sample {
                root,
                cloud,
                statuses,
            });
        });
        Some(due)
    }

    pub(in crate::app) fn poll_cloud_rows(&mut self) {
        let root = self.library_root().map(Path::to_path_buf);
        let rows = &mut self.panels.tree.cloud;
        let Ok(sample) = rows.probe.results.1.try_recv() else {
            return;
        };
        rows.probe.running = false;
        if root.as_ref() != Some(&sample.root) {
            return;
        }
        rows.probe.cloud = Some(sample.cloud);
        if rows.statuses != sample.statuses {
            rows.statuses = sample.statuses;
            self.request_redraw();
        }
    }
}
