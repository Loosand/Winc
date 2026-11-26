//! Markdown 源码字节偏移对照表（[`SourceMap`]）、有意空行记录（[`BlankRows`]）与标题锚点生成。
//!
//! 本模块不依赖上层 AST 类型，供解析器与增量更新单向调用。

use std::ops::Range;
use std::sync::Arc;

/// 在原文中查找下一个字时往后找的最远距离（字节，经验值）。
///
/// 该值明显超过常见 HTML 标签（带长网址的 `<img>`、`<a>`）的长度，并避免在长 HTML 块中
/// 每次查找都扫到块尾。
const REACH: usize = 4096;

/// 顶层块之间与文档末尾的有意空行位置，供实时预览排版留出对应的空白高度。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BlankRows {
    offsets: Arc<[usize]>,
    before: Vec<Range<usize>>,
    after: Range<usize>,
}

impl BlankRows {
    /// Indices in [`Self::offsets`] for the gap before the given top-level block.
    pub fn before(&self, block: usize) -> Range<usize> {
        self.before.get(block).cloned().unwrap_or_default()
    }

    /// 正文顶层块（不含移到文末的脚注）的空行槽位总数。
    pub fn before_len(&self) -> usize {
        self.before.len()
    }

    /// Source range of the preceding gap when it consists of exactly one blank line between blocks.
    pub(super) fn separator_before(&self, block: usize) -> Option<Range<usize>> {
        match &self.offsets[self.before(block)] {
            [start, end] => Some(*start..*end),
            _ => None,
        }
    }

    /// Indices in [`Self::offsets`] for the gap at the end of the document.
    pub fn after(&self) -> Range<usize> {
        self.after.clone()
    }

    /// Shared byte offsets, so a reflow only needs to update the on-screen geometry.
    pub fn offsets(&self) -> &Arc<[usize]> {
        &self.offsets
    }

    /// 局部增量重解析后，把 `[b0, b1)` 范围内的空行表替换为 `local` 的空行表，并平移后续偏移。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn splice_local(
        &mut self,
        local: &BlankRows,
        b0: usize,
        b1: usize,
        main: usize,
        local_len: usize,
        old_start: usize,
        byte_delta: isize,
    ) {
        let shift_byte = |pos: usize| (pos as isize + byte_delta) as usize;
        let new_main = main - (b1 - b0) + local_len;
        let can_patch = local_len == b1 - b0 && b1 < main && {
            let old_span_start = self.before(b0).start;
            let old_span_end = self.before(b1).end;
            let mut new_local_count = 0usize;
            for i in 0..local_len {
                new_local_count += local.before(i).len();
            }
            let after_slice = &local.offsets[local.after.clone()];
            if after_slice.len() >= 3 {
                new_local_count += after_slice.len() - 1;
            }
            old_span_end - old_span_start == new_local_count
        };
        if can_patch {
            let old_span_start = self.before(b0).start;
            let old_span_end = self.before(b1).end;
            let mut offsets = self.offsets.to_vec();
            let mut cursor = old_span_start;
            for i in 0..local_len {
                let r = local.before(i);
                let s = cursor;
                for &o in &local.offsets[r] {
                    offsets[cursor] = o + old_start;
                    cursor += 1;
                }
                self.before[b0 + i] = s..cursor;
            }
            let s = cursor;
            let after_slice = &local.offsets[local.after.clone()];
            if after_slice.len() >= 3 {
                for &o in &after_slice[..after_slice.len() - 1] {
                    offsets[cursor] = o + old_start;
                    cursor += 1;
                }
            }
            self.before[b1] = s..cursor;
            if byte_delta != 0 {
                for o in &mut offsets[old_span_end..] {
                    *o = shift_byte(*o);
                }
            }
            self.offsets = offsets.into();
            return;
        }

        let mut offsets = Vec::with_capacity(self.offsets.len() + local.offsets.len());
        let mut before = Vec::with_capacity(new_main);
        for i in 0..b0 {
            let r = self.before(i);
            let s = offsets.len();
            offsets.extend_from_slice(&self.offsets[r]);
            before.push(s..offsets.len());
        }
        for i in 0..local_len {
            let r = local.before(i);
            let s = offsets.len();
            offsets.extend(local.offsets[r].iter().map(|&o| o + old_start));
            before.push(s..offsets.len());
        }
        if b1 < main {
            let s = offsets.len();
            let after_slice = &local.offsets[local.after.clone()];
            if after_slice.len() >= 2 {
                let useful = &after_slice[..after_slice.len() - 1];
                if useful.len() >= 2 {
                    offsets.extend(useful.iter().map(|&o| o + old_start));
                }
            }
            before.push(s..offsets.len());
            for i in (b1 + 1)..main {
                let r = self.before(i);
                let s = offsets.len();
                offsets.extend(self.offsets[r].iter().map(|&o| shift_byte(o)));
                before.push(s..offsets.len());
            }
        }
        let after_start = offsets.len();
        if b1 < main {
            let r = self.after();
            offsets.extend(self.offsets[r].iter().map(|&o| shift_byte(o)));
        } else {
            let r = local.after();
            offsets.extend(local.offsets[r].iter().map(|&o| o + old_start));
        }
        self.offsets = offsets.into();
        self.before = before;
        self.after = after_start..self.offsets.len();
    }
}

