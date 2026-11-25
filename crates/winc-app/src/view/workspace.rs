//! 打开的目录和文件的绘制：侧栏（项目和分支、工具栏、文件树）、全局搜索面板、标签栏、开始页。

use super::*;

pub(super) mod library;
pub(super) mod note_search;
mod notebook;
pub(super) mod outline;
pub(super) mod panel;
pub(super) mod sidebar;
pub(super) mod start;
pub(super) mod top_bar;

pub(in crate::view) use super::controls::list::retracting_color;
pub(in crate::view) use super::controls::{over, segment_colors};
