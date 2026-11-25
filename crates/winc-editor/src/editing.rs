//! 编辑：编辑命令、多光标与列选择、折叠与块识别、文件内查找、粘贴。
//!
//! `command` 把按键映射成 [`Command`] 并在 [`Document`] 上执行；`cursors` 负责多光标与列选择；
//! `folding` 与 `scopes` 提供折叠状态、显示行换算和块识别；`search` 是文件内查找与替换；
//! `paste` 处理粘贴文件与图片；`modal` 是模态编辑层所需的接口。

use std::collections::VecDeque;

use winc_core::Lines;

use crate::document::content::Content;

pub(crate) mod command;
pub(crate) mod cursors;
pub(crate) mod folding;
pub(crate) mod long_lines;
pub(crate) mod modal;
pub mod paste;
pub mod scopes;
pub(crate) mod search;

/// 模态编辑层的标记与修改列表：位置随文本的修改移动。
#[derive(Debug, Default)]
pub(crate) struct Marks {
    /// 各标记的名字与位置。
    pub(crate) marks: Vec<(char, usize)>,
    /// 修改列表中的位置，从旧到新。
    pub(crate) changes: Vec<usize>,
    /// 逐行执行命令期间被跟踪的位置，从小到大。
    pub(crate) tracked: VecDeque<usize>,
}

impl Marks {
    /// 是否没有任何标记与修改记录。
    pub(crate) fn is_empty(&self) -> bool {
        self.marks.is_empty() && self.changes.is_empty() && self.tracked.is_empty()
    }

    /// 文本 `text` 即将被修改：从 `start` 起的 `removed` 个字符换成 `inserted` 个字符。
    ///
    /// 所在的行被整行删除的标记一并删除，其余位置随修改移动。
    pub(crate) fn edit(&mut self, text: &Content, start: usize, removed: usize, inserted: usize) {
        let end = start + removed;
        let moved = |pos: usize| {
            if pos < start {
                pos
            } else if pos < end {
                start
            } else {
                pos - removed + inserted
            }
        };
        // 位置所在的行是否被整行删除。
        let deleted = |pos: usize| {
            if pos < start || pos >= end {
                return false;
            }
            let line = text.line_of(pos);
            let (from, to) = (text.line_start(line), text.line_end(line));
            start <= from && to <= end && (start < from || to < end)
        };
        self.marks.retain(|(_, pos)| !deleted(*pos));
        self.tracked.retain(|pos| !deleted(*pos));
        for (_, pos) in &mut self.marks {
            *pos = moved(*pos);
        }
        for pos in &mut self.changes {
            *pos = moved(*pos);
        }
        for pos in &mut self.tracked {
            *pos = moved(*pos);
        }
    }
}