/// 文字与原文的对照：文字中的字节偏移对应原文中的哪个字节偏移。
///
/// 按文字的先后记下若干段：每段文字与原文逐字节相同；不在任何一段里的文字（软换行换成的
/// 空格、emoji、脚注编号这类由原文生成的文字）对应前一段的末尾。
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash)]
pub struct SourceMap(Vec<Anchor>);

/// [`SourceMap`] 的一段：文字从 `text` 起、原文从 `source` 起的 `len` 个字节相同；`len` 为 0
/// 时只是一个对应点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Anchor {
    text: u32,
    source: u32,
    len: u32,
}

/// 把原文的字节偏移转成 [`Anchor`] 中记录的 `u32`；超过 4 GB 时返回 `None`。
///
/// 超过 4 GB 的文件按大文件打开，不生成预览，因此不记录文字与原文的对照。
fn offset(at: usize) -> Option<u32> {
    u32::try_from(at).ok()
}

impl SourceMap {
    /// 文字中第 `at` 个字节对应原文中的字节偏移；`at` 在第一段之前或没有对照时为 `None`。
    pub fn source(&self, at: usize) -> Option<usize> {
        let i = self
            .0
            .partition_point(|a| a.text as usize <= at)
            .checked_sub(1)?;
        let a = self.0[i];
        Some(a.source as usize + (at - a.text as usize).min(a.len as usize))
    }

    /// 文字中第 `at` 个字节作为半开区间终点时，对应原文的排他终点字节偏移（不跨越到下一段标记之后）。
    pub fn source_end(&self, at: usize) -> Option<usize> {
        let i = self
            .0
            .partition_point(|a| (a.text as usize) < at)
            .checked_sub(1)?;
        let a = self.0[i];
        Some(a.source as usize + (at - a.text as usize).min(a.len as usize))
    }

    /// 仅当文字中第 `at` 个字节真正落在某条原文对照段内（或恰好命中零长度锚点）时返回其原文位置；
    /// 附加在段落末尾的生成文字（如脚注末尾的 ` ↩`）返回 `None`，避免污染真实正文的 `Span`。
    pub fn char_source(&self, at: usize) -> Option<usize> {
        let i = self
            .0
            .partition_point(|a| a.text as usize <= at)
            .checked_sub(1)?;
        let a = self.0[i];
        let off = at - a.text as usize;
        if a.len == 0 {
            (off == 0).then_some(a.source as usize)
        } else {
            (off < a.len as usize).then_some(a.source as usize + off)
        }
    }

