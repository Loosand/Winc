//! 可编辑 Markdown 的原文结构：统一字符/字节/解析偏移换算，定位当前块与隐藏标记，并校验段落范围。

use std::sync::Arc;

use ropey::Rope;
use winc_core::{Lines, Selection};
use winc_editor::Document;
use winc_preview::markdown::{self, LeafRange, Markdown};

use super::Fenced;
use super::laid::LiveLeaf;
use crate::app::App;

/// 可编辑 Markdown 的原文坐标桥与结构查询视图（不依赖 `App`）。
///
/// 统一处理字符偏移 ↔ UTF-8 字节偏移 ↔ 上一版解析范围的位移对齐，以及叶子块定位、
/// 正文判定、隐藏块前缀长度与围栏边界查询，便于纯单元测试独立覆盖。
pub(super) struct LiveSource<'a> {
    rope: &'a Rope,
    parsed: &'a Markdown,
    revision: u64,
    checked: Option<u64>,
}

impl<'a> LiveSource<'a> {
    /// 基于当前原文 `rope`、最新解析结果 `parsed` 及对应版本号构造坐标桥。
    pub(super) fn new(
        rope: &'a Rope,
        parsed: &'a Markdown,
        revision: u64,
        checked: Option<u64>,
    ) -> Self {
        Self {
            rope,
            parsed,
            revision,
            checked,
        }
    }

    /// 解析结果是否与当前文档版本一致。
    pub(super) fn is_fresh(&self) -> bool {
        self.checked == Some(self.revision)
    }

    /// 将字符偏移安全转换为 UTF-8 字节偏移（超出文档长度时钳制到末尾）。
    pub(super) fn char_to_byte(&self, char_idx: usize) -> usize {
        self.rope.char_to_byte(char_idx.min(self.rope.len_chars()))
    }

    /// 将 UTF-8 字节偏移安全转换为字符偏移（超出文档长度时钳制到末尾）。
    pub(super) fn byte_to_char(&self, byte_idx: usize) -> usize {
        self.rope.byte_to_char(byte_idx.min(self.rope.len_bytes()))
    }

    /// 将任意字节偏移钳制并对齐到合法 UTF-8 字符边界。
    pub(super) fn snap_byte(&self, byte_idx: usize) -> usize {
        self.char_to_byte(self.byte_to_char(byte_idx))
    }

    /// 当输入领先于后台解析时，将上一版解析结果中的字节位置按当前文档长度差平移。
    pub(super) fn shift_parsed(&self, parsed_byte: usize) -> usize {
        let len = self.rope.len_bytes();
        let delta = len as i64 - self.parsed.len as i64;
        usize::try_from(parsed_byte as i64 + delta)
            .unwrap_or(0)
            .min(len)
    }

