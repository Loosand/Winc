//! 文本与坐标层：位置映射、坐标桥、块结构与围栏边界查询。
//!
//! 职责：
//! - 通过 [`ChangeMap`] 在文本变更后精确映射派生位置（异步解析范围、光标锚点、装饰层区间等）；
//! - 通过 [`SourceBridge`] 统一字符偏移、UTF-8 字节偏移与解析块范围的换算；
//! - 提供叶子块、围栏块、行边界与段落边界的无状态查询。

use std::borrow::Cow;
use std::ops::Range;

use ropey::{Rope, RopeSlice};
use winc_core::{Change, Selection};
use winc_preview::markdown::{self, LeafRange, Markdown};

use crate::layout::LiveLeaf;

/// 位置映射时的边界倾向：当偏移落在插入点或被删除区间内时的落点规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assoc {
    /// 偏向变更起点（插入点左侧 / 删除区间起点）。
    Before,
    /// 偏向变更终点（插入点右侧 / 删除区间替换后的末尾）。
    After,
}

/// 一组已应用的原子编辑（按原文本偏移升序且互不重叠），用于把旧位置精确映射到新文本。
///
/// 借鉴 `CodeMirror` 6 的 `ChangeDesc` 思想，支持按顺序组合多步变更；坐标单位由调用方统一
/// （UTF-8 字节偏移或字符偏移均可，只要构图与查询口径一致）。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ChangeMap {
    /// 各处编辑按原文本起点升序排列：`(old_start, old_len, new_len)`。
    edits: Vec<(usize, usize, usize)>,
}

impl ChangeMap {
    /// 空映射：所有位置保持不变。
    pub fn empty() -> Self {
        Self::default()
    }

    /// 构造单处替换的映射：原文本 `start..start + old_len` 替换为长度为 `new_len` 的文本。
    pub fn single(start: usize, old_len: usize, new_len: usize) -> Self {
        if old_len == 0 && new_len == 0 {
            return Self::empty();
        }
        Self {
            edits: vec![(start, old_len, new_len)],
        }
    }

    /// 从一组顺序应用的 `winc_core::Change`（字符偏移）构造映射。
    ///
    /// 每个 [`Change`] 均基于前一条应用后的文本坐标（与 `Transaction::apply` 一致），
    /// 返回可按顺序映射单个位置或区间的映射表。
    pub fn from_char_changes(changes: &[Change]) -> ChangeSeq {
        let steps = changes
            .iter()
            .filter(|c| !c.removed.is_empty() || !c.inserted.is_empty())
            .map(|c| {
                Self::single(
                    c.start,
                    c.removed.chars().count(),
                    c.inserted.chars().count(),
                )
            })
            .collect();
        ChangeSeq { steps }
    }

    /// 是否不包含任何有效变更。
    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    /// 新旧文本的总长度差（`new_len - old_len`）。
    pub fn len_delta(&self) -> isize {
        self.edits
            .iter()
            .map(|&(_, old_len, new_len)| new_len as isize - old_len as isize)
            .sum()
    }

    /// 将旧文本中的位置 `pos` 映射到新文本。
    pub fn map_pos(&self, pos: usize, assoc: Assoc) -> usize {
        let mut delta: isize = 0;
        for &(start, old_len, new_len) in &self.edits {
            let end = start + old_len;
            if pos < start {
                break;
            }
            if pos > end {
                delta += new_len as isize - old_len as isize;
                continue;
            }
            // `pos` 落在 `start..=end` 内。
            let mapped = if old_len == 0 {
                // 纯插入点 `pos == start`：按倾向决定留在插入前还是跳到插入后。
                match assoc {
                    Assoc::Before => start as isize + delta,
                    Assoc::After => start as isize + delta + new_len as isize,
                }
            } else if pos == start {
                start as isize + delta
            } else if pos == end {
                start as isize + delta + new_len as isize
            } else {
                // 严格落在被替换/删除的区间内部。
                match assoc {
                    Assoc::Before => start as isize + delta,
                    Assoc::After => start as isize + delta + new_len as isize,
                }
            };
            return mapped.max(0) as usize;
        }
        (pos as isize + delta).max(0) as usize
    }

    /// 将旧文本中的左闭右开区间 `range` 映射到新文本。
    ///
    /// 起点使用 [`Assoc::After`]、终点使用 [`Assoc::Before`]，避免在区间外部紧贴边界处输入时
    /// 意外把新文字吸进旧区间（适合装饰层、批注锚点等）。
    pub fn map_range(&self, range: Range<usize>) -> Range<usize> {
        let start = self.map_pos(range.start, Assoc::After);
        let end = self.map_pos(range.end, Assoc::Before);
        start..end.max(start)
    }

