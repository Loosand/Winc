//! 保存：把内容原子地写入文件，全部步骤在文件所在的机器上执行。
//!
//! 先写同目录下的临时文件，落盘后恢复原文件的权限和属组，再改名替换目标文件；断电或进程中止后
//! 磁盘上是完整的旧文件或新文件。改名替换不可行或会改变文件属性时退回原地写入，见 [`save`]。

#[cfg(test)]
mod tests;

use std::io::{self, Write};
use std::path::Path;

pub use crate::local::save::{BeforeReplace, SaveError, SaveOptions};
use crate::local::save_local;
use crate::meta::Metadata;
use crate::{locate, remote};

/// 把 `write` 写出的内容保存到 `path`，返回写入后的文件信息。
///
/// 文件所在的目录不存在时先创建。已有文件的权限和属组保留（macOS 上还有扩展属性和 ACL）；
/// 新文件使用默认权限。写入失败时删除临时文件，目标文件不变。
///
/// # Errors
///
/// 见 [`SaveError`]；`path` 是虚拟路径时还有以 [`SaveError::Io`] 返回的连接的错误。
pub fn save(
    path: impl AsRef<Path>,
    options: &SaveOptions<'_>,
    write: impl FnOnce(&mut dyn Write) -> io::Result<()>,
) -> Result<Metadata, SaveError> {
    let path = path.as_ref();
    // 远端：内容传给服务进程，由它在文件所在的机器上执行下面的同一套流程。两个只用于 WSL
    // 的选项不传递。
    if let Some(at) = locate(path) {
        let connection = remote::connection(&at.target)?;
        return connection.save(&at.path, options.bom, options.in_memory, write);
    }
    save_local(path, options, write)
}
