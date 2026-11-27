//! 改动文件系统的操作：建目录、新建文件、改名、复制、删除。

#[cfg(test)]
mod tests;

use std::io::{self, Write};
use std::path::Path;

/// 创建目录 `path`；上级目录必须已存在。
///
/// # Errors
///
/// 与 `std::fs::create_dir` 相同。
pub fn create_dir(path: impl AsRef<Path>) -> io::Result<()> {
    std::fs::create_dir(path.as_ref())
}

/// 创建目录 `path` 及其不存在的各级上级目录；目录已存在时成功。
///
/// # Errors
///
/// 与 `std::fs::create_dir_all` 相同。
pub fn create_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    std::fs::create_dir_all(path.as_ref())
}

/// 新建文件 `path` 并写入 `contents`；`path` 上已有任何条目时不覆盖。
///
/// 写入失败时删除写了一部分的文件。
///
/// # Errors
///
/// `path` 已存在时返回 [`io::ErrorKind::AlreadyExists`]；其余与创建和写入文件相同。
pub fn create_new(path: impl AsRef<Path>, contents: &[u8]) -> io::Result<()> {
    let path = path.as_ref();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    if let Err(error) = file.write_all(contents) {
        let _ = std::fs::remove_file(path);
        return Err(error);
    }
    Ok(())
}

/// 把 `contents` 写入 `path`，文件已存在时覆盖全部内容。
///
/// 不经过临时文件，写入中途失败时文件只剩部分内容；文档的保存使用 [`save`](super::save)。
///
/// # Errors
///
/// 与 `std::fs::write` 相同。
pub fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
    std::fs::write(path.as_ref(), contents)
}

/// 把 `from` 改名为 `to`。
///
/// # Errors
///
/// 与 `std::fs::rename` 相同。
pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    std::fs::rename(from.as_ref(), to.as_ref())
}

/// 把文件 `from` 的内容和权限复制到 `to`，返回复制的字节数；`to` 已存在时覆盖。
///
/// # Errors
///
/// 与 `std::fs::copy` 相同。
pub fn copy(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<u64> {
    copy_watched(from, to, &|_| true)
}

/// 同 [`copy`]，复制前、后向 `watch` 报告进度；`watch` 返回假时停止复制。
///
/// # Errors
///
/// `watch` 返回假而停止时返回 [`io::ErrorKind::Interrupted`]，其余与 `std::fs::copy` 相同。
pub fn copy_watched(
    from: impl AsRef<Path>,
    to: impl AsRef<Path>,
    watch: &dyn Fn(u64) -> bool,
) -> io::Result<u64> {
    if !watch(0) {
        return Err(io::ErrorKind::Interrupted.into());
    }
    let copied = std::fs::copy(from.as_ref(), to.as_ref())?;
    watch(copied);
    Ok(copied)
}

/// 把 `path` 的权限位设为 `mode`（低 12 位）；本机是 Windows 时什么都不做。
///
/// # Errors
///
/// 与 `std::fs::set_permissions` 相同。
pub fn set_mode(path: impl AsRef<Path>, mode: u32) -> io::Result<()> {
    let (path, mode) = (path.as_ref(), mode & 0o7777);
    #[cfg(unix)]
    return std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(mode));
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

/// 把符号链接 `from` 复制为指向同一目标的符号链接 `to`，不复制目标的内容。
///
/// Windows 上链接分为指向目录的和指向文件的两种，按 `from` 此刻指向的目标选择；创建符号链接
/// 需要相应的权限。
///
/// # Errors
///
/// 读取或创建链接失败时返回对应的 `io::Error`。
pub fn copy_symlink(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    let (from, to) = (from.as_ref(), to.as_ref());
    let target = std::fs::read_link(from)?;
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, to);
    #[cfg(windows)]
    return if std::fs::metadata(from).is_ok_and(|m| m.is_dir()) {
        std::os::windows::fs::symlink_dir(target, to)
    } else {
        std::os::windows::fs::symlink_file(target, to)
    };
}

/// 删除文件 `path`（符号链接删除链接本身）。
///
/// # Errors
///
/// 与 `std::fs::remove_file` 相同。
pub fn remove_file(path: impl AsRef<Path>) -> io::Result<()> {
    std::fs::remove_file(path.as_ref())
}

/// 删除 `path`：文件和符号链接删除其本身，目录连同其中的全部内容一起删除。不可恢复。
///
/// # Errors
///
/// `path` 不存在，或其中有无法删除的项时返回对应的 `std::io::Error`；后一种情况下已删除的
/// 部分不恢复。
pub fn remove_all(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref();
    if std::fs::symlink_metadata(path)?.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}
