//! Markdown 源码位置与显示行、光标坐标的对应。

use super::{App, CachedCaret, Laid, Point, line_box};

impl Laid {
    /// 查找原文字节所属的显示块。脚注的显示顺序与原文不同。
    pub(crate) fn block_at_source(&self, source: usize) -> Option<usize> {
        self.ranges
            .iter()
            .rposition(|range| range.contains(&source))
            .or_else(|| self.ranges.iter().rposition(|range| range.end == source))
    }

    /// Position and hit height of an explicit blank row.
    pub(crate) fn blank_point(&self, source: usize) -> Option<Point> {
        let i = self
            .gaps
            .partition_point(|gap| self.blank_offsets[gap.offsets.start] <= source)
            .checked_sub(1)?;
        let gap = &self.gaps[i];
        let (end, starts) = self.blank_offsets[gap.offsets.clone()].split_last()?;
        if source >= *end {
            return None;
        }
        let row = starts.partition_point(|&s| s <= source) - 1;
        Some((0.0, self.pad + gap.top + row as f32 * gap.row, gap.row))
    }

    /// Whether a byte begins an actual glyph in the visible source mapping.
    pub(in crate::app::preview::markdown) fn shows_source(&self, source: usize) -> bool {
        let Some(block) = self
            .ranges
            .iter()
            .rposition(|range| range.contains(&source))
        else {
            return false;
        };
        let source = source - self.ranges[block].start;
        self.spans(block..block + 1).any(|(.., span, _, text)| {
            span.char_at_source(text, source).is_some_and(|at| {
                at < span.advances.len() && span.source_of(text, at) == Some(source)
            })
        })
    }

    /// An arrow's normal line-height step may cross a shorter blank row. Stop on its
    /// centre first instead of moving directly to the next text row.
    pub(in crate::app::preview::markdown) fn limit_blank_step(
        &self,
        from: f32,
        target: f32,
    ) -> f32 {
        let down = target > from;
        let at = from - self.pad + if down { 0.5 } else { -0.5 };
        let gap = if down {
            self.gaps.get(
                self.gaps
                    .partition_point(|gap| gap.bottom() - gap.row / 2.0 <= at),
            )
        } else {
            self.gaps
                .partition_point(|gap| gap.top + gap.row / 2.0 < at)
                .checked_sub(1)
                .and_then(|i| self.gaps.get(i))
        };
        let Some(gap) = gap else {
            return target;
        };
        let row = (at - gap.top) / gap.row - 0.5;
        let row = if down {
            row.floor() + 1.0
        } else {
            row.ceil() - 1.0
        };
        let row = row.clamp(0.0, (gap.offsets.len() - 2) as f32);
        let center = self.pad + gap.top + (row + 0.5) * gap.row;
        if down {
            target.min(center)
        } else {
            target.max(center)
        }
    }

    /// 原文第 `source` 个字节按这一帧的光标归属显示的位置。
    pub(crate) fn caret_point(&self, source: usize) -> Option<Point> {
        self.caret_point_with_affinity(source, self.drawn_upstream == Some(source))
    }

    /// 按指定的折行归属定位。`upstream` 为真时，优先取上一显示行末尾。
    pub(crate) fn caret_point_with_affinity(&self, source: usize, upstream: bool) -> Option<Point> {
        if let Some(cached) = self.caret.get()
            && cached.source == source
            && cached.upstream == upstream
        {
            return cached.point;
        }
        let point = self.point_of_with_affinity(source, upstream);
        self.caret.set(Some(CachedCaret {
            source,
            upstream,
            point,
        }));
        point
    }

    fn point_of_with_affinity(&self, source: usize, upstream: bool) -> Option<Point> {
        if let Some(point) = self.blank_point(source) {
            return Some(point);
        }
        // 脚注排在最后，原文范围不按先后，从后往前找第一个包含它的块。
        let block = self.block_at_source(source)?;
        let source = source - self.ranges[block].start;
        // 光标应画在上一显示行的末尾：取原文位置在它之前的最后一段，该段正好在此结束时用其末尾。
        if upstream
            && let Some((top, x, baseline, size, span, text)) = self
                .spans(block..block + 1)
                .filter(|(.., span, _, _)| span.source.is_some_and(|s| (s as usize) < source))
                .max_by_key(|(.., span, _, _)| span.source)
                .map(|(top, _, x, baseline, size, span, _, text)| {
                    (top, x, baseline, size, span, text)
                })
            && span.char_at_source(text, source) == Some(span.advances.len())
        {
            let x = x + span.advances.iter().sum::<f32>();
            let (y, h) = line_box(top + baseline, size);
            return Some((x, self.pad + y, h));
        }
        // 先按每段第一个字找（每段只看一次），再在找到的那段里逐字找：长列表、长代码块是一个
        // 顶层块，每帧都要找，不逐字看整块。
        let (top, x, baseline, size, span, text) = self
            .spans(block..block + 1)
            .filter(|(.., span, _, _)| span.source.is_some_and(|s| s as usize <= source))
            .max_by_key(|(.., span, _, _)| span.source)
            .map(|(top, _, x, baseline, size, span, _, text)| {
                (top, x, baseline, size, span, text)
            })?;
        let k = span.char_at_source(text, source)?;
        let x = x + span.advances[..k].iter().sum::<f32>();
        let (y, h) = line_box(top + baseline, size);
        Some((x, self.pad + y, h))
    }
}

