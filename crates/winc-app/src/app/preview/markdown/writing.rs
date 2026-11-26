//! Markdown 的写作命令适配器：通过 `winc_richtext::commands::plan_writing` 统一分发。

use std::ops::Range;

use winc_preview::markdown::paragraph::Kind;
pub(super) use winc_richtext::line_text;
use winc_richtext::{WritingCommand, commands};

use super::*;

impl App {
    /// 可编辑的预览中光标在代码块、公式、表格等按原文显示的块里，写作命令不改动它。
    fn in_raw_block(&mut self) -> bool {
        if !self.markdown_live() {
            return false;
        }
        self.parse_markdown_inline();
        self.live_leaf().is_some_and(|l| !l.prose)
    }

    /// 统一执行由内核规划的写作命令；`skip_raw_block` 为真时在代码块/公式/表格内不生效。
    fn run_writing_command(&mut self, skip_raw_block: bool, command: WritingCommand<'_>) {
        if skip_raw_block && self.in_raw_block() {
            return;
        }
        let Some(doc) = self.editor.active() else {
            return;
        };
        if skip_raw_block && !doc.extra_selections().is_empty() {
            return;
        }
        let Some(rope) = doc.text().rope() else {
            return;
        };
        let crlf = doc.line_ending == winc_core::LineEnding::CrLf;
        let Some(plan) = commands::plan_writing(rope, doc.selection, command, crlf) else {
            return;
        };
        self.apply_writing_plan(plan);
    }

    /// 把选中的各行（没有选区时是光标所在的一行）改成 `kind`。
    pub(in crate::app) fn markdown_paragraph(&mut self, kind: Kind) {
        self.run_writing_command(true, WritingCommand::Paragraph(kind));
    }

    /// 把选中的各行放进 `fence` 围起的块里（代码块、公式块）。
    pub(in crate::app) fn markdown_fenced(&mut self, fence: &str) {
        self.run_writing_command(true, WritingCommand::Fenced(fence));
    }

    /// 插入两列的表格：光标在空行上时换掉这一行，否则插在这一行下面。
    pub(in crate::app) fn markdown_table(&mut self) {
        self.run_writing_command(true, WritingCommand::Table);
    }

    /// 把选中的文字（一行之内）做成链接或图片（`image`）；剪贴板里是网址时作为地址。
    pub(in crate::app) fn markdown_link(&mut self, image: bool) {
        if self.in_raw_block() {
            return;
        }
        let url = self.clipboard_text();
        self.run_writing_command(
            true,
            WritingCommand::Link {
                clipboard_url: url.as_deref(),
                image,
            },
        );
    }

    /// 选中的各行（首行、末行）；只有光标、没有选区时另给出光标在行中的位置（字节偏移）。
    /// 有多个光标时为 `None`。
    pub(super) fn selected_lines(&self) -> Option<(usize, usize, Option<usize>)> {
        let doc = self.editor.active()?;
        let rope = doc.text().rope()?;
        if !doc.extra_selections().is_empty() {
            return None;
        }
        Some(commands::selected_lines(rope, doc.selection))
    }

    /// 把第 `first` 到 `last` 行（不含末行的换行符）换成 `text`，之后选中 `select`（字节偏移）。
    pub(super) fn replace_lines(
        &mut self,
        first: usize,
        last: usize,
        text: &str,
        select: Range<usize>,
    ) {
        self.run_writing_command(
            false,
            WritingCommand::ReplaceLines {
                first,
                last,
                text,
                select,
            },
        );
    }

    /// 在第 `line` 行下面隔一个空行插入块 `block`，光标放在块中的 `at`（字节偏移）。
    pub(super) fn insert_below(&mut self, line: usize, block: &str, at: usize) {
        self.run_writing_command(
            false,
            WritingCommand::InsertBelow {
                line,
                block,
                caret_byte: at,
            },
        );
    }

    fn apply_writing_plan(&mut self, plan: winc_richtext::EditPlan) {
        self.apply_edit_plan(plan, false);
        if self.markdown_live() {
            self.finish_live_edit_reset_line();
            self.reset_blink();
        }
        self.ensure_caret_visible();
        self.request_redraw();
    }
}
