//! 块级增量 Markdown 重解析。
//!
//! 常规输入、换行、退格只影响当前顶层块及其相邻空行，不需要重新拼接和解析整篇文档。
//! [`Markdown::reparse_local`] 仅切片重解析被改动覆盖的局部窗口，并把结果与原有块表、
//! 源码范围、行号、叶子表和空行表精确拼接，产出与全文 [`super::parse`] 逐字段一致的
//! [`Markdown`]；遇到可能影响跨块语义的场景（YAML 属性头、脚注、链接引用定义、围栏边界、
//! HTML 块、目录标题变更等）时安全返回 `None`，回退到全文解析。

use std::ops::Range;

use super::{BlankRows, Block, LeafRange, Markdown, parse};

/// 粗判原文是否包含非脚注的链接引用定义（`[id]: url`）。
pub(super) fn has_link_ref_defs(source: &str) -> bool {
    for line in source.lines() {
        let trimmed = line.trim_start_matches([' ', '\t']);
        if trimmed.starts_with('[') && !trimmed.starts_with("[^") && trimmed.contains("]:") {
            return true;
        }
    }
    false
}

/// 检查一段切片是否包含可能跨越局部窗口边界或影响全局结构的语法。
fn has_global_or_fence_syntax(text: &str) -> bool {
    text.contains("[^")
        || text.contains("```")
        || text.contains("~~~")
        || text.contains("$$")
        || text.contains('<')
        || text.contains("]:")
        || text.contains("[toc]")
        || text.contains("[TOC]")
        || text.contains("[[toc]]")
        || text.contains("[[TOC]]")
        || text
            .as_bytes()
            .windows(5)
            .any(|w| w.eq_ignore_ascii_case(b"[toc]"))
}

/// `bytes` 是否以至少一个完整空白行结尾（用于确认窗口与后一块之间仍由空行隔离）。
fn ends_with_blank_line(bytes: &[u8]) -> bool {
    let Some(last_nl) = bytes.len().checked_sub(1).filter(|&i| bytes[i] == b'\n') else {
        return false;
    };
    let mut i = last_nl;
    if i > 0 && bytes[i - 1] == b'\r' {
        i -= 1;
    }
    while i > 0 && matches!(bytes[i - 1], b' ' | b'\t') {
        i -= 1;
    }
    i > 0 && bytes[i - 1] == b'\n'
}

