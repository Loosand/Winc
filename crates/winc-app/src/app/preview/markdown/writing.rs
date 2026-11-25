//! Markdown 的写作命令适配器：将块级格式与插入命令转发至 `winc_richtext::commands`。

use std::ops::Range;

use winc_preview::markdown::paragraph::Kind;
use winc_richtext::commands;
pub(super) use winc_richtext::line_text;

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

    /// 把选中的各行（没有选区时是光标所在的一行）改成 `kind`。
    pub(in crate::app) fn markdown_paragraph(&mut self, kind: Kind) {
        if self.in_raw_block() {
            return;
        }
        let Some(doc) = self.editor.active() else {
            return;
        };
        if !doc.extra_selections().is_empty() {
            return;
        }
        let Some(rope) = doc.text().rope() else {
            return;
        };
        let crlf = doc.line_ending == winc_core::LineEnding::CrLf;
        let plan = commands::plan_paragraph_kind(rope, doc.selection, kind, crlf);
        self.apply_writing_plan(plan);
    }

    /// 把选中的各行放进 `fence` 围起的块里（代码块、公式块）。
    pub(in crate::app) fn markdown_fenced(&mut self, fence: &str) {
        if self.in_raw_block() {
            return;
        }
        let Some(doc) = self.editor.active() else {
            return;
        };
        if !doc.extra_selections().is_empty() {
            return;
        }
        let Some(rope) = doc.text().rope() else {
            return;
        };
        let crlf = doc.line_ending == winc_core::LineEnding::CrLf;
        let plan = commands::plan_insert_fenced(rope, doc.selection, fence, crlf);
        self.apply_writing_plan(plan);
    }

    /// 插入两列的表格：光标在空行上时换掉这一行，否则插在这一行下面。
    pub(in crate::app) fn markdown_table(&mut self) {
        if self.in_raw_block() {
            return;
        }
        let Some(doc) = self.editor.active() else {
            return;
        };
        if !doc.extra_selections().is_empty() {
            return;
        }
        let Some(rope) = doc.text().rope() else {
            return;
        };
        let crlf = doc.line_ending == winc_core::LineEnding::CrLf;
        let plan = commands::plan_insert_table(rope, doc.selection, crlf);
        self.apply_writing_plan(plan);
    }

    /// 把选中的文字（一行之内）做成链接或图片（`image`）；剪贴板里是网址时作为地址。
    pub(in crate::app) fn markdown_link(&mut self, image: bool) {
        if self.in_raw_block() {
            return;
        }
        let url = self.clipboard_text();
        let Some(doc) = self.editor.active() else {
            return;
        };
        if !doc.extra_selections().is_empty() {
            return;
        }
        let Some(rope) = doc.text().rope() else {
            return;
        };
        let crlf = doc.line_ending == winc_core::LineEnding::CrLf;
        let Some(plan) =
            commands::plan_insert_link(rope, doc.selection, url.as_deref(), image, crlf)
        else {
            return;
        };
        self.apply_writing_plan(plan);
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
        let Some(doc) = self.editor.active() else {
            return;
        };
        let Some(rope) = doc.text().rope() else {
            return;
        };
        let crlf = doc.line_ending == winc_core::LineEnding::CrLf;
        let plan = commands::plan_replace_lines(rope, first, last, text.to_owned(), select, crlf);
        self.apply_writing_plan(plan);
    }

    /// 在第 `line` 行下面隔一个空行插入块 `block`，光标放在块中的 `at`（字节偏移）。
    pub(super) fn insert_below(&mut self, line: usize, block: &str, at: usize) {
        let Some(doc) = self.editor.active() else {
            return;
        };
        let Some(rope) = doc.text().rope() else {
            return;
        };
        let crlf = doc.line_ending == winc_core::LineEnding::CrLf;
        let plan = commands::plan_insert_below(rope, line, block, at, crlf);
        self.apply_writing_plan(plan);
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
