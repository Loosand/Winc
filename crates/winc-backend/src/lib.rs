//! 文件系统的访问入口：提供原子保存、统一元数据与跨平台文件操作。
//!
//! 文件操作在 [`fs`] 中，上层代码经它访问文件，不直接调用 `std::fs`。

pub mod fs;
pub mod local;
mod meta;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