    /// 查找原文字节偏移 `caret` 所在的叶子块，输入领先于解析时按当前原文对齐范围。
    pub(super) fn leaf_at(&self, caret: usize) -> Option<LiveLeaf> {
        let len = self.rope.len_bytes();
        let caret = self.snap_byte(caret);
        let i = self
            .parsed
            .leaves
            .partition_point(|l| l.range.start <= caret);
        let line = self.rope.byte_to_line(caret);
        let line_start = self.rope.line_to_byte(line);
        let line_end = self
            .rope
            .try_line_to_byte(line + 1)
            .map_or(len, |b| b.saturating_sub(1).max(caret))
            .min(len);
        let block_of = |at: usize| {
            self.parsed
                .ranges
                .iter()
                .rposition(|r| r.start <= at && at < r.end.max(r.start + 1))
        };
        let prev = i.checked_sub(1).map(|k| &self.parsed.leaves[k]);
        let next = self.parsed.leaves.get(i);
        let (found, leaf, gap) = match (prev, next) {
            (Some(leaf), _) if caret <= self.shift_parsed(leaf.range.end).max(leaf.range.start) => {
                (
                    leaf,
                    leaf.range.start..self.shift_parsed(leaf.range.end),
                    false,
                )
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
                    line_start.min(start)..self.shift_parsed(leaf.range.end).max(start),
                    false,
                )
            }
            // 光标在叶子块之后的空行上：范围延伸到光标所在行，使光标在原文的空行上有位置。
            (Some(leaf), _) => (leaf, leaf.range.start..line_end, true),
            (None, Some(leaf)) => {
                let start = leaf.range.start;
                (
                    leaf,
                    start..self.shift_parsed(leaf.range.end).max(start),
                    false,
                )
            }
            (None, None) => return None,
        };
        // 块之间的空行视为正文：回车另起一段。
        let prose = gap || found.prose;
        let table = !gap && found.table;
        let before = self.rope.byte_slice(line_start..caret.max(line_start));
        let blank = line_start > found.range.start
            && found.prose
            && before.chars().all(char::is_whitespace);
        let block = block_of(found.range.start)?;
        let range = self.parsed.ranges[block].start
            ..self
                .shift_parsed(self.parsed.ranges[block].end)
                .max(leaf.end);
        // 输入之后、重新解析之前，范围来自上一版文档：起点可能已超出文档，也可能不在字符边界上；
        // 先将范围限制在当前文档内，之后才能按范围截取原文。
        let leaf =
            self.snap_byte(leaf.start)..self.snap_byte(leaf.end).max(self.snap_byte(leaf.start));
        let range =
            self.snap_byte(range.start)..self.snap_byte(range.end).max(self.snap_byte(range.start));
        let last = self.rope.byte_to_line(leaf.end.min(len));
        let edge =
            (!prose && !table && (line <= self.rope.byte_to_line(leaf.start) || line + 1 >= last))
                .then_some(line);
        Some(LiveLeaf {
            block,
            range,
            leaf,
            parsed_end: found.range.end,
            revision: self.revision,
            prose,
            table,
            blank,
            at: caret,
            edge,
        })
    }

    /// 原文第 `at` 个字节所在的叶子块是否为正文（段落、标题）；解析结果不是当前版本时为 `None`。
    pub(super) fn prose_at(&self, at: usize) -> Option<bool> {
        if !self.is_fresh() {
            return None;
        }
        Some(
            leaf_at_or_before(self.parsed, at)
                .is_some_and(|leaf| leaf.prose && at <= leaf.range.end),
        )
    }

    /// 返回第 `line` 行开头在预览中不显示的块标记长度（字符）；解析结果落后时返回 0。
    pub(super) fn hidden_prefix_chars(&self, line: usize) -> usize {
        if !self.is_fresh() || line >= self.rope.len_lines() {
            return 0;
        }
        let start = self.rope.line_to_byte(line);
        let text = self.rope.line(line).to_string();
        let text = text.trim_end_matches(['\n', '\r']);
        let Some(leaf) = leaf_at_or_before(self.parsed, start + text.len()) else {
            return 0;
        };
        let hidden = if leaf.range.start >= start {
            leaf.range.start - start
        } else if leaf.prose && start <= leaf.range.end {
            markdown::enter::indent(text)
        } else {
            0
        };
        self.byte_to_char(start + hidden) - self.rope.line_to_char(line)
    }

    /// 返回 `caret` 所在的围栏代码块或 `$$` 公式各行的位置；不在围栏块中时为 `None`。
    pub(super) fn fenced_at(&self, caret: usize) -> Option<Fenced> {
        let live = self.leaf_at(caret).filter(|l| !l.prose && !l.table)?;
        fenced_lines(self.rope, live.leaf)
    }

    /// 在解析结果与原文保持一致的范围内，返回 `source` 所在段落（或回退到单行）的选区。
    pub(super) fn paragraph_selection(
        &self,
        source: usize,
        unchanged_lines: impl FnOnce(u64) -> usize,
        line_span: impl FnOnce(usize) -> std::ops::Range<usize>,
    ) -> Option<Selection> {
        let revision = self.checked?;
        let leaf = leaf_at_or_before(self.parsed, source).filter(|leaf| source <= leaf.range.end);
        if revision != self.revision {
            // 相邻行的修改也可能改变段落结构，例如加入标题下划线。
            let unchanged = unchanged_lines(revision);
            let limit = self.rope.line_to_byte(unchanged.saturating_sub(1));
            if source >= limit || leaf.is_some_and(|leaf| leaf.range.end > limit) {
                return None;
            }
        }

        let Some(leaf) = leaf.filter(|leaf| leaf.prose) else {
            let line = self.rope.byte_to_line(source);
            let span = line_span(line);
            let start = (span.start + self.hidden_prefix_chars(line)).min(span.end);
            return Some(Selection::new(start, span.end));
        };
        let mut end = leaf.range.end;
        // 保留段内换行，段尾的分隔换行留在选区外。
        while end > leaf.range.start && matches!(self.rope.get_byte(end - 1), Some(b'\r' | b'\n')) {
            end -= 1;
        }
        Some(Selection::new(
            self.byte_to_char(leaf.range.start),
            self.byte_to_char(end),
        ))
    }
}

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
    /// 构造当前活动文档的可编辑 Markdown 原文坐标桥。
    fn live_source(&self) -> Option<LiveSource<'_>> {
        let doc = self.editor.active()?;
        let rope = doc.text().rope()?;
        let parsed = self.preview.markdown.parsed.as_ref()?;
        let checked = self
            .preview
            .markdown
            .checked
            .filter(|_| self.preview.markdown.doc == Some(doc.id));
        Some(LiveSource::new(rope, parsed, doc.revision, checked))
    }

    /// 可编辑的预览中文字光标的原文位置（字节偏移，按当前文字计算）。
    pub(crate) fn live_caret_source(&self) -> Option<usize> {
        let doc = self.editor.active()?;
        let rope = doc.text().rope()?;
        Some(rope.char_to_byte(doc.selection.head.min(rope.len_chars())))
    }

    /// 返回光标所在的围栏代码块或 `$$` 公式各行的位置；光标不在这类块中时为 `None`。
    pub(super) fn live_fenced(&self) -> Option<Fenced> {
        self.live_source()?.fenced_at(self.live_caret_source()?)
    }

    /// 返回可编辑的预览中光标所在的叶子块，见 [`LiveLeaf`]；尚未解析或没有叶子块时为 `None`。
    pub(crate) fn live_leaf(&self) -> Option<LiveLeaf> {
        self.live_leaf_at(self.live_caret_source()?)
    }

    pub(super) fn live_leaf_at(&self, caret: usize) -> Option<LiveLeaf> {
        self.live_source()?.leaf_at(caret)
    }

    /// 原文第 `at` 个字节所在的叶子块是否为正文（段落、标题）；不在任何叶子块中时为
    /// `Some(false)`，解析结果不是当前版本时为 `None`。
    pub(super) fn live_prose_at(&self, at: usize) -> Option<bool> {
        self.live_source()?.prose_at(at)
    }

    /// 返回第 `line` 行开头在预览中不显示的块标记的长度（字符）。
    ///
    /// 叶子块的第一行取叶子块之前的部分（引用的 `>`、列表记号、任务框），正文的续行取引用
    /// 标记和缩进。解析结果不是当前版本时为 0。
    pub(super) fn live_hidden_prefix(&self, line: usize) -> usize {
        self.live_source()
            .map_or(0, |src| src.hidden_prefix_chars(line))
    }

    /// 解析落后时，只使用仍与已绘制内容及当前原文一致的段落范围。
    pub(super) fn paragraph_selection(
        &self,
        doc: &Document,
        rope: &Rope,
        source: usize,
    ) -> Option<Selection> {
        let preview = &self.preview.markdown;
        let checked = preview.checked.filter(|_| preview.doc == Some(doc.id))?;
        let parsed = preview.parsed.as_ref()?;
        // 命中位置来自上一帧，新解析可能还没画出来。
        if preview.layout.as_ref()?.source != Arc::as_ptr(parsed) as usize {
            return None;
        }
        let bridge = LiveSource::new(rope, parsed, doc.revision, Some(checked));
        bridge.paragraph_selection(
            source,
            |rev| doc.unchanged_lines_since(rev),
            |line| doc.text().line_span(line),
        )
    }
}

