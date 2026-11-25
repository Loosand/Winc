//! 可编辑预览中光标所在段落或标题的排版：按样式显示（比例字体、粗体、链接），只显示光标
//! 接触到的元素的 Markdown 标记（`**`、`[文字](网址)` 的括号和网址、`` ` ``、`#`），标记以次要
//! 色排版。单击或移动光标时段落外观保持不变，阅读不受干扰；光标进入粗体后，两侧的 `**` 才显示。
//!
//! 按当前原文解析该段落，不等待后台解析，输入快于解析时结果仍然正确。显示的文字逐字对应原文，
//! 标记同样有原文位置，光标、选区和删除都作用于原文。

use std::collections::BTreeSet;
use std::ops::Range;

use pulldown_cmark::{Event, Parser, Tag};

use super::{Block, Inline, Run, SourceMap, Style, options, parse};

/// 编辑器核心 · 行内标记显隐：从当前段落或标题的原文 `text` 生成可编辑的排版块。
/// 显露标记改变的是显示文本和位置映射，不是文件内容或撤销历史。
///
/// `text` 是叶子块的原文，续行带有引用的 `>` 和列表的缩进；`caret` 是光标相对 `text` 的
/// 字节偏移，为 `None` 时不显示标记；`insert` 是输入法组字，排在光标处并加下划线，没有原文位置。
/// `leading_quote` 表示首行同样带有外层引用的前缀。
/// 对照以 `text` 的开头为准（`Inline::source` 为 0）；原文已不是段落时逐字显示，不带样式。
pub fn reveal(
    text: &str,
    caret: Option<usize>,
    insert: Option<&str>,
    leading_quote: bool,
) -> Block {
    let mut blocks = reveal_blocks(text, caret, insert, leading_quote);
    if blocks.len() == 1
        && let Some(single) = blocks.pop()
    {
        return single;
    }
    let insert = caret.zip(insert);
    let lines = Lines::strip(text, leading_quote);
    let plain = lines.plain.trim_end_matches(['\n', '\r']);
    Block::Paragraph(verbatim(plain, insert, |at| Some(lines.to_text(at))))
}

/// 编辑器核心 · 多块行内标记显隐：与 [`reveal`] 相同，但当实时编辑期间（例如回车拆段、
/// 异步解析尚未追上时）原文已经拆成多个段落或标题，逐块保持富文本排版，仅对光标所在块
/// 显露接触到的语法标记，避免整段退回为未排版的纯文本源码。
pub fn reveal_blocks(
    text: &str,
    caret: Option<usize>,
    insert: Option<&str>,
    leading_quote: bool,
) -> Vec<Block> {
    let insert = caret.zip(insert);
    let lines = Lines::strip(text, leading_quote);
    let plain = &lines.plain;
    // 去掉引用标记后没有内容（只输入了 `>`）：逐字显示原文，光标仍有位置可落。
    if plain.trim().is_empty() && !text.trim().is_empty() {
        let text = text.trim_end_matches(['\n', '\r']);
        return vec![Block::Paragraph(verbatim(text, insert, Some))];
    }
    let parsed = parse(plain);
    if parsed.blocks.is_empty() {
        let text = plain.trim_end_matches(['\n', '\r']);
        return vec![Block::Paragraph(verbatim(text, insert, |at| {
            Some(lines.to_text(at))
        }))];
    }
    if parsed.blocks.len() == 1 {
        return match parsed.blocks.as_slice() {
            [Block::Paragraph(inline)] => {
                let base = parsed.ranges.first().map_or(0, |r| r.start) + inline.source;
                vec![Block::Paragraph(reveal_slice(
                    text,
                    &lines,
                    0..plain.len(),
                    base,
                    inline,
                    caret,
                    insert,
                    leading_quote,
                ))]
            }
            [Block::Heading(level, inline)] => {
                let base = parsed.ranges.first().map_or(0, |r| r.start) + inline.source;
                vec![Block::Heading(
                    *level,
                    reveal_slice(
                        text,
                        &lines,
                        0..plain.len(),
                        base,
                        inline,
                        caret,
                        insert,
                        leading_quote,
                    ),
                )]
            }
            _ => {
                let text = plain.trim_end_matches(['\n', '\r']);
                vec![Block::Paragraph(verbatim(text, insert, |at| {
                    Some(lines.to_text(at))
                }))]
            }
        };
    }

    // 多个块（常见于回车拆段而后台解析尚未追上）：只对光标所在的块显露标记，其余块保持排版。
    let caret_plain = caret.map(|c| lines.to_plain(c));
    let mut blocks = Vec::with_capacity(parsed.blocks.len());
    for (block, range) in parsed.blocks.into_iter().zip(parsed.ranges) {
        let line_limit = match plain[range.end..].find('\n') {
            Some(rel) => range.end + rel,
            None => plain.len(),
        };
        let slice_end = plain[..line_limit].trim_end_matches('\r').len();
        let slice = range.start..slice_end.max(range.start);
        let here = caret_plain.is_some_and(|c| slice.start <= c && c <= slice.end);
        let block_caret = here.then_some(caret).flatten();
        let block_insert = here.then_some(insert).flatten();
        match block {
            Block::Paragraph(inline) => {
                let base = range.start + inline.source;
                blocks.push(Block::Paragraph(reveal_slice(
                    text,
                    &lines,
                    slice,
                    base,
                    &inline,
                    block_caret,
                    block_insert,
                    leading_quote && range.start == 0,
                )));
            }
            Block::Heading(level, inline) => {
                let base = range.start + inline.source;
                blocks.push(Block::Heading(
                    level,
                    reveal_slice(
                        text,
                        &lines,
                        slice,
                        base,
                        &inline,
                        block_caret,
                        block_insert,
                        leading_quote && range.start == 0,
                    ),
                ));
            }
            _ => {
                let raw_part = plain[slice.clone()].trim_end_matches(['\n', '\r']);
                let start = slice.start;
                blocks.push(Block::Paragraph(verbatim(raw_part, block_insert, |at| {
                    Some(lines.to_text(start + at))
                })));
            }
        }
    }
    blocks
}