    /// 占用的堆内存（字节，按容量计算），用于内存测量。
    pub fn heap_bytes(&self) -> usize {
        self.0.capacity() * std::mem::size_of::<Anchor>()
    }

    /// 对照的段数。
    pub fn anchors(&self) -> usize {
        self.0.len()
    }

    /// 是否没有任何对照。
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// 记下文字从 `text` 起的 `len` 个字节与原文从 `source` 起的字节相同；与前一段首尾相接时
    /// 合并。
    pub(super) fn push(&mut self, text: usize, source: usize, len: usize) {
        let (Some(text), Some(source), Some(len)) = (offset(text), offset(source), offset(len))
        else {
            return;
        };
        if let Some(last) = self.0.last_mut() {
            if last.len > 0 && last.text + last.len == text && last.source + last.len == source {
                last.len += len;
                return;
            }
            // 同一处的对应点只留最后一个。
            if last.len == 0 && last.text == text {
                *last = Anchor { text, source, len };
                return;
            }
        }
        self.0.push(Anchor { text, source, len });
    }

    /// 文字中 `range` 内的字节换成了 `len` 个字节的新文字。
    ///
    /// 新文字对应原范围的起点，其后各段的文字偏移按长度差平移。
    pub(super) fn replace(&mut self, range: Range<usize>, len: usize) {
        let Some(start) = self.source(range.start) else {
            return;
        };
        let (from, to) = (range.start as i64, range.end as i64);
        let delta = len as i64 - (to - from);
        let mut out = Vec::with_capacity(self.0.len() + 1);
        for a in &self.0 {
            let (text, end) = (a.text as i64, a.text as i64 + a.len as i64);
            if text < from {
                let len = (end.min(from) - text) as u32;
                out.push(Anchor { len, ..*a });
            }
        }
        out.push(Anchor {
            text: range.start as u32,
            source: start as u32,
            len: 0,
        });
        for a in &self.0 {
            let (text, end) = (a.text as i64, a.text as i64 + a.len as i64);
            if text >= to {
                out.push(Anchor {
                    text: (text + delta) as u32,
                    ..*a
                });
            } else if end > to {
                let cut = (to - text) as u32;
                out.push(Anchor {
                    text: (to + delta) as u32,
                    source: a.source + cut,
                    len: a.len - cut,
                });
            }
        }
        self.0 = out;
    }

    /// 最靠前的原文位置。
    pub(super) fn first(&self) -> Option<usize> {
        self.0.iter().map(|a| a.source as usize).min()
    }

    /// 枚举全部对照锚点 `(text_start, source_start, len)`。
    pub(super) fn iter_spans(&self) -> impl Iterator<Item = (usize, usize, usize)> + '_ {
        self.0
            .iter()
            .map(|a| (a.text as usize, a.source as usize, a.len as usize))
    }

    /// 回收对照表多余的堆容量。
    pub(super) fn shrink_to_fit(&mut self) {
        self.0.shrink_to_fit();
    }

    /// 记录的各段原文位置都减去 `by`，改为相对于原文第 `by` 个字节。
    pub(super) fn rebase(&mut self, by: usize) {
        let by = offset(by).unwrap_or(u32::MAX);
        for a in &mut self.0 {
            a.source = a.source.saturating_sub(by);
        }
    }
}

