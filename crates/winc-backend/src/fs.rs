//! 文件操作：函数名和语义与 `std::fs` 中的同名函数相同，返回统一的 [`Metadata`]。

mod change;
mod dir;
mod open;
mod save;
#[cfg(test)]
mod tests;

pub use change::{
    copy, copy_symlink, copy_watched, create_dir, create_dir_all, create_new, remove_all,
    remove_file, rename, set_mode, write,
};
pub use dir::{Entry, ReadDir, read_dir};
pub use open::{Handle, Opened, open};
pub use save::{BeforeReplace, SaveError, SaveOptions, save};

use std::io;
use std::path::{Path, PathBuf};

pub use crate::meta::{Kind, Metadata, Stamp, Unix};

/// 返回 `path` 的文件信息，跟随符号链接。
///
/// # Errors
///
/// 与 `std::fs::metadata` 相同。
pub fn metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
    std::fs::metadata(path.as_ref()).map(|m| Metadata::from(&m))
}

/// 返回 `path` 的文件信息，不跟随符号链接。
///
/// # Errors
///
/// 与 `std::fs::symlink_metadata` 相同。
pub fn symlink_metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
    std::fs::symlink_metadata(path.as_ref()).map(|m| Metadata::from(&m))
}

/// 对若干路径逐个取文件信息（跟随符号链接），结果与 `paths` 的顺序一一对应。
pub fn metadata_many<P: AsRef<Path>>(paths: &[P]) -> Vec<io::Result<Metadata>> {
    paths
        .iter()
        .map(|path| std::fs::metadata(path.as_ref()).map(|m| Metadata::from(&m)))
        .collect()
}

/// 是否表示文件不存在：路径上没有该项，或路径中间的一段不是目录。
///
/// 其他错误（权限不足等）不说明文件是否存在，不能据此认为文件已被删除。
pub fn is_absent(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

/// 是否存在并且是普通文件（跟随符号链接）；无法确定时为否。
pub fn is_file(path: impl AsRef<Path>) -> bool {
    metadata(path).is_ok_and(|m| m.is_file())
}

/// 是否存在并且是目录（跟随符号链接）；无法确定时为否。
pub fn is_dir(path: impl AsRef<Path>) -> bool {
    metadata(path).is_ok_and(|m| m.is_dir())
}

/// 是否为符号链接（不跟随链接，指向不存在目标的链接同样为是）；无法确定时为否。
pub fn is_symlink(path: impl AsRef<Path>) -> bool {
    symlink_metadata(path).is_ok_and(|m| m.is_symlink())
}

/// 是否存在（跟随符号链接，指向不存在目标的符号链接不算存在）；无法确定时为否。
pub fn exists(path: impl AsRef<Path>) -> bool {
    metadata(path).is_ok()
}

/// 读取 `path` 的全部内容。
///
/// # Errors
///
/// 与 `std::fs::read` 相同。
pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    std::fs::read(path.as_ref())
}

/// 读取 `path` 的全部内容并按 UTF-8 解码。
///
/// # Errors
///
/// 与 `std::fs::read_to_string` 相同。
pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    std::fs::read_to_string(path.as_ref())
}

/// 返回符号链接 `path` 指向的目标。
///
/// # Errors
///
/// 与 `std::fs::read_link` 相同。
pub fn read_link(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    std::fs::read_link(path.as_ref())
}
