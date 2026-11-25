<p align="center">
  <img src="./crates/winc-app/assets/icon.svg" alt="Minc" width="128" />
</p>

<h1 align="center">Minc</h1>

<p align="center">
  本地优先的极速 Markdown 写作软件，为专注写作与 Agent 协作而生。
</p>

<p align="center">
  <a href="./README.md">English</a> | 简体中文
</p>

<p align="center">
  <a href="./LICENSE"><img alt="License" src="https://img.shields.io/badge/License-Apache%202.0-blue" /></a>
  <img alt="Rust" src="https://img.shields.io/badge/Rust-2024-b7410e" />
  <img alt="Platforms" src="https://img.shields.io/badge/Platforms-macOS%20%7C%20Linux%20%7C%20Windows-555" />
</p>

## 概述

Minc 基于 Rust 与自研渲染引擎构建，不依赖 Electron 或 WebView，目标是在保持原生级启动与输入响应的前提下，提供类似 Typora 的所见即所得 Markdown 写作体验。

所有笔记始终以普通 `.md` 文件保存在本地真实目录中，不引入专有数据库或云端锁定。

| 指标 | 结果 |
| --- | --- |
| 冷启动到第一帧 | ~119 ms（macOS 20 次中位数） |
| 打开 10 万行时的常驻内存 | ~52 MB |
| 105 KiB 混合 Markdown 输入首帧 | ~12 ms（p50） |

以上数据在 macOS 开发机测得，具体表现随硬件与系统负载有所差异。

## 核心能力

| 方面 | 能力 |
| --- | --- |
| 实时排版写作 | 单一可编辑排版视图（所见即所得），支持数学公式、Mermaid 图表、中文 IME 连续录入、网页表格与富文本粘贴保真 |
| 纯本地笔记库 | 直接打开任意本地文件夹作为笔记库，提供纸页式卡片主页、文件树与 `⌘P` 全文快速检索 |
| 笔记属性 | 正文顶部可折叠属性面板，支持文本、数字、日期、复选框、单选/多选等类型，值存为标准 YAML Frontmatter |
| 链接与反链 | 支持标准 Markdown 链接与 `[[WikiLink]]` 双链跳转，目标页底部展示反向引用列表与上下文回跳 |
| 参考资料预览 | 内置图片、SVG、PDF（macOS / Windows）、Word（docx）、表格（xlsx / csv）与 JSON 结构图本地预览 |
| 外观与语言 | 内置 16 套浅色与深色主题，完整支持简体中文与英文界面，支持触控板缩放与触屏手势 |

## 演进方向：常青写作层 + Agent 协作层

Minc 采用「两层 + 薄边界」的长期架构，把变化慢的写作体验与变化快的 LLM/Agent 能力物理隔离：

- **常青层（本地写作宿主）**：守住启动速度、输入手感、精准撤销与纯本地 `.md` 文件所有权，同时在编辑层沉淀四项通用人机协作原语——行内批注、选区 Diff 与单步撤销、可折叠侧栏流、工作区文件安全回写。
- **演进层（外部 Agent 运行时）**：通过进程隔离的 RPC 协议按需接入外部 LLM/Agent 能力，承接错别字校对、翻译润色、联网检索落盘、事实核查、配图生成及 Skills 工作流；未启用 AI 时不拉起子进程，不增加编辑器启动与内存负担。

## 支持平台

| 平台 | 系统要求 |
| --- | --- |
| macOS | macOS 11 及以上（Apple 芯片 / Intel） |
| Linux | x86_64（X11 / Wayland） |
| Windows | Windows 10 1809 及以上（x86_64） |

当前主要在 macOS 上开发与验收。

## 从源码构建

工具链版本由 `rust-toolchain.toml` 自动指定：

```bash
cargo run --release                 # 构建并运行
cargo run --release -- <笔记目录>    # 打开指定文件夹或 Markdown 文件
```

## 本地开发与验证

在 macOS 上可运行本地热重载脚本进行桌面验收（使用独立隔离的测试配置与会话，不影响正式资料）：

```bash
python3 scripts/dev.py              # 监听改动、自动编译并重启开发版应用
python3 scripts/dev.py --once       # 仅构建并启动一次
```

常用测试与基准命令：

```bash
cargo xtask test                    # 测试相对 HEAD 有变动的 crate
cargo xtask test --all              # 运行整个 workspace 测试
cargo clippy --workspace --all-targets -- -D warnings
cargo xtask bench                   # 运行启动、内存与滚动性能基准
```

## 模块结构

界面由 Rust 编写，窗口使用 `winit`，渲染由自研 CPU 光栅化完成（macOS 上直接写入 `IOSurface`）。核心 crate 分层如下：

| Crate | 职责 |
| --- | --- |
| `winc-core` | 文本缓冲区（Rope）、编辑命令、撤销栈与词级 Diff |
| `winc-editor` | 文档管理、文件树、项目搜索等编辑逻辑 |
| `winc-preview` | Markdown 解析、笔记链接索引，以及图片、SVG、PDF、docx、表格、JSON 预览 |
| `winc-typeset` | Markdown 实时排版布局 |
| `winc-render` | 字形处理、光栅化与画面呈现 |
| `winc-watch` | 本地工作区目录文件变更监听 |
| `winc-config` | 配置、主题与会话状态持久化 |
| `winc-i18n` | 界面多语言文案（Fluent） |
| `winc-app` | 桌面应用本体：窗口、事件分发与各视图界面 |
| `winc` | 可执行程序入口 |
| `xtask` | 测试、性能基准与构建自动化脚本 |

> 注：仓库仍保留部分源自上游代码编辑器的底层模块（如 `winc-syntax`、`winc-vim`、`winc-pty`、`winc-backend`、`winc-server`），后续将按路线图从写作界面中逐步收起或按需精简。

## 许可证与致谢

本项目基于 Apache License 2.0 许可的 Winc 源码进行二次开发，并完整保留原始 [LICENSE](LICENSE) 与 [NOTICE](NOTICE) 声明。