#[allow(clippy::too_many_arguments)]
fn reveal_slice(
    text: &str,
    lines: &Lines,
    slice: Range<usize>,
    base: usize,
    inline: &Inline,
    caret: Option<usize>,
    insert: Option<(usize, &str)>,
    leading_quote: bool,
) -> Inline {
    let plain = &lines.plain;
    let ranges = caret.map_or_else(Vec::new, |_| {
        elements(&plain[slice.clone()])
            .into_iter()
            .map(|r| slice.start + r.start..slice.start + r.end)
            .collect()
    });
    let elements = ElementIndex::new(&ranges);
    let mut out = Out {
        insert,
        quote: caret.and_then(|at| quote_at(text, at, leading_quote)),
        ..Out::default()
    };
    let caret = caret.map(|c| lines.to_plain(c));
    let anchors = &inline.map.0;
    // 已写入的文字和已处理的原文（相对 `plain`）的末尾位置。
    let (mut t, mut s) = (0, slice.start);
    let emit = |out: &mut Out, gap: Range<usize>, made: Range<usize>| {
        let generated = &inline.text[made.clone()];
        // 由原文生成的文字（emoji、脚注编号）之前的标记单独成为一处：光标接触时显示原文，
        // 不显示生成的文字。
        let own = !generated.trim().is_empty();
        let shown = out.gap(plain, gap, &elements, caret, own, lines);
        let image = generated.contains(super::IMAGE_MARK);
        if shown && image && !out.text.ends_with('\n') {
            // 显示图片的原文时，图片另起一行排在原文下方，使原文与图片都完整可见。
            out.push('\n', Style::default(), None);
        }
        if !(shown && own) || image {
            out.copy(inline, made, None, &elements, caret);
        }
    };
    for a in anchors {
        let (at, src, len) = (a.text as usize, base + a.source as usize, a.len as usize);
        emit(&mut out, s..src.max(s), t..at.max(t));
        t = t.max(at);
        if len > 0 {
            out.copy(inline, at..at + len, Some((src, lines)), &elements, caret);
            t = at + len;
            s = s.max(src + len);
        }
    }
    emit(&mut out, s..slice.end.max(s), t..inline.text.len());
    out.flush();
    Inline {
        text: out.text,
        runs: out.runs,
        links: inline.links.clone(),
        images: inline.images.clone(),
        align: inline.align,
        map: out.map,
        ids: inline.ids.clone(),
        source: 0,
    }
}

