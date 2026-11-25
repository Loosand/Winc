//! 可编辑 Markdown 的原文结构：定位当前块和隐藏标记，并校验段落范围的版本。

use std::sync::Arc;

use ropey::Rope;
use winc_core::{Lines, Selection};
use winc_editor::Document;
use winc_preview::markdown::{self, LeafRange, Markdown};

use super::Fenced;
use super::laid::LiveLeaf;
use crate::app::App;

/// 查找已解析围栏代码块或公式块的边界；较短或后跟代码的围栏行视为内容。
pub(super) fn fenced_lines(rope: &ropey::Rope, range: std::ops::Range<usize>) -> Option<Fenced> {
    let len = rope.len_bytes();
    let start = range.start.min(len);
    let first = rope.byte_to_line(start);
    let opening = rope.byte_slice(start..rope.line_to_byte(first + 1));
    let opening = std::borrow::Cow::<str>::from(opening);
    let open = opening.trim();
    let mark = open.bytes().next()?;
    let min = match mark {
        b'`' | b'~' => 3,
        b'$' => 2,
        _ => return None,
    };
    let count = open.bytes().take_while(|&b| b == mark).count();
    if count < min {
        return None;
    }
    let quotes = rope
        .byte_slice(rope.line_to_byte(first)..start)
        .chars()
        .filter(|&c| c == '>')
        .count();
    let text = |line| {
        let raw = std::borrow::Cow::<str>::from(rope.line(line));
        let mut body = raw.as_ref();
        for _ in 0..quotes {
            let Some(rest) = body.trim_start_matches([' ', '\t']).strip_prefix('>') else {
                break;
            };
            body = rest;
        }
        body.trim_matches([' ', '\t', '\r', '\n']).to_owned()
    };
    let end = range.end.min(len).saturating_sub(1).max(start);
    let mut last = rope.byte_to_line(end);
    while last > first && text(last).is_empty() {
        last -= 1;
    }
    // Single-line display math has no separate closing line to navigate around.
    if first == last && mark == b'$' && open[count..].ends_with("$$") {
        return None;
    }
    let close = text(last);
    let closed = last > first && close.len() >= count && close.bytes().all(|b| b == mark);
    let content = first + 1..if closed { last } else { last + 1 };
    Some(Fenced {
        first,
        last,
        closed,
        bare: count == open.len(),
        empty: content.clone().all(|line| text(line).is_empty()),
        content,
    })
}

impl App {
    /// 可编辑的预览中文字光标的原文位置（字节偏移，按当前文字计算）。
    pub(crate) fn live_caret_source(&self) -> Option<usize> {
        let doc = self.editor.active()?;
        let rope = doc.text().rope()?;
        Some(rope.char_to_byte(doc.selection.head.min(rope.len_chars())))
    }

    /// 返回光标所在的围栏代码块或 `$$` 公式各行的位置；光标不在这类块中时为 `None`。
    pub(super) fn live_fenced(&self) -> Option<Fenced> {
        let live = self.live_leaf().filter(|l| !l.prose && !l.table)?;
        let rope = self.editor.active()?.text().rope()?;
        fenced_lines(rope, live.leaf)
    }

    /// 返回可编辑的预览中光标所在的叶子块，见 [`LiveLeaf`]；尚未解析或没有叶子块时为 `None`。
    ///
    pub(crate) fn live_leaf(&self) -> Option<LiveLeaf> {
        self.live_leaf_at(self.live_caret_source()?)
    }

    pub(super) fn live_leaf_at(&self, caret: usize) -> Option<LiveLeaf> {
        let doc = self.editor.active()?;
        let parsed = self.preview.markdown.parsed.as_ref()?;
        let rope = doc.text().rope()?;
        let len = rope.len_bytes();
        let delta = len as i64 - parsed.len as i64;
        let shift = |at: usize| usize::try_from(at as i64 + delta).unwrap_or(0).min(len);
        let i = parsed.leaves.partition_point(|l| l.range.start <= caret);
        let line = rope.byte_to_line(caret);
        let line_start = rope.line_to_byte(line);
        let line_end = rope
            .try_line_to_byte(line + 1)
            .map_or(len, |b| b.saturating_sub(1).max(caret))
            .min(len);
        let block_of = |at: usize| {
            parsed
                .ranges
                .iter()
                .rposition(|r| r.start <= at && at < r.end.max(r.start + 1))
        };
        let prev = i.checked_sub(1).map(|k| &parsed.leaves[k]);
        let next = parsed.leaves.get(i);
        let (found, leaf, gap) = match (prev, next) {
            (Some(leaf), _) if caret <= shift(leaf.range.end).max(leaf.range.start) => {
                (leaf, leaf.range.start..shift(leaf.range.end), false)
            }
            // 光标在块的标记上（引用的 `>`、提示块的 `[!NOTE]`、列表的序号），尚未到达其第一个
            // 叶子块：取该叶子块，从光标所在行显示原文。
            (_, Some(leaf))
                if block_of(leaf.range.start)
                    .is_some_and(|block| block_of(caret) == Some(block)) =>
            {
                let start = leaf.range.start;
                (
                    leaf,
                    line_start.min(start)..shift(leaf.range.end).max(start),
                    false,
                )
            }
            // 光标在叶子块之后的空行上：范围延伸到光标所在行，使光标在原文的空行上有位置。
            (Some(leaf), _) => (leaf, leaf.range.start..line_end, true),
            (None, Some(leaf)) => {
                let start = leaf.range.start;
                (leaf, start..shift(leaf.range.end).max(start), false)
            }
            (None, None) => return None,
        };
        // 块之间的空行视为正文：回车另起一段。
        let prose = gap || found.prose;
        let table = !gap && found.table;
        let before = rope.byte_slice(line_start..caret.max(line_start));
        let blank = line_start > found.range.start
            && found.prose
            && before.chars().all(char::is_whitespace);
        let block = block_of(found.range.start)?;
        let range = parsed.ranges[block].start..shift(parsed.ranges[block].end).max(leaf.end);
        // 输入之后、重新解析之前，范围来自上一版文档：起点可能已超出文档，也可能不在字符边界上；
        // 先将范围限制在当前文档内，之后才能按范围截取原文。
        let snap = |at: usize| rope.char_to_byte(rope.byte_to_char(at.min(len)));
        let leaf = snap(leaf.start)..snap(leaf.end).max(snap(leaf.start));
        let range = snap(range.start)..snap(range.end).max(snap(range.start));
        let last = rope.byte_to_line(leaf.end.min(len));
        let edge =
            (!prose && !table && (line <= rope.byte_to_line(leaf.start) || line + 1 >= last))
                .then_some(line);
        Some(LiveLeaf {
            block,
            range,
            leaf,
            parsed_end: found.range.end,
            revision: doc.revision,
            prose,
            table,
            blank,
            at: caret,
            edge,
        })
    }

