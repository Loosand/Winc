//! 路径所在的主机：本机或 Windows 上的某个 WSL 发行版。
//!
//! 在路径所在的主机上运行的程序（`git` 等）使用该主机上的路径写法，输出的也是该主机上的路径。
//! 本模块在两种写法之间换算，WSL 的换算见 [`crate::wsl`]。

use std::path::{Path, PathBuf};

/// 把在 `root` 所在主机上运行的程序输出的路径 `text` 转换为本程序使用的路径。
///
/// 绝对路径按该主机上的路径处理；其余路径相对于 `root`。
#[must_use]
pub fn resolve(root: &Path, text: &str) -> PathBuf {
    crate::wsl::resolve(root, text)
}

/// 返回把 `path` 传给在 `root` 所在主机上运行的程序时使用的写法。
///
/// 两者不在同一主机上时无法换算，返回 `path` 原样的文本；程序找不到该路径并报错。
#[must_use]
pub fn arg(root: &Path, path: &Path) -> String {
    crate::wsl::git_arg(root, path)
}
