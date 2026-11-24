//! iCloud status for the visible file tree. File metadata is read on a worker.

use std::collections::HashMap;
use std::time::Instant;

use winc_i18n::t;

use super::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(
    not(target_os = "macos"),
    allow(dead_code, reason = "iCloud statuses come from macOS")
)]
pub(crate) enum CloudStatus {
    OnlineOnly,
    Downloading,
    Uploading,
    Failed,
}

impl CloudStatus {
    pub(crate) fn transferring(self) -> bool {
        matches!(self, Self::Downloading | Self::Uploading)
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::OnlineOnly => t!("workspace-icloud-online-only"),
            Self::Downloading => t!("workspace-icloud-downloading"),
            Self::Uploading => t!("workspace-icloud-uploading"),
            Self::Failed => t!("workspace-icloud-error"),
        }
    }
}

#[derive(Default)]
pub(crate) struct CloudRows {
    statuses: HashMap<PathBuf, CloudStatus>,
    #[cfg(target_os = "macos")]
    probe: Probe,
}

#[cfg(test)]
impl CloudRows {
    pub(in crate::app) fn set_status(&mut self, path: PathBuf, status: CloudStatus) {
        self.statuses.insert(path, status);
    }
}

impl App {
    pub(crate) fn cloud_status(&self, path: &Path) -> Option<CloudStatus> {
        let status = *self.panels.tree.cloud.statuses.get(path)?;
        if status == CloudStatus::OnlineOnly
            && self
                .editor
                .active()
                .is_some_and(|doc| doc.path.as_deref() == Some(path) && doc.is_pending())
        {
            return Some(CloudStatus::Downloading);
        }
        Some(status)
    }
}

#[cfg(target_os = "macos")]
mod polling;
#[cfg(target_os = "macos")]
use polling::Probe;

#[cfg(not(target_os = "macos"))]
impl App {
    pub(in crate::app) fn cloud_rows_tick(&mut self, _: Instant) -> Option<Instant> {
        None
    }
}