pub(super) fn leaf_at_or_before(parsed: &Markdown, source: usize) -> Option<&LeafRange> {
    let end = parsed
        .leaves
        .partition_point(|leaf| leaf.range.start <= source);
    end.checked_sub(1).map(|index| &parsed.leaves[index])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinate_bridge_handles_multibyte_utf8_and_clamping() {
        let text = "你好🚀world\n第二行";
        let rope = Rope::from_str(text);
        let parsed = markdown::parse(text);
        let src = LiveSource::new(&rope, &parsed, 1, Some(1));

        // 「你」占 3 字节，「好」占 3 字节，「🚀」占 4 字节。
        assert_eq!(src.char_to_byte(0), 0);
        assert_eq!(src.char_to_byte(1), 3);
        assert_eq!(src.char_to_byte(2), 6);
        assert_eq!(src.char_to_byte(3), 10);
        assert_eq!(src.char_to_byte(999), text.len());

        // 落在多字节字符中间的字节偏移会自动对齐到合法字符边界，不会 panic。
        assert_eq!(src.snap_byte(1), 0);
        assert_eq!(src.snap_byte(7), 6);
        assert_eq!(src.snap_byte(999), text.len());
        assert_eq!(src.byte_to_char(10), 3);
    }

    #[test]
    fn leaf_at_shifts_ranges_when_typing_ahead_of_parse() {
        let old = "第一段文字\n\n第二段";
        let parsed = markdown::parse(old);
        // 模拟用户在后台解析回来前在第一段末尾继续输入了 2 个汉字（6 字节）。
        let typed = "第一段文字追加\n\n第二段";
        let rope = Rope::from_str(typed);
        let src = LiveSource::new(&rope, &parsed, 2, Some(1));

        assert!(!src.is_fresh());
        assert_eq!(src.prose_at(0), None);
        assert_eq!(src.hidden_prefix_chars(0), 0);

        let caret = "第一段文字追加".len();
        let leaf = src.leaf_at(caret).expect("应定位到正在输入的第一段");
        assert!(leaf.prose);
        assert!(!leaf.table);
        assert_eq!(leaf.revision, 2);
        assert!(leaf.leaf.end >= caret);
        assert!(typed.is_char_boundary(leaf.leaf.start));
        assert!(typed.is_char_boundary(leaf.leaf.end));
    }

    #[test]
    fn hidden_prefix_and_prose_queries_cover_lists_quotes_and_fences() {
        let text = "> 引用第一行\n> 续行\n\n- [x] 任务项\n\n```rust\nlet x = 1;\nlet y = 2;\n```\n\n$$\na + b\n$$\n";
        let rope = Rope::from_str(text);
        let parsed = markdown::parse(text);
        let src = LiveSource::new(&rope, &parsed, 5, Some(5));

        assert!(src.is_fresh());
        // 引用前缀 "> " 为 2 个字符。
        assert_eq!(src.hidden_prefix_chars(0), 2);
        assert_eq!(src.hidden_prefix_chars(1), 2);
        // 任务列表项 "- [x] " 为 6 个字符。
        assert_eq!(src.hidden_prefix_chars(3), 6);

        // 光标落在引用 `>` 符号上时，叶子块应从行首开始展开原文。
        let on_quote = src.leaf_at(0).expect("引用前缀应命中引用叶子块");
        assert_eq!(on_quote.leaf.start, 0);
        assert!(on_quote.prose);

        // 代码块内的叶子块不是正文，且能查到完整的围栏范围与边缘行。
        let code_at = text.find("let x = 1;").unwrap();
        assert_eq!(src.prose_at(code_at), Some(false));
        let fence = src.fenced_at(code_at).expect("应识别围栏代码块");
        assert!(fence.closed);
        assert!(!fence.bare);
        assert!(!fence.empty);
        assert_eq!(fence.first, 5);
        assert_eq!(fence.last, 8);
        assert_eq!(fence.content, 6..8);

        // 落在开围栏行时标记为边缘行，落在中间内容行时不为边缘行。
        let fence_open = text.find("```rust").unwrap();
        assert_eq!(src.leaf_at(fence_open).unwrap().edge, Some(5));
        assert_eq!(src.leaf_at(code_at).unwrap().edge, None);

        // 多行公式块也能识别围栏。
        let math_at = text.find("a + b").unwrap();
        let math_fence = src.fenced_at(math_at).expect("应识别多行公式块");
        assert!(math_fence.closed);
        assert!(math_fence.bare);
        assert_eq!(math_fence.content, 11..12);
    }

    #[test]
    fn single_line_display_math_has_no_separate_fence_lines() {
        let text = "$$x^2 + y^2 = z^2$$\n";
        let rope = Rope::from_str(text);
        let parsed = markdown::parse(text);
        let src = LiveSource::new(&rope, &parsed, 1, Some(1));
        assert!(src.fenced_at(4).is_none());
    }
}