    /// 原文第 `at` 个字节所在的叶子块是否为正文（段落、标题）；不在任何叶子块中时为
    /// `Some(false)`，解析结果不是当前版本时为 `None`。
    pub(super) fn live_prose_at(&self, at: usize) -> Option<bool> {
        let doc = self.editor.active()?;
        if self.preview.markdown.checked != Some(doc.revision) {
            return None;
        }
        let parsed = self.preview.markdown.parsed.as_ref()?;
        Some(leaf_at_or_before(parsed, at).is_some_and(|leaf| leaf.prose && at <= leaf.range.end))
    }

    /// 返回第 `line` 行开头在预览中不显示的块标记的长度（字符）。
    ///
    /// 叶子块的第一行取叶子块之前的部分（引用的 `>`、列表记号、任务框），正文的续行取引用
    /// 标记和缩进。解析结果不是当前版本时为 0。
    pub(super) fn live_hidden_prefix(&self, line: usize) -> usize {
        let Some(doc) = self.editor.active() else {
            return 0;
        };
        if self.preview.markdown.checked != Some(doc.revision) {
            return 0;
        }
        let (Some(parsed), Some(rope)) = (self.preview.markdown.parsed.as_ref(), doc.text().rope())
        else {
            return 0;
        };
        if line >= rope.len_lines() {
            return 0;
        }
        let start = rope.line_to_byte(line);
        let text = rope.line(line).to_string();
        let text = text.trim_end_matches(['\n', '\r']);
        let Some(leaf) = leaf_at_or_before(parsed, start + text.len()) else {
            return 0;
        };
        let hidden = if leaf.range.start >= start {
            leaf.range.start - start
        } else if leaf.prose && start <= leaf.range.end {
            markdown::enter::indent(text)
        } else {
            0
        };
        rope.byte_to_char((start + hidden).min(rope.len_bytes())) - rope.line_to_char(line)
    }

    /// 解析落后时，只使用仍与已绘制内容及当前原文一致的段落范围。
    pub(super) fn paragraph_selection(
        &self,
        doc: &Document,
        rope: &Rope,
        source: usize,
    ) -> Option<Selection> {
        let preview = &self.preview.markdown;
        let revision = preview.checked.filter(|_| preview.doc == Some(doc.id))?;
        let parsed = preview.parsed.as_ref()?;
        // 命中位置来自上一帧，新解析可能还没画出来。
        if preview.layout.as_ref()?.source != Arc::as_ptr(parsed) as usize {
            return None;
        }

        let leaf = leaf_at_or_before(parsed, source).filter(|leaf| source <= leaf.range.end);
        if revision != doc.revision {
            // 相邻行的修改也可能改变段落结构，例如加入标题下划线。
            let unchanged = doc.unchanged_lines_since(revision);
            let limit = rope.line_to_byte(unchanged.saturating_sub(1));
            if source >= limit || leaf.is_some_and(|leaf| leaf.range.end > limit) {
                return None;
            }
        }

        let Some(leaf) = leaf.filter(|leaf| leaf.prose) else {
            let line = rope.byte_to_line(source);
            let span = doc.text().line_span(line);
            let start = (span.start + self.live_hidden_prefix(line)).min(span.end);
            return Some(Selection::new(start, span.end));
        };
        let mut end = leaf.range.end;
        // 保留段内换行，段尾的分隔换行留在选区外。
        while end > leaf.range.start && matches!(rope.get_byte(end - 1), Some(b'\r' | b'\n')) {
            end -= 1;
        }
        Some(Selection::new(
            rope.byte_to_char(leaf.range.start),
            rope.byte_to_char(end),
        ))
    }
}

pub(super) fn leaf_at_or_before(parsed: &Markdown, source: usize) -> Option<&LeafRange> {
    let end = parsed
        .leaves
        .partition_point(|leaf| leaf.range.start <= source);
    end.checked_sub(1).map(|index| &parsed.leaves[index])
}
