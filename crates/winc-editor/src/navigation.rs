//! 定位：文件内的符号、配置文件的键路径。
//!
//! - `symbols`：提取文件内的函数、类型、标题等符号，供转到符号与大纲使用。
//! - `keypath`：计算光标在 JSON、YAML、TOML 文件中的键路径。
//!
//! 各模块只处理调用方传入的文本，不访问磁盘，也不依赖界面。

pub mod keypath;
pub mod symbols;
