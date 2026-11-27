//! 外部程序：获取用户 shell 的环境、带超时和取消地运行程序，以及为主机换算路径。
//!
//! [`shell`] 提供环境与进程运行，[`host`] 为运行在其他主机上的程序换算路径。

pub(crate) mod copying;
pub mod host;
pub mod shell;
pub mod wsl;
