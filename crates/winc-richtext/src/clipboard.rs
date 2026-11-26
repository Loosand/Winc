//! 富文本选区语法补全、复制与智能粘贴内核。
//!
//! 本模块不依赖系统剪贴板 IO 或窗口状态，仅基于只读原文（[`Rope`]）、解析结构（[`Markdown`]）
//! 与当前选区计算：
//! - 粘贴处上下文（[`prose_spot`]）与智能改写（[`plan_paste_text`]、[`plan_paste_html`]）；
//! - 隐藏 Markdown 语法的选区扩展（[`operation_range`]）与跨行强调修复（[`plan_repair_emphasis`]）；
//! - 富文本/纯文本剪贴板复制载荷（[`plan_copy`]）。

use std::borrow::Cow;
use std::ops::Range;

use ropey::Rope;
use winc_core::{Change, EditKind, Selection, Transaction};
use winc_preview::markdown::{
    self, Block, Markdown, cells, clipboard, enter, format, from_html::from_html, paragraph, reveal,
};

use crate::mapping::{in_fence, is_image_url, is_url, leaf_at_or_before};

/// 生成富文本 HTML 剪贴板载荷的原文大小上限（字节）。
const HTML_BUDGET: usize = 512 * 1024;
/// 单行或跨行段落扫描隐藏标记的字节上限（避免超长单行拖慢交互）。
const SCAN_LIMIT: usize = 32 * 1024;

/// 粘贴处上下文：光标所在行中光标之前、之后的文字，以及上一行是否为空（或不存在）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasteSpot {
    /// 光标之前本行内容。
    pub before: String,
    /// 光标之后本行内容。
    pub after: String,
    /// 上一行是否为空行（或当前就是首行）。
    pub blank_above: bool,
}

impl PasteSpot {
    /// 在当前位置插入块级 Markdown（如表格、多块 HTML）时补足前后空行分隔。
    pub fn block(&self, text: &str, continue_after: bool) -> String {
        let lead = if !self.before.trim().is_empty() {
            "\n\n"
        } else if !self.blank_above {
            "\n"
        } else {
            ""
        };
        let text = text.trim_end_matches('\n');
        let tail = if continue_after || !self.after.trim().is_empty() {
            "\n\n"
        } else {
            ""
        };
        format!("{lead}{text}{tail}")
    }
}

/// 富文本复制规划产出的剪贴板载荷。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyPayload {
    /// 仅写入纯文本（例如显式复制 Markdown 源码、代码块内部或超长文本）。
    Plain(String),
    /// 同时写入 HTML 与纯文本，支持粘贴到富文本目标保留格式。
    Formatted {
        /// HTML 片段。
        html: String,
        /// 对应纯文本。
        plain: String,
    },
}

/// 计算光标处的正文粘贴上下文；光标位于行内代码、代码块、公式或 HTML 块中时返回 `None`。
pub fn prose_spot(
    rope: &Rope,
    selection: Selection,
    parsed: Option<&Markdown>,
    fresh: bool,
) -> Option<PasteSpot> {
    let from = selection.from().min(rope.len_chars());
    let (from_byte, line) = (rope.char_to_byte(from), rope.char_to_line(from));
    let start = rope.line_to_byte(line);
    let before = rope.byte_slice(start..from_byte).to_string();
    if before.matches('`').count() % 2 == 1 {
        return None;
    }
    if fresh && let Some(parsed) = parsed {
        if let Some(leaf) = leaf_at_or_before(parsed, from_byte)
            && leaf.range.contains(&from_byte)
            && !leaf.prose
        {
            return None;
        }
    } else if in_fence(rope.lines_at(0).take(line)) {
        return None;
    }
    let to = selection.to().min(rope.len_chars());
    let end_line = rope.char_to_line(to);
    let end = rope.line_to_byte((end_line + 1).min(rope.len_lines()));
    let after = rope.byte_slice(rope.char_to_byte(to)..end).to_string();
    let blank_above = line == 0 || rope.line(line - 1).to_string().trim().is_empty();
    Some(PasteSpot {
        before,
        after,
        blank_above,
    })
}