/// `bytes` 是否以至少一个完整空白行开头（用于确认窗口与前一块之间仍由空行隔离）。
fn starts_with_blank_line(bytes: &[u8]) -> bool {
    let mut i = 0;
    while i < bytes.len() && matches!(bytes[i], b' ' | b'\t') {
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b'\r' {
        i += 1;
    }
    i < bytes.len() && bytes[i] == b'\n'
}

fn shift_byte(at: usize, delta: isize) -> usize {
    usize::try_from(at as isize + delta).unwrap_or(0)
}

fn shift_range(range: &Range<usize>, delta: isize) -> Range<usize> {
    shift_byte(range.start, delta)..shift_byte(range.end, delta)
}

fn block_may_affect_boundary(block: &Block) -> bool {
    matches!(
        block,
        Block::Code { .. }
            | Block::Math(..)
            | Block::Details { .. }
            | Block::Footnote(..)
            | Block::Rule(None)
    )
}

impl Markdown {
    /// 尝试仅局部重解析本次单一连续编辑区间（`old_edit` -> `new_edit`），返回与全文
    /// [`parse`] 逐字段一致的新 [`Markdown`]；若编辑可能影响跨块或全局结构则返回 `None`。
    ///
    /// - `old_edit`：编辑前在旧文档中的字节范围。
    /// - `new_edit`：编辑后在新文档中的字节范围（要求 `old_edit.start == new_edit.start`）。
    /// - `new_len`：新文档总字节数。
    /// - `line_of_byte`：返回新文档中任意字节偏移所在的行号（从 0 起）。
    /// - `slice`：按字节范围读取新文档切片。
    pub fn reparse_local(
        &self,
        old_edit: Range<usize>,
        new_edit: Range<usize>,
        new_len: usize,
        line_of_byte: impl Fn(usize) -> usize,
        slice: impl Fn(Range<usize>) -> String,
    ) -> Option<Markdown> {
        if old_edit.start != new_edit.start
            || old_edit.end > self.len
            || new_edit.end > new_len
            || self.has_link_defs
        {
            return None;
        }
        let byte_delta = new_edit.end as isize - old_edit.end as isize;
        if self.len as isize + byte_delta != new_len as isize {
            return None;
        }
        let main = self.blank_rows.before.len();
        if main == 0 {
            return None;
        }
        let prop_end = self.properties.as_ref().map_or(0, |(end, _)| *end);
        if self.properties.is_some() && old_edit.start < prop_end {
            return None;
        }
        // 不允许编辑与文末脚注定义的原文范围重叠。
        if self.ranges[main..]
            .iter()
            .any(|r| r.start < old_edit.end && r.end > old_edit.start)
        {
            return None;
        }

        // 确定被编辑覆盖的顶层正文块范围 b0..b1（均在 0..=main 内）。
        let mut b0 = self.ranges[..main]
            .partition_point(|r| r.end < old_edit.start)
            .min(main);
        // 向前找到与前一块之间存在空行隔离的安全起点。
        while b0 > 0 {
            let gap = self.blank_rows.before(b0);
            if let Some(&gap_start) = gap
                .start
                .checked_sub(0)
                .and_then(|s| (!gap.is_empty()).then(|| &self.blank_rows.offsets[s]))
                && gap_start < old_edit.start
                && gap_start >= prop_end
            {
                break;
            }
            b0 -= 1;
        }
        if b0 == 0 && self.properties.is_some() {
            return None;
        }
        let old_start = if b0 == 0 {
            0
        } else {
            self.blank_rows.offsets[self.blank_rows.before(b0).start]
        };

        // 向后找到与后一块之间存在空行隔离的安全终点。
        let mut b1 = self.ranges[..main]
            .partition_point(|r| r.start <= old_edit.end)
            .max(b0);
        while b1 < main {
            let gap = self.blank_rows.before(b1);
            if !gap.is_empty() && self.blank_rows.offsets[gap.start] > old_edit.end {
                break;
            }
            b1 += 1;
        }
        // 若到达正文末尾且文末存在移位脚注，保守回退全文解析。
        if b1 == main && main < self.blocks.len() {
            return None;
        }
        let old_end = if b1 < main {
            self.ranges[b1].start
        } else {
            self.len
        };
        if old_start > old_edit.start || old_end < old_edit.end {
            return None;
        }
        // 被替换的旧块若含代码块/公式/HTML/折叠块，保守回退全文解析。
        if self.blocks[b0..b1].iter().any(block_may_affect_boundary) {
            return None;
        }
        let old_has_heading = self.blocks[b0..b1]
            .iter()
            .any(|b| matches!(b, Block::Heading(..)));
        if self.has_toc && old_has_heading {
            return None;
        }

        let new_end = shift_byte(old_end, byte_delta);
        if new_end < old_start || new_end > new_len {
            return None;
        }
        let window_text = slice(old_start..new_end);
        // 局部窗口边界校验：非首块窗口必须仍以空行起头，非尾块窗口必须仍以空行结尾。
        if b0 > 0 && !starts_with_blank_line(window_text.as_bytes()) {
            return None;
        }
        if b1 < main && !ends_with_blank_line(window_text.as_bytes()) {
            return None;
        }
        if old_start == 0 && window_text.starts_with("---") {
            return None;
        }
        if has_global_or_fence_syntax(&window_text) {
            return None;
        }

        let local = parse(&window_text);
        if local.blocks.is_empty()
            || local.properties.is_some()
            || local.has_link_defs
            || local.has_toc
            || local.blank_rows.before.len() != local.blocks.len()
            || local.blocks.iter().any(block_may_affect_boundary)
        {
            return None;
        }
        if self.has_toc && local.blocks.iter().any(|b| matches!(b, Block::Heading(..))) {
            return None;
        }

        // 检查旧叶子表没有跨过窗口边界的项。
        if self.leaves.iter().any(|l| {
            (l.range.start < old_start && l.range.end > old_start)
                || (l.range.start < old_end && l.range.end > old_end)
        }) {
            return None;
        }

        let local_len = local.blocks.len();
        let new_main = main - (b1 - b0) + local_len;

        // 1. 拼接 blocks 与 hashes。
        let mut blocks = Vec::with_capacity(self.blocks.len() - (b1 - b0) + local_len);
        blocks.extend_from_slice(&self.blocks[..b0]);
        blocks.extend(local.blocks);
        blocks.extend_from_slice(&self.blocks[b1..]);

        let mut hashes = Vec::with_capacity(blocks.len());
        hashes.extend_from_slice(&self.hashes[..b0]);
        hashes.extend(local.hashes);
        hashes.extend_from_slice(&self.hashes[b1..]);

        // 2. 拼接 ranges（正文与文末脚注按相对窗口的位置分别平移）。
        let mut ranges = Vec::with_capacity(blocks.len());
        ranges.extend_from_slice(&self.ranges[..b0]);
        ranges.extend(
            local
                .ranges
                .iter()
                .map(|r| (r.start + old_start)..(r.end + old_start)),
        );
        for range in &self.ranges[b1..main] {
            ranges.push(shift_range(range, byte_delta));
        }
        for range in &self.ranges[main..] {
            if range.start >= old_end {
                ranges.push(shift_range(range, byte_delta));
            } else {
                ranges.push(range.clone());
            }
        }

        // 3. 拼接 lines。
        let start_line = line_of_byte(old_start);
        let old_end_line = if b1 < main {
            self.lines[b1]
        } else {
            // 文末总行数
            self.lines.last().copied().unwrap_or(0)
        };
        let new_end_line = line_of_byte(new_end);
        let line_delta = new_end_line as isize - old_end_line as isize;
        let mut lines = Vec::with_capacity(blocks.len());
        lines.extend_from_slice(&self.lines[..b0]);
        lines.extend(local.lines.iter().map(|&l| start_line + l));
        for &l in &self.lines[b1..] {
            lines.push(usize::try_from(l as isize + line_delta).unwrap_or(0));
        }

        // 4. 拼接 leaves。
        let prefix_leaves = self.leaves.partition_point(|l| l.range.end <= old_start);
        let suffix_leaves = self.leaves.partition_point(|l| l.range.start < old_end);
        let mut leaves = Vec::with_capacity(
            prefix_leaves + local.leaves.len() + (self.leaves.len() - suffix_leaves),
        );
        leaves.extend_from_slice(&self.leaves[..prefix_leaves]);
        leaves.extend(local.leaves.into_iter().map(|l| LeafRange {
            range: (l.range.start + old_start)..(l.range.end + old_start),
            prose: l.prose,
            table: l.table,
        }));
        leaves.extend(self.leaves[suffix_leaves..].iter().map(|l| LeafRange {
            range: shift_range(&l.range, byte_delta),
            prose: l.prose,
            table: l.table,
        }));

        // 5. 拼接 blank_rows。
        let mut offsets =
            Vec::with_capacity(self.blank_rows.offsets.len() + local.blank_rows.offsets.len());
        let mut before = Vec::with_capacity(new_main);

        // 前缀 [0..b0]：原样保留。
        for i in 0..b0 {
            let r = self.blank_rows.before(i);
            let s = offsets.len();
            offsets.extend_from_slice(&self.blank_rows.offsets[r]);
            before.push(s..offsets.len());
        }
        // 局部块 [0..local_len]：全部平移 +old_start。
        for i in 0..local_len {
            let r = local.blank_rows.before(i);
            let s = offsets.len();
            offsets.extend(local.blank_rows.offsets[r].iter().map(|&o| o + old_start));
            before.push(s..offsets.len());
        }
        // 若后面还有正文块 b1：利用 local.blank_rows.after 恢复 b1 之前的空行表。
        if b1 < main {
            let s = offsets.len();
            let after_slice = &local.blank_rows.offsets[local.blank_rows.after.clone()];
            // local.blank_rows.after 含末尾哨兵 `window_text.len() + 1` 以及结束最后一块的那一个 `\n`：
            // 当 window_text 以 k 个空行结尾时，after_slice 长度为 k + 2（首元素为最后一块结束行下一行，
            // 倒数第二个元素为 window_text.len()，最后一个为 window_text.len() + 1）。
            // 去掉末尾哨兵即得到长度 k + 1 的 `before[b1]`。
            if after_slice.len() >= 2 {
                let useful = &after_slice[..after_slice.len() - 1];
                // 仅当至少有 1 行空行（useful.len() >= 2）时记录；ends_with_blank_line 已确保这一点。
                if useful.len() >= 2 {
                    offsets.extend(useful.iter().map(|&o| o + old_start));
                }
            }
            before.push(s..offsets.len());

            // 其余后缀正文块 [b1 + 1..main]：全部平移 +byte_delta。
            for i in (b1 + 1)..main {
                let r = self.blank_rows.before(i);
                let s = offsets.len();
                offsets.extend(
                    self.blank_rows.offsets[r]
                        .iter()
                        .map(|&o| shift_byte(o, byte_delta)),
                );
                before.push(s..offsets.len());
            }
        }

        // 文末 after 空行表。
        let after_start = offsets.len();
        if b1 < main {
            let r = self.blank_rows.after();
            offsets.extend(
                self.blank_rows.offsets[r]
                    .iter()
                    .map(|&o| shift_byte(o, byte_delta)),
            );
        } else {
            let r = local.blank_rows.after();
            offsets.extend(local.blank_rows.offsets[r].iter().map(|&o| o + old_start));
        }
        let after = after_start..offsets.len();

        Some(Markdown {
            blocks,
            hashes,
            lines,
            blank_rows: BlankRows {
                offsets: offsets.into(),
                before,
                after,
            },
            properties: self.properties.clone(),
            ranges,
            leaves,
            len: new_len,
            has_link_defs: self.has_link_defs,
            has_toc: self.has_toc,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply_and_check(doc: &str, old_range: Range<usize>, insert: &str, expect_incremental: bool) {
        let prev = parse(doc);
        let mut next = String::with_capacity(doc.len() - old_range.len() + insert.len());
        next.push_str(&doc[..old_range.start]);
        next.push_str(insert);
        next.push_str(&doc[old_range.end..]);
        let new_range = old_range.start..(old_range.start + insert.len());
        let line_of_byte = |b: usize| memchr::memchr_iter(b'\n', &next.as_bytes()[..b]).count();
        let slice = |r: Range<usize>| next[r].to_owned();

        let inc = prev.reparse_local(old_range, new_range, next.len(), line_of_byte, slice);
        if expect_incremental {
            let inc = inc.expect("expected incremental reparse to succeed");
            let full = parse(&next);
            assert_eq!(inc.blocks, full.blocks, "blocks mismatch for {next:?}");
            assert_eq!(inc.hashes, full.hashes, "hashes mismatch for {next:?}");
            assert_eq!(inc.lines, full.lines, "lines mismatch for {next:?}");
            assert_eq!(inc.ranges, full.ranges, "ranges mismatch for {next:?}");
            assert_eq!(inc.leaves, full.leaves, "leaves mismatch for {next:?}");
            assert_eq!(
                inc.blank_rows, full.blank_rows,
                "blank_rows mismatch for {next:?}"
            );
            assert_eq!(inc.properties, full.properties);
            assert_eq!(inc.len, full.len);
            assert_eq!(inc, full);
        } else {
            assert!(
                inc.is_none(),
                "expected fallback to full parse for {next:?}"
            );
        }
    }

    #[test]
    fn incremental_reparse_matches_full_parse_bit_for_bit() {
        let doc = concat!(
            "---\n",
            "title: 示例文档\n",
            "---\n\n",
            "# 一级标题\n\n",
            "第一段正文，包含 **加粗** 与 $R_0$。\n\n",
            "第二段第一行  \n",
            "第二段第二行\n\n",
            "- 列表甲\n",
            "- 列表乙\n\n",
            "> 引用第一行\n",
            "> 引用第二行\n\n",
            "| 列一 | 列二 |\n",
            "|---|---|\n",
            "| 甲 | 乙 |\n\n",
            "尾段正文。\n\n\n",
            "最后一段。\n",
        );

        // 1. 段中插入中文与空格
        let at = doc.find("包含").unwrap();
        apply_and_check(doc, at..at, "新插入的中文 ", true);

        // 2. 段尾输入空格
        let at = doc.find("。").unwrap() + "。".len();
        apply_and_check(doc, at..at, "   ", true);

        // 3. 段落中间按回车拆成两段
        let at = doc.find("与 $R_0$").unwrap();
        apply_and_check(doc, at..at, "\n\n", true);

        // 4. 在段间空行上输入空格
        let at = doc.find("\n\n\n").unwrap() + 2;
        apply_and_check(doc, at..at, "    ", true);

        // 5. 列表中插入文字与新项
        let at = doc.find("列表甲").unwrap() + "列表甲".len();
        apply_and_check(doc, at..at, "补充\n- 列表插项", true);

        // 6. 引用中修改文字
        let at = doc.find("引用第一行").unwrap() + "引用第一行".len();
        apply_and_check(doc, at..at, " 续写", true);

        // 7. 表格单元格中修改文字
        let at = doc.find("| 甲").unwrap() + "| 甲".len();
        apply_and_check(doc, at..at, "  测试中", true);

        // 8. 末段修改文字
        let at = doc.find("最后一段。").unwrap() + "最后一段".len();
        apply_and_check(doc, at..at, "新增尾部", true);

        // 9. 删除部分文字
        let from = doc.find("，包含").unwrap();
        let to = from + "，包含".len();
        apply_and_check(doc, from..to, "", true);

        // 10. 含 [TOC] 与脚注的文档：编辑普通段落仍走增量且逐字段一致！
        let toc_doc = concat!(
            "---\n",
            "title: 传染病动力学\n",
            "---\n\n",
            "# 标题一\n\n",
            "正文段落内容。\n\n",
            "[TOC]\n\n",
            "## 标题二\n\n",
            "带脚注引用[^km]的段落。\n\n",
            "另一段普通正文。\n\n",
            "[^km]: 脚注定义。\n",
        );
        let at = toc_doc.find("正文段落内容").unwrap() + "正文段落内容".len();
        apply_and_check(toc_doc, at..at, " 增量编辑中", true);

        // 编辑标题（在含 [TOC] 文档中）应安全回退全文解析以刷新目录。
        let at = toc_doc.find("标题一").unwrap() + "标题一".len();
        apply_and_check(toc_doc, at..at, "改名", false);

        // 编辑 YAML 头应安全回退全文解析。
        let at = toc_doc.find("传染病").unwrap();
        apply_and_check(toc_doc, at..at, "新", false);

        // 引入代码围栏或脚注引用应安全回退全文解析。
        let at = toc_doc.find("正文段落内容").unwrap();
        apply_and_check(toc_doc, at..at, "```rust\n", false);
        apply_and_check(toc_doc, at..at, "[^new]", false);
    }
}
