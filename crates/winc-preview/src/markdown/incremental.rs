//! 块级增量 Markdown 重解析。
//!
//! 常规输入、换行、退格只影响当前顶层块及其相邻空行，不需要重新拼接和解析整篇文档。
//! [`reparse_local`] 仅切片重解析被改动覆盖的局部窗口，并把结果与原有块表、
//! 源码范围、行号、叶子表和空行表精确拼接，产出与全文 [`super::parse`] 逐字段一致的
//! [`Markdown`]；遇到可能影响跨块语义的场景（YAML 属性头、脚注、链接引用定义、围栏边界、
//! HTML 块、目录标题变更等）时安全返回 `None`，回退到全文解析。

use std::ops::Range;

use super::{Block, LeafRange, Markdown, parse};

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

struct LocalPlan {
    b0: usize,
    b1: usize,
    main: usize,
    old_start: usize,
    old_end: usize,
    byte_delta: isize,
    line_delta: isize,
    start_line: usize,
    prefix_leaves: usize,
    suffix_leaves: usize,
    local: Markdown,
}

fn plan_local(
    doc: &Markdown,
    old_edit: Range<usize>,
    new_edit: Range<usize>,
    new_len: usize,
    line_of_byte: &impl Fn(usize) -> usize,
    slice: &impl Fn(Range<usize>) -> String,
) -> Option<LocalPlan> {
    if old_edit.start != new_edit.start
        || old_edit.end > doc.len
        || new_edit.end > new_len
        || doc.has_link_defs
    {
        return None;
    }
    let byte_delta = new_edit.end as isize - old_edit.end as isize;
    if doc.len as isize + byte_delta != new_len as isize {
        return None;
    }
    let main = doc.blank_rows.before_len();
    if main == 0 {
        return None;
    }
    let prop_end = doc.properties.as_ref().map_or(0, |(end, _)| *end);
    if doc.properties.is_some() && old_edit.start < prop_end {
        return None;
    }
    // 不允许编辑与文末脚注定义的原文范围重叠。
    if doc.ranges[main..]
        .iter()
        .any(|r| r.start < old_edit.end && r.end > old_edit.start)
    {
        return None;
    }

    // 确定被编辑覆盖的顶层正文块范围 b0..b1（均在 0..=main 内）。
    let mut b0 = doc.ranges[..main]
        .partition_point(|r| r.end < old_edit.start)
        .min(main);
    // 向前找到与前一块之间存在空行隔离的安全起点。
    while b0 > 0 {
        let gap = doc.blank_rows.before(b0);
        if !gap.is_empty() {
            let gap_start = doc.blank_rows.offsets()[gap.start];
            if gap_start < old_edit.start && gap_start >= prop_end {
                break;
            }
        }
        b0 -= 1;
    }
    if b0 == 0 && doc.properties.is_some() {
        return None;
    }
    let old_start = if b0 == 0 {
        0
    } else {
        doc.blank_rows.offsets()[doc.blank_rows.before(b0).start]
    };

    // 向后找到与后一块之间存在空行隔离的安全终点。
    let mut b1 = doc.ranges[..main]
        .partition_point(|r| r.start <= old_edit.end)
        .max(b0);
    while b1 < main {
        let gap = doc.blank_rows.before(b1);
        if !gap.is_empty() && doc.blank_rows.offsets()[gap.start] > old_edit.end {
            break;
        }
        b1 += 1;
    }
    // 若到达正文末尾且文末存在移位脚注，保守回退全文解析。
    if b1 == main && main < doc.blocks.len() {
        return None;
    }
    let old_end = if b1 < main {
        doc.ranges[b1].start
    } else {
        doc.len
    };
    if old_start > old_edit.start || old_end < old_edit.end {
        return None;
    }
    // 被替换的旧块若含代码块/公式/HTML/折叠块，保守回退全文解析。
    if doc.blocks[b0..b1].iter().any(block_may_affect_boundary) {
        return None;
    }
    let old_has_heading = doc.blocks[b0..b1]
        .iter()
        .any(|b| matches!(b, Block::Heading(..)));
    if doc.has_toc && old_has_heading {
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
        || local.blank_rows.before_len() != local.blocks.len()
        || local.blocks.iter().any(block_may_affect_boundary)
    {
        return None;
    }
    if doc.has_toc && local.blocks.iter().any(|b| matches!(b, Block::Heading(..))) {
        return None;
    }

    // 检查旧叶子表没有跨过窗口边界的项（用二分定位窗口周边，避免线性扫整篇文档）。
    let prefix_leaves = doc.leaves.partition_point(|l| l.range.end <= old_start);
    if let Some(prev) = prefix_leaves.checked_sub(1).and_then(|i| doc.leaves.get(i))
        && prev.range.end > old_start
    {
        return None;
    }
    if doc
        .leaves
        .get(prefix_leaves)
        .is_some_and(|l| l.range.start < old_start && l.range.end > old_start)
    {
        return None;
    }
    let suffix_leaves = doc.leaves.partition_point(|l| l.range.start < old_end);
    if let Some(last) = suffix_leaves.checked_sub(1).and_then(|i| doc.leaves.get(i))
        && last.range.end > old_end
    {
        return None;
    }

    let start_line = line_of_byte(old_start);
    let old_end_line = if b1 < main {
        doc.lines[b1]
    } else {
        doc.lines.last().copied().unwrap_or(0)
    };
    let new_end_line = line_of_byte(new_end);
    let line_delta = new_end_line as isize - old_end_line as isize;

    Some(LocalPlan {
        b0,
        b1,
        main,
        old_start,
        old_end,
        byte_delta,
        line_delta,
        start_line,
        prefix_leaves,
        suffix_leaves,
        local,
    })
}

/// 就地执行块级局部增量重解析：直接复用未改动的所有顶层 [`Block`]（零深拷贝），
/// 返回是否成功完成增量更新；无法局部更新时保持 `doc` 不变并返回 `false`。
pub fn reparse_local_in_place(
    doc: &mut Markdown,
    old_edit: Range<usize>,
    new_edit: Range<usize>,
    new_len: usize,
    line_of_byte: impl Fn(usize) -> usize,
    slice: impl Fn(Range<usize>) -> String,
) -> bool {
    let Some(plan) = plan_local(doc, old_edit, new_edit, new_len, &line_of_byte, &slice) else {
        return false;
    };
    let LocalPlan {
        b0,
        b1,
        main,
        old_start,
        old_end,
        byte_delta,
        line_delta,
        start_line,
        prefix_leaves,
        suffix_leaves,
        local,
    } = plan;
    let local_len = local.blocks.len();

    // 1. 就地替换 blocks 与 hashes（未改动的块零拷贝）。
    if local_len == b1 - b0 {
        for (dst, src) in doc.blocks[b0..b1].iter_mut().zip(local.blocks) {
            *dst = src;
        }
        for (dst, src) in doc.hashes[b0..b1].iter_mut().zip(local.hashes) {
            *dst = src;
        }
    } else {
        doc.blocks.splice(b0..b1, local.blocks);
        doc.hashes.splice(b0..b1, local.hashes);
    }

    // 2. 就地平移并替换 ranges。
    if byte_delta != 0 {
        for range in &mut doc.ranges[b1..main] {
            *range = shift_range(range, byte_delta);
        }
        for range in &mut doc.ranges[main..] {
            if range.start >= old_end {
                *range = shift_range(range, byte_delta);
            }
        }
    }
    let local_ranges = local
        .ranges
        .into_iter()
        .map(|r| (r.start + old_start)..(r.end + old_start));
    if local_len == b1 - b0 {
        for (dst, src) in doc.ranges[b0..b1].iter_mut().zip(local_ranges) {
            *dst = src;
        }
    } else {
        doc.ranges.splice(b0..b1, local_ranges);
    }

    // 3. 就地平移并替换 lines。
    if line_delta != 0 {
        for l in &mut doc.lines[b1..] {
            *l = usize::try_from(*l as isize + line_delta).unwrap_or(0);
        }
    }
    let local_lines = local.lines.into_iter().map(|l| start_line + l);
    if local_len == b1 - b0 {
        for (dst, src) in doc.lines[b0..b1].iter_mut().zip(local_lines) {
            *dst = src;
        }
    } else {
        doc.lines.splice(b0..b1, local_lines);
    }

    // 4. 就地平移并替换 leaves。
    if byte_delta != 0 {
        for leaf in &mut doc.leaves[suffix_leaves..] {
            leaf.range = shift_range(&leaf.range, byte_delta);
        }
    }
    let local_leaves = local.leaves.into_iter().map(|l| LeafRange {
        range: (l.range.start + old_start)..(l.range.end + old_start),
        prose: l.prose,
        table: l.table,
    });
    if local_leaves.len() == suffix_leaves - prefix_leaves {
        for (dst, src) in doc.leaves[prefix_leaves..suffix_leaves]
            .iter_mut()
            .zip(local_leaves)
        {
            *dst = src;
        }
    } else {
        doc.leaves
            .splice(prefix_leaves..suffix_leaves, local_leaves);
    }

    // 5. 更新 blank_rows：块数和局部空行段长度不变时直接就地平移 offsets，否则重建紧凑表。
    doc.blank_rows.splice_local(
        &local.blank_rows,
        b0,
        b1,
        main,
        local_len,
        old_start,
        byte_delta,
    );

    doc.len = new_len;
    true
}

/// 尝试仅局部重解析本次单一连续编辑区间（`old_edit` -> `new_edit`），返回与全文
/// [`parse`] 逐字段一致的新 [`Markdown`]；若编辑可能影响跨块或全局结构则返回 `None`。
///
/// - `old_edit`：编辑前在旧文档中的字节范围。
/// - `new_edit`：编辑后在新文档中的字节范围（要求 `old_edit.start == new_edit.start`）。
/// - `new_len`：新文档总字节数。
/// - `line_of_byte`：返回新文档中任意字节偏移所在的行号（从 0 起）。
/// - `slice`：按字节范围读取新文档切片。
pub fn reparse_local(
    doc: &Markdown,
    old_edit: Range<usize>,
    new_edit: Range<usize>,
    new_len: usize,
    line_of_byte: impl Fn(usize) -> usize,
    slice: impl Fn(Range<usize>) -> String,
) -> Option<Markdown> {
    let mut next = doc.clone();
    reparse_local_in_place(&mut next, old_edit, new_edit, new_len, line_of_byte, slice)
        .then_some(next)
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

        let inc = reparse_local(&prev, old_range, new_range, next.len(), line_of_byte, slice);
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