/// 计算编辑或复制单选区时应一并覆盖的隐藏 Markdown 语法范围（字符偏移）。
///
/// 当选区完整覆盖某段隐藏标记的可见正文且该标记未因光标展开时，将其起止标记（及完整覆盖的
/// 列表前缀）一并纳入操作范围，避免编辑或删除后留下悬空残缺标记。
pub fn operation_range(
    rope: &Rope,
    selection: Selection,
    parsed: Option<&Markdown>,
    fresh: bool,
    revealed_at: Option<usize>,
) -> Range<usize> {
    if selection.is_empty() {
        return selection.range();
    }
    let Some(parsed) = parsed.filter(|_| fresh) else {
        return selection.range();
    };
    let chars = rope.len_chars();
    let (from, to) = (
        rope.char_to_byte(selection.from().min(chars)),
        rope.char_to_byte(selection.to().min(chars)),
    );
    let (first, last) = (rope.byte_to_line(from), rope.byte_to_line(to));
    let (mut start, mut end) = (from, to);
    let mut paragraph = None;
    for line in std::iter::once(first).chain((last != first).then_some(last)) {
        let text = rope.line(line);
        if text.len_bytes() > SCAN_LIMIT {
            continue;
        }
        let base = rope.line_to_byte(line);
        let text = text.to_string();
        let text = text.trim_end_matches(['\r', '\n']);
        let part = from.saturating_sub(base)..(to - base).min(text.len());
        if part.is_empty() {
            continue;
        }
        let leaf = [base + part.start, base + part.end - 1]
            .into_iter()
            .find_map(|at| {
                leaf_at_or_before(parsed, at).filter(|leaf| leaf.prose && leaf.range.contains(&at))
            });
        let Some(leaf) = leaf else {
            continue;
        };
        let range = &leaf.range;
        let multi_line =
            rope.byte_to_line(range.start) != rope.byte_to_line(range.end.saturating_sub(1));
        let relative_caret = |base: usize, len: usize| {
            revealed_at
                .and_then(|at| at.checked_sub(base))
                .filter(|&at| at <= len)
        };
        let mut expanded = if multi_line && range.len() <= SCAN_LIMIT {
            if paragraph != Some(range.start) {
                let source = Cow::<str>::from(rope.byte_slice(range.clone()));
                let selected =
                    from.saturating_sub(range.start)..(to - range.start).min(source.len());
                let expanded = reveal::include_hidden_paragraph_markers(
                    &source,
                    selected,
                    relative_caret(range.start, source.len()),
                );
                start = start.min(range.start + expanded.start);
                end = end.max(range.start + expanded.end);
                paragraph = Some(range.start);
            }
            start.saturating_sub(base)..(end - base).min(text.len())
        } else {
            clipboard::include_hidden_markers(text, part, relative_caret(base, text.len()))
        };
        if let Some(marker) = enter::list_prefix(text)
            && expanded.start <= marker.end
            && expanded.end >= text.trim_end().len()
        {
            expanded.start = expanded.start.min(marker.start);
        }
        start = start.min(base + expanded.start);
        end = end.max(base + expanded.end);
    }
    rope.byte_to_char(start)..rope.byte_to_char(end)
}