/// 在原文 `source` 的 `*cursor..end` 中找刚放进文字第 `at` 个字节起的 `text`，记到 `map`，
/// `*cursor` 移到找到处之后。
///
/// 通常 `text` 原样出现在原文中（行内代码、公式在定界符里面）；含转义、实体时逐字往后找，
/// 找不到的字对应当前位置。查找限于 [`REACH`] 以内，HTML 块很长时耗时也不随长度增长。
pub(super) fn locate(
    map: &mut SourceMap,
    at: usize,
    text: &str,
    source: &str,
    cursor: &mut usize,
    end: usize,
) {
    let window = |from: usize, reach: usize| {
        let mut to = end.min(from.saturating_add(reach)).max(from);
        while !source.is_char_boundary(to) {
            to -= 1;
        }
        source.get(from..to).unwrap_or_default()
    };
    if text.is_empty() {
        return;
    }
    if let Some(p) = window(*cursor, text.len() + REACH).find(text) {
        map.push(at, *cursor + p, text.len());
        *cursor += p + text.len();
        return;
    }
    for (i, c) in text.char_indices() {
        match window(*cursor, REACH).find(c) {
            Some(p) => {
                map.push(at + i, *cursor + p, c.len_utf8());
                *cursor += p + c.len_utf8();
            }
            None => map.push(at + i, *cursor, 0),
        }
    }
}

/// Recover intentional blank rows without treating hidden definitions or generated blocks as spacing.
pub(super) fn blank_rows(source: &str, ranges: &[Range<usize>], main: usize) -> BlankRows {
    let mut before = vec![0..0; main];
    let mut offsets = Vec::new();
    let is_white = |b: &u8| matches!(b, b' ' | b'\t' | b'\r' | b'\n');
    let source = source.as_bytes();
    let count = |bytes: &[u8]| memchr::memchr_iter(b'\n', bytes).count();
    let end_of_content = |bytes: &[u8]| {
        bytes
            .iter()
            .rposition(|b| !is_white(b))
            .map_or(0, |i| i + 1)
    };
    let line_start = |at| memchr::memrchr2(b'\n', b'\r', &source[..at]).map_or(0, |i| i + 1);
    let previous_line = |at: usize| {
        // Match Ropey's CR, LF and CRLF boundaries; do not split a CRLF pair.
        let newline = at.saturating_sub(1);
        let before = newline
            - usize::from(
                source.get(newline) == Some(&b'\n') && newline > 0 && source[newline - 1] == b'\r',
            );
        line_start(before)
    };
    let mut push = |rows: usize, at, tail| {
        let start = offsets.len();
        if rows > 0 {
            let mut line = line_start(at);
            if tail {
                offsets.push(source.len() + 1); // EOF itself can be a blank-row start
            }
            for _ in 0..rows {
                offsets.push(line);
                line = previous_line(line);
            }
            if !tail {
                offsets.push(line);
            }
            offsets[start..].reverse();
        }
        start..offsets.len()
    };
    for i in 0..main {
        let start = ranges[i].start;
        let rows = if i == 0 {
            let lead = &source[..start];
            if lead.iter().all(is_white) {
                count(lead)
            } else {
                0
            }
        } else {
            let prev = &ranges[i - 1];
            if prev.start >= start || prev.end > start {
                continue;
            }
            let prefix = &source[prev.start..start];
            let content_end = end_of_content(prefix);
            if content_end != 0 && prev.start + content_end <= prev.end {
                count(&prefix[content_end..]).saturating_sub(1)
            } else {
                0
            }
        };
        before[i] = push(rows, start, false);
    }
    let after = ranges
        .last()
        .filter(|_| main == ranges.len())
        .map_or(0, |last| {
            let tail = &source[last.start..];
            let end = end_of_content(tail);
            if end != 0 && last.start + end <= last.end {
                count(&tail[end..])
            } else {
                0
            }
        });
    let after = push(after, source.len(), true);
    BlankRows {
        offsets: offsets.into(),
        before,
        after,
    }
}

/// 给标题生成锚点的规则：转成小写，空格换成 `-`，去掉其他标点（保留 `-`、`_`）。
pub fn slug(text: &str) -> String {
    text.chars()
        .filter_map(|c| {
            if c == ' ' {
                Some('-')
            } else if c.is_alphanumeric() || c == '-' || c == '_' {
                Some(c)
            } else {
                None
            }
        })
        .flat_map(char::to_lowercase)
        .collect()
}