    /// 将旧文本中的块范围 `range` 映射到新文本，允许在块边界继续输入时向外扩展。
    pub fn map_block_range(&self, range: Range<usize>) -> Range<usize> {
        let start = self.map_pos(range.start, Assoc::Before);
        let end = self.map_pos(range.end, Assoc::After);
        start..end.max(start)
    }
}

/// 按时间顺序排列的多步 [`ChangeMap`] 序列，用于在多次增量编辑后依次映射旧位置。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ChangeSeq {
    steps: Vec<ChangeMap>,
}

impl ChangeSeq {
    /// 是否为空序列。
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// 追加一步变更映射。
    pub fn push(&mut self, map: ChangeMap) {
        if !map.is_empty() {
            self.steps.push(map);
        }
    }

    /// 清空已累积的变更（通常在新一版解析完成后调用）。
    pub fn clear(&mut self) {
        self.steps.clear();
    }

    /// 依次经过全部步骤映射位置 `pos`。
    pub fn map_pos(&self, mut pos: usize, assoc: Assoc) -> usize {
        for step in &self.steps {
            pos = step.map_pos(pos, assoc);
        }
        pos
    }

    /// 依次经过全部步骤映射区间 `range`（内收缩边界语义）。
    pub fn map_range(&self, mut range: Range<usize>) -> Range<usize> {
        for step in &self.steps {
            range = step.map_range(range);
        }
        range
    }

    /// 依次经过全部步骤映射块区间 `range`（外扩展边界语义）。
    pub fn map_block_range(&self, mut range: Range<usize>) -> Range<usize> {
        for step in &self.steps {
            range = step.map_block_range(range);
        }
        range
    }
}

/// 光标所在的围栏代码块或 `$$` 公式块的边界信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fenced {
    /// 开围栏所在的行号。
    pub first: usize,
    /// 该块最后一行的行号：已闭合时为闭围栏。
    pub last: usize,
    /// 是否有闭围栏。
    pub closed: bool,
    /// 开围栏是否只有围栏字符，不带语言名。
    pub bare: bool,
    /// 内容是否为空（没有行，或各行均为空白）。
    pub empty: bool,
    /// 内容各行的行号范围。
    pub content: Range<usize>,
}

/// 可编辑 Markdown 的原文坐标桥与结构查询视图。
///
/// 统一处理字符偏移 ↔ UTF-8 字节偏移 ↔ 上一版解析范围的位移对齐，以及叶子块定位、
/// 正文判定、隐藏块前缀长度与围栏边界查询。
pub struct SourceBridge<'a> {
    rope: &'a Rope,
    parsed: &'a Markdown,
    revision: u64,
    checked: Option<u64>,
}

impl<'a> SourceBridge<'a> {
    /// 基于当前原文 `rope`、最新解析结果 `parsed` 及对应版本号构造坐标桥。
    pub fn new(rope: &'a Rope, parsed: &'a Markdown, revision: u64, checked: Option<u64>) -> Self {
        Self {
            rope,
            parsed,
            revision,
            checked,
        }
    }

    /// 解析结果是否与当前文档版本一致。
    pub fn is_fresh(&self) -> bool {
        self.checked == Some(self.revision)
    }

    /// 将字符偏移安全转换为 UTF-8 字节偏移（超出文档长度时钳制到末尾）。
    pub fn char_to_byte(&self, char_idx: usize) -> usize {
        self.rope.char_to_byte(char_idx.min(self.rope.len_chars()))
    }

    /// 将 UTF-8 字节偏移安全转换为字符偏移（超出文档长度时钳制到末尾）。
    pub fn byte_to_char(&self, byte_idx: usize) -> usize {
        self.rope.byte_to_char(byte_idx.min(self.rope.len_bytes()))
    }

    /// 将任意字节偏移钳制并对齐到合法 UTF-8 字符边界。
    pub fn snap_byte(&self, byte_idx: usize) -> usize {
        self.char_to_byte(self.byte_to_char(byte_idx))
    }

    /// 当输入领先于后台解析时，将上一版解析结果中的字节位置按当前文档长度差平移。
    pub fn shift_parsed(&self, parsed_byte: usize) -> usize {
        let len = self.rope.len_bytes();
        let delta = len as i64 - self.parsed.len as i64;
        usize::try_from(parsed_byte as i64 + delta)
            .unwrap_or(0)
            .min(len)
    }

