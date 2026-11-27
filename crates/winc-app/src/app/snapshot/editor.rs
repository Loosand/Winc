//! 编辑区的测试：编辑、多光标、折叠、格式化、导航、粘贴、滚动与滚动条、空白符、十六进制视图。

use super::*;

pub(super) mod cursors;
pub(super) mod editing;
pub(super) mod folding;
pub(super) mod navigation;
pub(super) mod paste;
pub(super) mod scrollbar;
pub(super) mod scrolling;
mod vim;
pub(super) mod whitespace;
