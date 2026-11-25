<p align="center">
  <img src="./crates/winc-app/assets/icon.svg" alt="Minc" width="128" />
</p>

<h1 align="center">Minc</h1>

<p align="center">
  A fast, local-first Markdown writing app built for focused writing and Agent collaboration.
</p>

<p align="center">
  English | <a href="./README.zh-CN.md">简体中文</a>
</p>

<p align="center">
  <a href="./LICENSE"><img alt="License" src="https://img.shields.io/badge/License-Apache%202.0-blue" /></a>
  <img alt="Rust" src="https://img.shields.io/badge/Rust-2024-b7410e" />
  <img alt="Platforms" src="https://img.shields.io/badge/Platforms-macOS%20%7C%20Linux%20%7C%20Windows-555" />
</p>

## Overview

Minc is built with Rust and a custom CPU-rasterized rendering engine—no Electron and no WebView. It combines native-grade startup and typing responsiveness with a Typora-style, single-view formatted Markdown writing experience.

Every note stays a plain `.md` file inside your local folder, with no proprietary database or cloud lock-in.

| Measure | Result |
| --- | --- |
| Cold start to first frame | ~119 ms (median of 20 runs on macOS) |
| Resident memory with 100,000 lines open | ~52 MB |
| First-frame typing latency (105 KiB mixed Markdown) | ~12 ms (p50) |

Measured on macOS; actual numbers vary by hardware and system load.

## Core Capabilities

| Area | Capabilities |
| --- | --- |
| Live-Formatted Writing | Single editable formatted view (WYSIWYG) with math, Mermaid diagrams, reliable CJK IME input, and fidelity-preserving HTML table & rich-text paste |
| Local Note Library | Open any local folder as a workspace; includes a paper-card Home view, file tree, and `⌘P` quick search across titles and content |
| Note Properties | Collapsible frontmatter property panel supporting text, number, date, checkbox, single/multi-select, stored cleanly as YAML |
| Links & Backlinks | Supports both standard Markdown links and `[[WikiLinks]]`, with a backlinks panel and jump-to-context navigation |
| Reference Previews | Built-in local previews for images, SVG, PDF (macOS / Windows), Word (`docx`), spreadsheets (`xlsx` / `csv`), and JSON graphs |
| Interface | 16 built-in light and dark themes, English and Simplified Chinese UI, trackpad zoom, and touch gestures |

## Direction: Evergreen Writing Core + Evolving Agent Layer

Minc separates slow-changing writing fundamentals from fast-changing LLM/Agent capabilities across a thin process boundary:

- **Evergreen Core (Local Writing Host)**: Preserves startup speed, typing feel, predictable undo, and plain `.md` file ownership, while providing four stable collaboration primitives—inline annotations, selection Diff with single-step undo, a collapsible side stream, and safe workspace file sync.
- **Evolving Layer (External Agent Runtime)**: Connects on demand via an isolated RPC sidecar for proofreading, translation, refinement, web research saved locally, fact-checking, diagram/image generation, and Skills workflows. When AI is not invoked, no sidecar process is started and editor performance remains unaffected.

## Platforms

| Platform | Requirements |
| --- | --- |
| macOS | macOS 11 or later (Apple silicon / Intel) |
| Linux | x86_64 (X11 / Wayland) |
| Windows | Windows 10 1809 or later (x86_64) |

Primary development and desktop verification currently target macOS.

## Building from Source

The Rust toolchain is pinned in `rust-toolchain.toml`:

```bash
cargo run --release                   # build and run
cargo run --release -- <notes-dir>    # open a folder or Markdown file
```

## Development & Verification

On macOS, run the local dev launcher for isolated desktop testing (keeps dev settings and sessions separate from personal notes):

```bash
python3 scripts/dev.py                # watch, rebuild, and relaunch the dev app
python3 scripts/dev.py --once         # build and launch once
```

Tests and benchmarks:

```bash
cargo xtask test                      # test crates changed since HEAD
cargo xtask test --all                # test the entire workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo xtask bench                     # run startup, memory, and scroll benchmarks
```

## Workspace Layout

The UI is written in Rust using `winit` for windowing and a custom CPU rasterizer (presenting directly to `IOSurface` on macOS):

| Crate | Responsibility |
| --- | --- |
| `winc-core` | Text buffer (Rope), editing commands, undo stack, and word-level diffing |
| `winc-editor` | Document state, file tree, and workspace search |
| `winc-preview` | Markdown parsing, note-link indexing, and previews for images, SVG, PDF, docx, spreadsheets, and JSON |
| `winc-typeset` | Layout engine for live-formatted Markdown |
| `winc-render` | Glyph handling, rasterization, and presentation |
| `winc-watch` | Local workspace file watching |
| `winc-config` | Configuration, themes, and session persistence |
| `winc-i18n` | Interface localization (Fluent bundles) |
| `winc-app` | Desktop application: windows, events, and views |
| `winc` | Binary entry point |
| `xtask` | Testing, benchmarking, and build automation |

> Note: Several lower-level crates inherited from the upstream code editor (`winc-syntax`, `winc-vim`, `winc-pty`, `winc-backend`, `winc-server`) remain in the workspace and are gradually being hidden from the writing UI or trimmed as the roadmap progresses.

## License & Attribution

This project is a derivative work based on Winc, licensed under the [Apache License, Version 2.0](LICENSE). Original license and attribution notices are preserved in [LICENSE](LICENSE) and [NOTICE](NOTICE).