    /// 查找原文字节偏移 `caret` 所在的叶子块，输入领先于解析时按当前原文对齐范围。
    pub fn leaf_at(&self, caret: usize) -> Option<LiveLeaf> {
        let len = self.rope.len_bytes();
        let caret = self.snap_byte(caret);
        let delta = len as i64 - self.parsed.len as i64;
        // 输入领先于异步解析时，先将当前光标还原到上一版解析坐标系，再在旧 `leaves` / `ranges`
        // 中查找归属块，避免刚输入的多字节字符（如中文）越过未平移的下一块起点而跳块。
        let parsed_caret = if delta > 0 {
            caret.saturating_sub(delta as usize)
        } else {
            caret
        };
        let i = self
            .parsed
            .leaves
            .partition_point(|l| l.range.start <= parsed_caret);
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
                    .is_some_and(|block| block_of(parsed_caret) == Some(block)) =>
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
    pub fn prose_at(&self, at: usize) -> Option<bool> {
        if !self.is_fresh() {
            return None;
        }
        Some(
            leaf_at_or_before(self.parsed, at)
                .is_some_and(|leaf| leaf.prose && at <= leaf.range.end),
        )
    }

    /// 返回第 `line` 行开头在预览中不显示的块标记长度（字符）；解析结果落后时返回 0。
    pub fn hidden_prefix_chars(&self, line: usize) -> usize {
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
    pub fn fenced_at(&self, caret: usize) -> Option<Fenced> {
        let live = self.leaf_at(caret).filter(|l| !l.prose && !l.table)?;
        fenced_lines(self.rope, live.leaf)
    }

    /// 在解析结果与原文保持一致的范围内，返回 `source` 所在段落（或回退到单行）的选区。
    pub fn paragraph_selection(
        &self,
        source: usize,
        unchanged_lines: impl FnOnce(u64) -> usize,
        line_span: impl FnOnce(usize) -> Range<usize>,
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

/// 返回起点不晚于 `source` 的最后一个叶子块。
pub fn leaf_at_or_before(parsed: &Markdown, source: usize) -> Option<&LeafRange> {
    let end = parsed
        .leaves
        .partition_point(|leaf| leaf.range.start <= source);
    end.checked_sub(1).map(|index| &parsed.leaves[index])
}

/// 查找已解析围栏代码块或公式块的边界；较短或后跟代码的围栏行视为内容。
pub fn fenced_lines(rope: &Rope, range: Range<usize>) -> Option<Fenced> {
    let len = rope.len_bytes();
    let start = range.start.min(len);
    let first = rope.byte_to_line(start);
    let opening = rope.byte_slice(start..rope.line_to_byte(first + 1));
    let opening = Cow::<str>::from(opening);
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
        let raw = Cow::<str>::from(rope.line(line));
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
    // 单行 `$$...$$` 公式没有独立的闭围栏行。
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

/// 返回第 `line` 行末尾（换行符之前）的位置（字符偏移）。
pub fn line_end(rope: &Rope, line: usize) -> usize {
    let text = rope.line(line);
    let ending = text
        .chars_at(text.len_chars())
        .reversed()
        .take_while(|c| matches!(c, '\n' | '\r'))
        .count();
    rope.line_to_char(line) + text.len_chars() - ending
}

/// 第 `line` 行的文字，不含换行符。
pub fn line_text(rope: &Rope, line: usize) -> String {
    let text = rope.line(line).to_string();
    text.trim_end_matches(['\n', '\r']).to_owned()
}

/// 找段落的边界时往前、往后各看多少行：很长的一段不再找下去。
const PARAGRAPH_LOOK: usize = 500;

/// 光标 `at`（字符位置）所在的段落：前后到空行为止的首行和末行，各方向最多看 500 行。
pub fn paragraph_lines(rope: &Rope, at: usize) -> (usize, usize) {
    let line = rope.char_to_line(at.min(rope.len_chars()));
    let blank = |l: usize| line_text(rope, l).trim().is_empty();
    if blank(line) {
        return (line, line);
    }
    let lines = rope.len_lines();
    let first = (line.saturating_sub(PARAGRAPH_LOOK)..line)
        .rev()
        .find(|&l| blank(l))
        .map_or(line.saturating_sub(PARAGRAPH_LOOK), |l| l + 1);
    let last = (line + 1..lines.min(line + 1 + PARAGRAPH_LOOK))
        .find(|&l| blank(l))
        .map_or((lines - 1).min(line + PARAGRAPH_LOOK), |l| l - 1);
    (first, last)
}

/// `lines` 中的各行结束后是否处于围栏代码块内。
///
/// 依据 `CommonMark` 统计开、闭围栏：行首最多 3 个空格，之后为至少 3 个反引号或波浪号；
/// 闭围栏与开围栏字符相同、长度不短于开围栏，且其后没有其他字符。
pub fn in_fence<'a>(lines: impl Iterator<Item = RopeSlice<'a>>) -> bool {
    let mut open: Option<(u8, usize)> = None;
    for line in lines {
        let text = Cow::<str>::from(line);
        let text = text.trim_end_matches(['\n', '\r']);
        let trimmed = text.trim_start_matches(' ');
        if text.len() - trimmed.len() > 3 {
            continue;
        }
        let Some(&mark) = trimmed
            .as_bytes()
            .first()
            .filter(|b| matches!(b, b'`' | b'~'))
        else {
            continue;
        };
        let run = trimmed.bytes().take_while(|&b| b == mark).count();
        if run < 3 {
            continue;
        }
        match open {
            None if mark == b'~' || !trimmed[run..].contains('`') => open = Some((mark, run)),
            Some((m, n)) if m == mark && run >= n && trimmed[run..].trim_end().is_empty() => {
                open = None;
            }
            _ => {}
        }
    }
    open.is_some()
}

/// 网址 `url` 是否指向图片（按 `?`、`#` 之前的扩展名判断）。
pub fn is_image_url(url: &str) -> bool {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let images = [
        ".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".avif", ".bmp",
    ];
    url.starts_with("http") && images.iter().any(|ext| path.ends_with(ext))
}

/// `text` 是否像一个网址。
pub fn is_url(text: &str) -> bool {
    ["https://", "http://", "mailto:"]
        .iter()
        .any(|p| text.len() > p.len() && text.starts_with(p))
        && !text.contains(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn change_map_tracks_insert_delete_and_replace_positions() {
        // 在偏移 5 处插入 3 个单位。
        let ins = ChangeMap::single(5, 0, 3);
        assert_eq!(ins.map_pos(4, Assoc::Before), 4);
        assert_eq!(ins.map_pos(5, Assoc::Before), 5);
        assert_eq!(ins.map_pos(5, Assoc::After), 8);
        assert_eq!(ins.map_pos(6, Assoc::Before), 9);

        // 紧贴装饰区间的两侧输入不会把新字吸进装饰区间。
        assert_eq!(ins.map_range(5..10), 8..13);
        assert_eq!(ins.map_range(0..5), 0..5);
        // 块范围允许在边界向外扩展。
        assert_eq!(ins.map_block_range(5..10), 5..13);
        assert_eq!(ins.map_block_range(0..5), 0..8);

        // 删除区间 4..8 替换为 2 个单位（4..6）。
        let rep = ChangeMap::single(4, 4, 2);
        assert_eq!(rep.map_pos(3, Assoc::After), 3);
        assert_eq!(rep.map_pos(4, Assoc::After), 4);
        assert_eq!(rep.map_pos(6, Assoc::Before), 4);
        assert_eq!(rep.map_pos(6, Assoc::After), 6);
        assert_eq!(rep.map_pos(8, Assoc::Before), 6);
        assert_eq!(rep.map_pos(10, Assoc::Before), 8);
        assert_eq!(rep.len_delta(), -2);
    }

    #[test]
    fn change_seq_composes_multiple_edits_in_order() {
        let mut seq = ChangeSeq::default();
        // 先在 2 处插入 4 个字符，再在 10 处删除 3 个字符。
        seq.push(ChangeMap::single(2, 0, 4));
        seq.push(ChangeMap::single(10, 3, 0));

        // 原位置 12 -> +4 = 16 -> -3 = 13。
        assert_eq!(seq.map_pos(12, Assoc::Before), 13);
        // 原区间 6..9 -> 10..13 -> 全部落入删除区间 -> 10..10。
        assert_eq!(seq.map_range(6..9), 10..10);
    }

    #[test]
    fn coordinate_bridge_handles_multibyte_utf8_and_clamping() {
        let text = "你好🚀world\n第二行";
        let rope = Rope::from_str(text);
        let parsed = markdown::parse(text);
        let src = SourceBridge::new(&rope, &parsed, 1, Some(1));

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
        let src = SourceBridge::new(&rope, &parsed, 2, Some(1));

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
        let src = SourceBridge::new(&rope, &parsed, 5, Some(5));

        assert!(src.is_fresh());
        assert_eq!(src.hidden_prefix_chars(0), 2);
        assert_eq!(src.hidden_prefix_chars(1), 2);
        assert_eq!(src.hidden_prefix_chars(3), 6);

        let on_quote = src.leaf_at(0).expect("引用前缀应命中引用叶子块");
        assert_eq!(on_quote.leaf.start, 0);
        assert!(on_quote.prose);

        let code_at = text.find("let x = 1;").unwrap();
        assert_eq!(src.prose_at(code_at), Some(false));
        let fence = src.fenced_at(code_at).expect("应识别围栏代码块");
        assert!(fence.closed);
        assert!(!fence.bare);
        assert!(!fence.empty);
        assert_eq!(fence.first, 5);
        assert_eq!(fence.last, 8);
        assert_eq!(fence.content, 6..8);

        let fence_open = text.find("```rust").unwrap();
        assert_eq!(src.leaf_at(fence_open).unwrap().edge, Some(5));
        assert_eq!(src.leaf_at(code_at).unwrap().edge, None);

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
        let src = SourceBridge::new(&rope, &parsed, 1, Some(1));
        assert!(src.fenced_at(4).is_none());
    }
}