/// 在可编辑段落中补齐选区完整覆盖的隐藏标记，返回原文中的字节范围。
pub fn include_hidden_paragraph_markers(
    text: &str,
    selection: Range<usize>,
    revealed_at: Option<usize>,
) -> Range<usize> {
    let lines = Lines::strip(text, false);
    let selected = lines.to_plain(selection.start)..lines.to_plain(selection.end);
    let expanded = super::clipboard::include_hidden_markers(
        &lines.plain,
        selected.clone(),
        revealed_at.map(|at| lines.to_plain(at)),
    );
    let start = if expanded.start < selected.start {
        lines.to_text(expanded.start)
    } else {
        selection.start
    };
    let end = if expanded.end > selected.end {
        lines.to_text(expanded.end)
    } else {
        selection.end
    };
    start..end
}

/// 带标记的行内元素在 `plain` 中的字节范围。
///
/// 包括强调、粗体、删除线、高亮、链接、图片、行内代码、公式、HTML 标签和脚注引用。
fn elements(plain: &str) -> Vec<Range<usize>> {
    let mut elements = Vec::new();
    // 行内代码和公式里的 `==` 是文字，不高亮。
    let mut code = Vec::new();
    for (event, range) in Parser::new_ext(plain, options()).into_offset_iter() {
        match &event {
            Event::Code(_) | Event::InlineMath(_) | Event::DisplayMath(_) => {
                code.push(range.clone());
            }
            _ => {}
        }
        if matches!(
            &event,
            Event::Start(
                Tag::Emphasis
                    | Tag::Strong
                    | Tag::Strikethrough
                    | Tag::Link { .. }
                    | Tag::Image { .. }
            ) | Event::Code(_)
                | Event::InlineMath(_)
                | Event::DisplayMath(_)
                | Event::InlineHtml(_)
                | Event::FootnoteReference(_)
        ) {
            elements.push(range);
        }
    }
    code.sort_unstable_by_key(|range| range.end);
    elements.extend(
        super::mark::pairs(plain, |at| {
            let i = code.partition_point(|range| range.end <= at);
            code.get(i).is_some_and(|range| range.contains(&at)) || super::mark::escaped(plain, at)
        })
        .into_iter()
        .map(|[open, close]| open.start..close.end),
    );
    elements
}

/// 原文字节对应的最内层元素只在起止边界变化。保留同样长时先遇到的元素。
struct ElementIndex<'a>(Vec<(usize, Option<&'a Range<usize>>)>);

impl<'a> ElementIndex<'a> {
    fn new(ranges: &'a [Range<usize>]) -> Self {
        let mut boundaries: Vec<_> = ranges
            .iter()
            .enumerate()
            .filter(|(_, range)| !range.is_empty())
            .flat_map(|(i, range)| [(range.start, true, i), (range.end, false, i)])
            .collect();
        boundaries.sort_unstable();
        let mut open = BTreeSet::new();
        let mut innermost = Vec::with_capacity(boundaries.len());
        for (at, start, i) in boundaries {
            let entry = (ranges[i].len(), i);
            if start {
                open.insert(entry);
            } else {
                open.remove(&entry);
            }
            innermost.push((at, open.first().map(|&(_, i)| &ranges[i])));
        }
        Self(innermost)
    }

    fn at(&self, at: usize) -> Option<&Range<usize>> {
        let i = self.0.partition_point(|(start, _)| *start <= at);
        self.0.get(i.checked_sub(1)?)?.1
    }
}

/// 返回光标是否接触 `range`：位于其中或紧贴其两端。
fn touches(range: &Range<usize>, caret: Option<usize>) -> bool {
    caret.is_some_and(|c| range.start <= c && c <= range.end)
}

