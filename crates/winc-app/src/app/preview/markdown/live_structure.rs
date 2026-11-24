//! 可编辑 Markdown 的原文结构：定位当前块和隐藏标记，并校验段落范围的版本。

use std::sync::Arc;

use ropey::Rope;
use winc_core::{Lines, Selection};
use winc_editor::Document;
use winc_preview::markdown::{self, LeafRange, Markdown};

use crate::app::App;

/// 可编辑的预览中光标所在的叶子块（见 [`Markdown::leaves`](winc_preview::markdown::Markdown::leaves)），该块显示原文。
///
/// 原文位置均按当前文字计算：解析之后输入的文字都位于该块内（光标在该块中），块的末尾
/// 按文字长度的变化移动。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveLeaf {
    /// 所在的顶层块（`Markdown::blocks` 的序号）。
    pub block: usize,
    /// 所在顶层块当前的范围（字节偏移）。
    pub range: std::ops::Range<usize>,
    /// 叶子块当前的范围（字节偏移）；光标位于其后的空行上时延伸到光标所在行，位于其前的
    /// 块标记上时从光标所在行起算。
    pub leaf: std::ops::Range<usize>,
    /// 叶子块在解析结果中的终点（字节偏移）：输入之后、重新解析之前与 `leaf` 的终点不同。
    pub parsed_end: usize,
    /// 文档版本号：输入文字后该块需要重排。
    pub revision: u64,
    /// 是否为正文（段落、标题），见 `LeafRange::prose`。
    pub prose: bool,
    /// 是否为表格；表格按单元格编辑。
    pub table: bool,
    /// 光标是否位于段落、标题之后的空行上。
    pub blank: bool,
    /// 查找该块所用的位置（字节偏移）。
    ///
    /// 通常是光标位置；按住鼠标期间是按下之前的光标位置。表格中该位置所在的单元格显示原文，
    /// 正文中显示该位置接触到的标记。
    pub at: usize,
    pub selection: Option<std::ops::Range<usize>>,
    /// 光标位于代码块、公式等叶子块的第一行或最后两行时，光标所在的行号。
    ///
    /// 围栏行只在光标位于其上时显示，因此光标进出这些行时需要重排；在中间各行移动时不重排。
    pub edge: Option<usize>,
}

impl App {
    /// 返回可编辑的预览中光标所在的叶子块，见 [`LiveLeaf`]；尚未解析或没有叶子块时为 `None`。
    ///
    /// 光标在第一个叶子块之前时取第一个。拖选期间取鼠标按下前的光标所在的块：
    /// 拖到其他块上时该块不切换回排版，鼠标下方的内容不移位。
    pub(crate) fn live_leaf(&self) -> Option<LiveLeaf> {
        let doc = self.editor.active()?;
        let parsed = self.preview.markdown.parsed.as_ref()?;
        let rope = doc.text().rope()?;
        let len = rope.len_bytes();
        let mut caret = self.live_layout_caret()?;
        let selection = self.live_layout_selection().map(|selection| {
            rope.char_to_byte(selection.from())..rope.char_to_byte(selection.to())
        });
        if let Some(selection) = &selection
            && caret == selection.start
            && leaf_at_or_before(parsed, selection.end).is_some_and(|leaf| {
                leaf.prose
                    && selection.start < leaf.range.start
                    && selection.end <= leaf.range.end
                    && rope.byte_to_line(selection.start)
                        == rope.byte_to_line(leaf.range.start.min(len))
            })
        {
            // 反向选中列表项时，起点可能在列表标记上，使用另一端定位正文。
            caret = selection.end;
        }
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
            selection,
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

fn leaf_at_or_before(parsed: &Markdown, source: usize) -> Option<&LeafRange> {
    let end = parsed
        .leaves
        .partition_point(|leaf| leaf.range.start <= source);
    end.checked_sub(1).map(|index| &parsed.leaves[index])
}
