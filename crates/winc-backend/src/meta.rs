//! 文件信息与种类，供本机与远端文件操作共用。

use std::time::{SystemTime, UNIX_EPOCH};

/// 文件的种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// 普通文件。
    File,
    /// 目录。
    Dir,
    /// 符号链接；只出现在不跟随符号链接的文件信息中。
    Symlink,
    /// 其他：设备、管道、套接字等。
    Other,
}

impl Kind {
    /// 是否为普通文件。
    pub fn is_file(self) -> bool {
        self == Self::File
    }

    /// 是否为目录。
    pub fn is_dir(self) -> bool {
        self == Self::Dir
    }

    /// 是否为符号链接。
    pub fn is_symlink(self) -> bool {
        self == Self::Symlink
    }
}

impl From<std::fs::FileType> for Kind {
    fn from(file_type: std::fs::FileType) -> Self {
        if file_type.is_symlink() {
            Self::Symlink
        } else if file_type.is_dir() {
            Self::Dir
        } else if file_type.is_file() {
            Self::File
        } else {
            Self::Other
        }
    }
}

/// 修改时间：自 Unix 纪元起的秒数和秒内的纳秒数（0 到 999 999 999），纪元之前秒数为负。
///
/// 只用于判断文件是否发生变化，按原样保存和比较，不换算为本机的时间类型：各系统时间类型的
/// 精度不同，换算会丢失远端文件系统的纳秒部分。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Stamp {
    /// 自 Unix 纪元起的秒数。
    pub secs: i64,
    /// 秒内的纳秒数。
    pub nanos: u32,
}

impl From<SystemTime> for Stamp {
    fn from(time: SystemTime) -> Self {
        match time.duration_since(UNIX_EPOCH) {
            Ok(after) => Self {
                secs: after.as_secs().try_into().unwrap_or(i64::MAX),
                nanos: after.subsec_nanos(),
            },
            // 纪元之前：秒数向下取整，纳秒部分保持非负。
            Err(error) => {
                let before = error.duration();
                let secs: i64 = before.as_secs().try_into().unwrap_or(i64::MAX);
                if before.subsec_nanos() == 0 {
                    Self {
                        secs: -secs,
                        nanos: 0,
                    }
                } else {
                    Self {
                        secs: -secs - 1,
                        nanos: 1_000_000_000 - before.subsec_nanos(),
                    }
                }
            }
        }
    }
}

/// Unix 系统上的文件属性。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unix {
    /// 文件类型与权限位（`st_mode`）。
    pub mode: u32,
    /// 属主的用户编号。
    pub uid: u32,
    /// 硬链接数。
    pub nlink: u64,
    /// 所在设备的编号；与 `ino` 一起标识同一个文件。
    pub dev: u64,
    /// inode 编号。
    pub ino: u64,
}

/// 文件信息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metadata {
    /// 种类。
    pub kind: Kind,
    /// 大小（字节）。
    pub len: u64,
    /// 修改时间；文件系统不提供时为 `None`。
    pub modified: Option<Stamp>,
    /// 是否只读：Unix 上所有写权限位均未设置，Windows 上设置了只读属性。
    pub readonly: bool,
    /// Unix 系统上的属性；文件位于 Windows 时为 `None`。
    pub unix: Option<Unix>,
}

impl Metadata {
    /// 是否为普通文件。
    pub fn is_file(&self) -> bool {
        self.kind.is_file()
    }

    /// 是否为目录。
    pub fn is_dir(&self) -> bool {
        self.kind.is_dir()
    }

    /// 是否为符号链接。
    pub fn is_symlink(&self) -> bool {
        self.kind.is_symlink()
    }
}

impl From<&std::fs::Metadata> for Metadata {
    fn from(metadata: &std::fs::Metadata) -> Self {
        let kind = Kind::from(metadata.file_type());
        #[cfg(unix)]
        let unix = {
            use std::os::unix::fs::MetadataExt;
            Some(Unix {
                mode: metadata.mode(),
                uid: metadata.uid(),
                nlink: metadata.nlink(),
                dev: metadata.dev(),
                ino: metadata.ino(),
            })
        };
        #[cfg(not(unix))]
        let unix = None;
        Self {
            kind,
            len: metadata.len(),
            modified: metadata.modified().ok().map(Stamp::from),
            readonly: metadata.permissions().readonly(),
            unix,
        }
    }
}