/// 可编辑的预览左侧写行号的各显示行：原文行号（从 0 起）和显示行的基线（相对内容顶部）。
///
/// 只看上下范围与 `view`（相对内容顶部）相交的顶层块。显示行从原文一行的开头排起（之前只有
/// `#`、`>`、列表记号、`|` 这类标记）时才写行号：段落里几行原文连成一段文字，折行和连上的
/// 各行不对应显示行，只写段落第一行的行号。`rope` 是排版所依据的原文。
pub(crate) fn source_lines(
    laid: &Laid,
    rope: &ropey::Rope,
    view: std::ops::Range<f32>,
) -> Vec<(usize, f32)> {
    let first = laid
        .tops
        .partition_point(|&t| t <= view.start)
        .saturating_sub(1);
    let last = laid.tops.partition_point(|&t| t < view.end);
    // 每个显示行最左边的一段：（基线，横坐标，在原文中的位置）。
    let mut rows: Vec<(f32, f32, usize)> = Vec::new();
    for (top, _, x, baseline, _, span, block, _) in laid.spans(first..last) {
        let (Some(start), Some(range)) = (span.source, laid.ranges.get(block)) else {
            continue;
        };
        let y = top + baseline;
        if (view.start..=view.end).contains(&y) {
            rows.push((y, x, (range.start + start as usize).min(rope.len_bytes())));
        }
    }
    rows.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    rows.dedup_by(|b, a| (a.0 - b.0).abs() < 0.5);
    rows.into_iter()
        .filter_map(|(y, _, from)| {
            // 解析之后又输入了文字时，记下的位置可能落在多字节字符中间：先对齐到字符边界。
            let from = rope.char_to_byte(rope.byte_to_char(from));
            let line = rope.byte_to_line(from);
            let prefix = rope.byte_slice(rope.line_to_byte(line)..from);
            prefix.chars().all(is_markup).then_some((line, y))
        })
        .collect()
}

/// `c` 是否可以出现在一行原文开头、正文之前的标记中：缩进、标题的 `#`、引用的 `>`、列表的
/// 记号和序号、任务框、表格的 `|`。
fn is_markup(c: char) -> bool {
    c.is_ascii_digit()
        || matches!(
            c,
            ' ' | '\t' | '#' | '>' | '-' | '*' | '+' | '.' | ')' | '|' | '[' | ']' | 'x' | 'X'
        )
}

/// 原文第 `source` 个字节在预览上的位置：左边缘（相对内容左边）、所在那一行铺底色的上边缘
/// （和滚动位置同一个量法）和高度；落在没有文字的块（分隔线、图）里时为 `None`。
///
/// 取原文位置不超过 `source` 的最后一个字之前；落在标记里（`**`、链接地址）时就是标记前面
/// 那个字之后。
pub(crate) fn point_of(laid: &Laid, source: usize) -> Option<Point> {
    laid.point_of_with_affinity(source, laid.drawn_upstream == Some(source))
}

/// 返回原文位置 `source` 处（取法同 [`point_of`]）文字的字体、模拟粗斜体和字号。
///
/// 输入法组字的文字按此样式绘制。
pub(crate) fn font_at(
    laid: &Laid,
    source: usize,
) -> Option<(winc_render::FontKind, winc_render::Faux, f32)> {
    let block = laid.block_at_source(source)?;
    let source = source - laid.ranges[block].start;
    laid.blocks[block].1.font_at_source(source)
}

/// 整份文字里第 `offset` 个字符所在那一行的上边缘（和滚动位置同一个量法）。
pub(crate) fn offset_top(app: &App, offset: usize) -> Option<f32> {
    let laid = app.preview.markdown.layout.as_ref()?;
    let i = laid
        .starts
        .partition_point(|&s| s <= offset)
        .saturating_sub(1);
    let (_, block) = laid.blocks.get(i)?;
    // 要画的项已经释放的块按字符在块中的比例估计，滚动到附近后重新排版。
    if block.is_shed() {
        let part = (offset - laid.starts[i]) as f32 / block.chars.max(1) as f32;
        return Some(laid.pad + laid.tops[i] + block.height * part.min(1.0));
    }
    laid.spans(i..i + 1)
        .find(|(_, start, _, _, _, span, _, _)| offset < start + span.advances.len())
        .map(|(top, _, _, baseline, size, ..)| laid.pad + line_box(top + baseline, size).0)
}