/// An existing quote prefix is editable from its first marker through the start of its text.
fn quote_at(text: &str, caret: usize, leading_quote: bool) -> Option<(&str, usize)> {
    let line = text.get(..caret)?.rfind('\n').map_or(0, |end| end + 1);
    if line == 0 && !leading_quote {
        return None;
    }
    let content = text[line..].split('\n').next()?;
    let prefix = content.trim_start_matches([' ', '\t']);
    if !prefix.starts_with('>') {
        return None;
    }
    let start = line + content.len() - prefix.len();
    let end = prefix
        .find(|c| !matches!(c, '>' | ' ' | '\t'))
        .unwrap_or(prefix.len());
    (caret <= start + end).then_some((&prefix[..end], start))
}

/// 去掉续行开头引用 `>` 和缩进后的原文，以及它与原文的对应关系。
struct Lines {
    plain: String,
    /// 各行在 `plain` 和原文中的起点（字节偏移）。
    starts: Vec<(usize, usize)>,
}

impl Lines {
    /// 返回去掉续行开头 `>` 和缩进后的文字，并记下各行在两个字符串中的起点。
    fn strip(text: &str, leading_quote: bool) -> Lines {
        let mut plain = String::with_capacity(text.len());
        let mut starts = Vec::new();
        let mut start = 0;
        for (i, line) in text.split('\n').enumerate() {
            // 段落续行开头的 `>` 只能视为引用标记，否则该行将开始一个新的引用。
            if i > 0 {
                plain.push('\n');
            }
            let skip = if i == 0 && !leading_quote {
                0
            } else {
                line.len() - line.trim_start_matches([' ', '\t', '>']).len()
            };
            starts.push((plain.len(), start + skip));
            plain.push_str(&line[skip..]);
            start += line.len() + 1;
        }
        Lines { plain, starts }
    }

    /// 返回 `plain` 中的位置在原文中的位置；行间的换行对应原文的换行。
    fn to_text(&self, at: usize) -> usize {
        let i = self
            .starts
            .partition_point(|&(p, _)| p <= at)
            .saturating_sub(1);
        let (p, t) = self.starts[i];
        t + (at - p)
    }

    /// 返回原文中的位置在 `plain` 中的位置；位于已去掉的行首标记中时取该行的开头。
    fn to_plain(&self, at: usize) -> usize {
        let i = self.starts.partition_point(|&(_, t)| t <= at);
        let Some(line) = i.checked_sub(1) else {
            return 0;
        };
        let (p, t) = self.starts[line];
        let next = self.starts.get(i).map_or(usize::MAX, |&(p, _)| p);
        (p + (at - t)).min(next)
    }
}

/// 逐字显示时也在原文位置插入组字。
fn verbatim(
    text: &str,
    insert: Option<(usize, &str)>,
    source: impl Fn(usize) -> Option<usize>,
) -> Inline {
    let mut out = Out {
        insert,
        ..Out::default()
    };
    for (i, c) in text.char_indices() {
        if c != '\r' {
            out.push(c, Style::default(), source(i));
        }
    }
    out.flush();
    Inline {
        text: out.text,
        runs: out.runs,
        map: out.map,
        ..Inline::default()
    }
}