/// 规划 Markdown 单选区复制的剪贴板载荷；选区字节数超过 `max_copy_bytes` 时返回 `None`。
///
/// - `markdown` 为真、位于代码块/公式等字面块内或内容超限时，返回 [`CopyPayload::Plain`]；
/// - 否则同时生成纯文本与 HTML（[`CopyPayload::Formatted`]）。
#[allow(clippy::too_many_arguments)]
pub fn plan_copy(
    rope: &Rope,
    selection: Selection,
    parsed: Option<&Markdown>,
    fresh: bool,
    operation: Option<Range<usize>>,
    markdown: bool,
    max_copy_bytes: u64,
) -> Option<CopyPayload> {
    let parsed = parsed.filter(|_| fresh);
    let expanded = operation.clone().unwrap_or_else(|| selection.range());
    let chars = rope.len_chars();
    let (mut start, mut end) = (expanded.start.min(chars), expanded.end.min(chars));
    let line = rope.char_to_line(start);
    let end_line = rope.char_to_line(end);
    let literal = parsed.is_some_and(|parsed| {
        let from = rope.char_to_byte(start);
        let to = rope.char_to_byte(end);
        leaf_at_or_before(parsed, from).is_some_and(|leaf| {
            !leaf.prose && !leaf.table && from < leaf.range.end && to <= leaf.range.end
        })
    });

    let mut copied_run = None;
    if !literal && end_line == line && rope.line(line).len_bytes() <= SCAN_LIMIT {
        let offset = rope.line_to_byte(line);
        let source = rope.line(line).to_string();
        let range = rope.char_to_byte(start) - offset..rope.char_to_byte(end) - offset;
        let expanded = if operation.is_some() {
            range.clone()
        } else {
            clipboard::include_markers(&source, range.clone())
        };
        if !markdown && expanded == range && (start..end) == selection.range() {
            copied_run = clipboard::formatted_run(&source, range);
        }
        start = rope.byte_to_char(offset + expanded.start);
        end = rope.byte_to_char(offset + expanded.end);
    } else if !literal && !markdown && end_line != line {
        let (a, b) = (rope.char_to_byte(start), rope.char_to_byte(end));
        let leaf_at = |at| {
            let leaf = leaf_at_or_before(parsed?, at)?;
            (leaf.prose && at <= leaf.range.end).then_some(&leaf.range)
        };
        let first = leaf_at(a).map_or(line, |r| rope.byte_to_line(r.start));
        let last = leaf_at(b).map_or(end_line, |r| rope.byte_to_line(r.end));
        let from = rope.line_to_byte(first);
        let to = rope.line_to_byte((last + 1).min(rope.len_lines()));
        if to - from <= HTML_BUDGET {
            let context = Cow::<str>::from(rope.byte_slice(from..to));
            copied_run = clipboard::formatted_range(&context, a - from..b - from);
        }
    }

    let copy_bytes = rope
        .char_to_byte(end)
        .saturating_sub(rope.char_to_byte(start)) as u64;
    if copy_bytes > max_copy_bytes {
        return None;
    }
    let source = rope.slice(start..end).to_string();
    if markdown || literal || source.len() > HTML_BUDGET {
        return Some(CopyPayload::Plain(source));
    }
    let (plain, html) = copied_run.unwrap_or_else(|| clipboard::formatted(&source));
    if plain.is_empty() {
        Some(CopyPayload::Plain(source))
    } else {
        Some(CopyPayload::Formatted { html, plain })
    }
}

/// 规划在正文中粘贴网址时的 Markdown 改写（选中文字变 `[文字](url)`，无选区图片网址变 `![](url)`）。
pub fn plan_paste_url(rope: &Rope, selection: Selection, content: &str) -> Option<String> {
    let url = content.trim();
    if !is_url(url) {
        return None;
    }
    let chars = rope.len_chars();
    let (from, to) = (selection.from().min(chars), selection.to().min(chars));
    let selected = rope.slice(from..to).to_string();
    let link = !selected.is_empty() && !selected.contains(['\n', '`']) && !is_url(selected.trim());
    let image = selected.is_empty() && is_image_url(url);
    if !link && !image {
        return None;
    }
    let text = if image { "" } else { selected.as_str() };
    Some(paragraph::link(text, Some(url), image).0)
}

/// 规划在正文中粘贴制表符分隔内容（TSV）时的 GFM 表格改写。
pub fn plan_paste_tsv(
    selection: Selection,
    spot: &PasteSpot,
    content: &str,
    live: bool,
) -> Option<String> {
    if !selection.is_empty() || !content.contains('\t') {
        return None;
    }
    let table = cells::from_tsv(content)?;
    Some(spot.block(&table, live))
}

/// 统一规划在正文中粘贴纯文本时的智能改写（优先网址转链接/图片，其次 TSV 转表格）。
pub fn plan_paste_text(
    rope: &Rope,
    selection: Selection,
    spot: &PasteSpot,
    content: &str,
    live: bool,
) -> Option<String> {
    plan_paste_url(rope, selection, content)
        .or_else(|| plan_paste_tsv(selection, spot, content, live))
}