/// 拼接生成的文字。
#[derive(Default)]
struct Out<'a> {
    text: String,
    runs: Vec<Run>,
    map: SourceMap,
    /// 尚未写入的输入法组字及其在原文中的位置；写入到原文位置不小于该值的第一个字符之前。
    insert: Option<(usize, &'a str)>,
    quote: Option<(&'a str, usize)>,
}

impl Out<'_> {
    fn push(&mut self, c: char, style: Style, source: Option<usize>) {
        if let Some((quote, start)) = self.quote
            && source.is_some_and(|source| source >= start + quote.len())
        {
            self.quote = None;
            let style = Style {
                flags: Style::SYNTAX,
                link: 0,
            };
            for (offset, c) in quote.char_indices() {
                self.push(c, style, Some(start + offset));
            }
        }
        if source.is_some_and(|s| self.insert.as_ref().is_some_and(|(at, _)| s >= *at)) {
            self.flush();
        }
        let at = self.text.len();
        self.text.push(c);
        match self.runs.last_mut() {
            Some(run) if run.style == style => run.end = self.text.len(),
            _ => self.runs.push(Run {
                end: self.text.len(),
                style,
            }),
        }
        if let Some(source) = source {
            self.map.push(at, source, c.len_utf8());
        }
    }

    /// 写入输入法组字（加下划线，没有原文位置）。
    fn flush(&mut self) {
        if let Some((_, text)) = self.insert.take() {
            let style = Style {
                flags: Style::UNDERLINE,
                link: 0,
            };
            for c in text.chars() {
                self.push(c, style, None);
            }
        }
    }

    /// 写入原有的文字 `range`（样式不变）；`source` 是 `range` 在 `plain` 中的起点及对照。
    ///
    /// 由原文生成的文字没有起点；光标接触到的公式显示 LaTeX 原文。
    fn copy(
        &mut self,
        inline: &Inline,
        range: Range<usize>,
        source: Option<(usize, &Lines)>,
        elements: &ElementIndex<'_>,
        caret: Option<usize>,
    ) {
        if range.is_empty() {
            return;
        }
        let first = inline.runs.partition_point(|run| run.end <= range.start);
        let mut runs = inline.runs[first..].iter().peekable();
        for (i, c) in inline.text[range.clone()].char_indices() {
            let at = range.start + i;
            while runs.peek().is_some_and(|run| run.end <= at) {
                runs.next();
            }
            let mut style = runs.peek().map_or(Style::default(), |run| run.style);
            let plain = source.map(|(s, _)| s + i);
            if style.has(Style::MATH)
                && plain
                    .and_then(|p| elements.at(p))
                    .is_some_and(|e| touches(e, caret))
            {
                style.flags = (style.flags & !Style::MATH) | Style::CODE;
            }
            self.push(c, style, source.map(|(s, lines)| lines.to_text(s + i)));
        }
    }

    /// 处理原文中未显示为文字的一段 `gap`（相对 `plain`），返回其中是否显示了内容。
    ///
    /// 光标接触的元素显示其标记，其余标记和换行不显示（软换行生成的空格已在文字中）。`own` 为真时，
    /// 不属于任何元素的标记单独成为一处（emoji 短码）；否则按行拆分，每段单独成为一处
    /// （标题的 `#`、转义的 `\`）。
    fn gap(
        &mut self,
        plain: &str,
        gap: Range<usize>,
        elements: &ElementIndex<'_>,
        caret: Option<usize>,
        own: bool,
        lines: &Lines,
    ) -> bool {
        if caret.is_none() {
            return false;
        }
        let mut shown = false;
        let text = &plain[gap.clone()];
        // 不属于任何元素的一段，截止到换行或相邻元素的边界。
        let piece = |i: usize| {
            if own {
                return 0..text.len();
            }
            let line_start = text[..i].rfind('\n').map_or(0, |n| n + 1);
            let from = text[line_start..i]
                .char_indices()
                .rev()
                .find_map(|(off, ch)| {
                    elements
                        .at(gap.start + line_start + off)
                        .map(|_| line_start + off + ch.len_utf8())
                })
                .unwrap_or(line_start);
            let line_end = text[i..].find('\n').map_or(text.len(), |n| i + n);
            let to = text[i..line_end]
                .char_indices()
                .find_map(|(off, _)| elements.at(gap.start + i + off).map(|_| i + off))
                .unwrap_or(line_end);
            from..to
        };
        for (i, c) in text.char_indices() {
            if matches!(c, '\n' | '\r') {
                continue;
            }
            let at = gap.start + i;
            let (show, style) = match elements.at(at) {
                Some(e) => (
                    touches(e, caret),
                    Style {
                        flags: Style::SYNTAX,
                        link: 0,
                    },
                ),
                None => {
                    let p = piece(i);
                    let p = gap.start + p.start..gap.start + p.end;
                    if plain[p.clone()].trim().is_empty() {
                        // 光标接触空白段（含段落首尾空格与多行段落行尾空格）时按普通文字保留，
                        // 确保键入空格、移动光标与拖选空格都有真实宽度。
                        (touches(&p, caret), Style::default())
                    } else {
                        (
                            touches(&p, caret),
                            Style {
                                flags: Style::SYNTAX,
                                link: 0,
                            },
                        )
                    }
                }
            };
            if show {
                self.push(c, style, Some(lines.to_text(at)));
                shown = true;
            }
        }
        shown
    }
}

#[cfg(test)]
mod tests;