/// 规划在正文中粘贴剪贴板 HTML 时的 Markdown 转换与块级分隔。
pub fn plan_paste_html(
    spot: &PasteSpot,
    html: &str,
    has_plain: bool,
    live: bool,
) -> Option<String> {
    let converted = from_html(html);
    let body = converted.text.trim_end();
    if body.is_empty() || (!converted.rich && has_plain) {
        return None;
    }
    let starts_block = ["#", "- ", "> ", "|", "```", "---", "!["]
        .iter()
        .any(|p| body.starts_with(p))
        || body
            .split_once(". ")
            .is_some_and(|(n, _)| n.chars().all(|c| c.is_ascii_digit()));
    if !body.contains('\n') && !starts_block {
        return Some(body.to_owned());
    }
    let end_in_prose = live
        && spot.after.trim().is_empty()
        && body
            .lines()
            .next_back()
            .is_some_and(|line| line.starts_with('|') && line.ends_with('|'))
        && matches!(markdown::parse(body).blocks.last(), Some(Block::Table(..)));
    Some(spot.block(&converted.text, end_in_prose))
}

/// 跨行替换/删除选区时，修复两端未被完全覆盖的简单强调标记（`*` / `**` / `~~` / `==`），
/// 避免留下残缺语法。
pub fn plan_repair_emphasis(
    rope: &Rope,
    selection: Selection,
    range: Range<usize>,
    replace: &str,
    prose_at: impl Fn(usize) -> Option<bool>,
) -> Option<Transaction> {
    let (first, last) = (rope.char_to_line(range.start), rope.char_to_line(range.end));
    if first == last
        || replace.contains(['\n', '\r'])
        || rope
            .line(first)
            .len_bytes()
            .max(rope.line(last).len_bytes())
            > SCAN_LIMIT
    {
        return None;
    }
    let (mut a, mut b) = (rope.char_to_byte(range.start), rope.char_to_byte(range.end));
    let (left, right) = (rope.line_to_byte(first), rope.line_to_byte(last));
    let (before, after) = (rope.line(first).to_string(), rope.line(last).to_string());
    let start = (prose_at(a) == Some(true))
        .then(|| format::emphasis_boundary(&before, a - left))
        .flatten();
    let end = (prose_at(b) == Some(true))
        .then(|| format::emphasis_boundary(&after, b - right))
        .flatten();
    if start.is_none() && end.is_none() {
        return None;
    }
    if let (Some((_, l)), Some((_, r))) = (&start, &end)
        && (l == r || replace.is_empty())
    {
        return None;
    }
    let mut inserted = String::new();
    if let Some((span, mark)) = start {
        let body = &before[span.start + mark.len()..a - left];
        let kept = body.trim_end();
        if kept.is_empty() {
            a = left + span.start;
        } else {
            a -= body.len() - kept.len();
            inserted.push_str(mark);
        }
        inserted.push_str(&body[kept.len()..]);
    }
    inserted.push_str(replace);
    let caret = inserted.chars().count();
    if let Some((span, mark)) = end {
        let body = &after[b - right..span.end - mark.len()];
        let kept = body.trim_start();
        inserted.push_str(&body[..body.len() - kept.len()]);
        if kept.is_empty() {
            b = right + span.end;
        } else {
            b += body.len() - kept.len();
            inserted.push_str(mark);
        }
    }
    let start_char = rope.byte_to_char(a);
    Some(Transaction::new(
        vec![Change {
            start: start_char,
            removed: rope.byte_slice(a..b).to_string(),
            inserted,
        }],
        selection,
        Selection::point(start_char + caret),
        EditKind::Other,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prose_spot_rejects_inline_code_and_fenced_blocks() {
        let text = "普通 `行内代码` 正文\n\n```rust\nlet x = 1;\n```\n";
        let rope = Rope::from_str(text);
        let parsed = markdown::parse(text);

        let prose_pos = rope.byte_to_char(text.find("正文").unwrap());
        let spot = prose_spot(&rope, Selection::point(prose_pos), Some(&parsed), true)
            .expect("正文位置应允许智能粘贴");
        assert!(spot.blank_above);

        let inline_code = rope.byte_to_char(text.find("行内代码").unwrap());
        assert!(prose_spot(&rope, Selection::point(inline_code), Some(&parsed), true).is_none());

        let fence_pos = rope.byte_to_char(text.find("let x").unwrap());
        assert!(prose_spot(&rope, Selection::point(fence_pos), Some(&parsed), true).is_none());
        assert!(prose_spot(&rope, Selection::point(fence_pos), None, false).is_none());
    }

    #[test]
    fn operation_range_expands_hidden_bold_markers_and_list_prefix() {
        let text = "- **重点**\n";
        let rope = Rope::from_str(text);
        let parsed = markdown::parse(text);
        let from = rope.byte_to_char(text.find("重点").unwrap());
        let to = rope.byte_to_char(text.find("重点").unwrap() + "重点".len());

        // 未展开标记时：覆盖整项可见文字应一并包入 `- ` 与 `**`。
        let expanded = operation_range(&rope, Selection::new(from, to), Some(&parsed), true, None);
        assert_eq!(rope.slice(expanded).to_string(), "- **重点**");

        // 光标正在加粗内（标记已展开）时：仅保留用户选中部分。
        let revealed = Some(text.find("重点").unwrap());
        let exact = operation_range(
            &rope,
            Selection::new(from, to),
            Some(&parsed),
            true,
            revealed,
        );
        assert_eq!(rope.slice(exact).to_string(), "重点");
    }

    #[test]
    fn plan_copy_generates_formatted_html_for_prose_and_plain_for_code_or_markdown() {
        let text = "前 **加粗** 后\n\n```rust\nlet a = 1;\n```\n";
        let rope = Rope::from_str(text);
        let parsed = markdown::parse(text);

        let bold_start = rope.byte_to_char(text.find("加粗").unwrap());
        let bold_end = rope.byte_to_char(text.find("加粗").unwrap() + "加粗".len());
        let sel = Selection::new(bold_start, bold_end);

        let formatted = plan_copy(&rope, sel, Some(&parsed), true, None, false, 1024)
            .expect("应生成富文本复制载荷");
        match formatted {
            CopyPayload::Formatted { html, plain } => {
                assert_eq!(plain, "加粗");
                assert!(html.contains("<strong>加粗</strong>"), "{html}");
            }
            CopyPayload::Plain(p) => panic!("期望 Formatted，实际 Plain({p:?})"),
        }

        let as_md = plan_copy(&rope, sel, Some(&parsed), true, None, true, 1024)
            .expect("复制为 Markdown 应保留源码标记");
        assert_eq!(as_md, CopyPayload::Plain("**加粗**".to_owned()));

        let code_start = rope.byte_to_char(text.find("let a").unwrap());
        let code_end = code_start + 5;
        let code_copy = plan_copy(
            &rope,
            Selection::new(code_start, code_end),
            Some(&parsed),
            true,
            None,
            false,
            1024,
        );
        assert_eq!(code_copy, Some(CopyPayload::Plain("let a".to_owned())));
    }

    #[test]
    fn smart_paste_rewrites_url_tsv_and_html() {
        let rope = Rope::from_str("点击这里");
        let spot = PasteSpot {
            before: String::new(),
            after: String::new(),
            blank_above: true,
        };
        assert_eq!(
            plan_paste_text(
                &rope,
                Selection::new(0, 4),
                &spot,
                "https://example.com",
                true
            ),
            Some("[点击这里](https://example.com)".to_owned())
        );
        assert_eq!(
            plan_paste_text(
                &rope,
                Selection::point(4),
                &spot,
                "https://example.com/a.png",
                true
            ),
            Some("![](https://example.com/a.png)".to_owned())
        );
        let table = plan_paste_text(&rope, Selection::point(0), &spot, "A\tB\n1\t2", true)
            .expect("TSV 应转为 Markdown 表格");
        assert!(table.contains("| A | B |"));

        let html = plan_paste_html(&spot, "<strong>重点</strong>", true, true);
        assert_eq!(html.as_deref(), Some("**重点**"));
    }
}
