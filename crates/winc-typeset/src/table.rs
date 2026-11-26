//! 表格的排版：列宽的分配、单元格的折行和对齐。

use super::*;

impl Cx<'_, '_> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn table(
        &mut self,
        aligns: &[Align],
        rows: &[Vec<Inline>],
        head: bool,
        x: f32,
        y: f32,
        width: f32,
        env: Env,
        record_patch: bool,
        out: &mut Out,
    ) -> Flow {
        // 表格的字号比正文小一级（正文 16 时为 14），表格内信息密度更高；内边距和行高随之缩放。
        let base = (self.base * TABLE_TEXT).round();
        let first_item = out.items.len();
        let (pad_x, pad_y) = ((base * 0.8125).round(), (base * 0.5).round());
        let cols = rows
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(0)
            .max(aligns.len());
        if cols == 0 {
            return Flow {
                height: 0.0,
                first_baseline: None,
            };
        }
        let para = |header: bool, col: usize| Para {
            size: base,
            bold: header,
            color: env.color,
            line: 1.5,
            align: aligns.get(col).copied().unwrap_or(Align::None),
            keep_spaces: false,
        };
        let live = self.caret_cell();
        // 输入领先于异步解析时，将位于当前编辑单元格之后的静态单元格源码起点按当前长度差平移，
        // 避免刚输入的多字节字符（如中文）与下一列未平移的旧起点撞车导致光标跳列。
        let live_delta = self.raw.as_ref().map_or(0isize, |raw| {
            raw.leaf.end as isize - raw.parsed_end as isize
        });
        let (min, max, col_bounds) =
            self.column_bounds_cached(rows, cols, &live, pad_x, |r, c| para(head && r == 0, c));
        let widths = column_widths(&min, &max, width);
        let total: f32 = widths.iter().sum::<f32>() + 1.0;
        let record = record_patch
            && rows.len() >= block::HUGE_TABLE_ROWS
            && out.items.is_empty()
            && col_bounds.is_some();
        let mut row_entries = if record {
            Vec::with_capacity(rows.len())
        } else {
            Vec::new()
        };
        let mut top = y;
        let mut first_baseline = None;
        let mut lines = vec![top];
        for (r, row) in rows.iter().enumerate() {
            let item_start = out.items.len() as u32;
            let tb_start = self.text.len();
            let ch_start = self.chars;
            let (row_h, row_baseline) = self.table_row(
                r,
                row,
                head,
                aligns,
                &widths,
                total,
                x,
                top,
                base,
                pad_x,
                pad_y,
                env,
                &live,
                live_delta,
                &mut out.items,
            );
            if r == 0 {
                first_baseline = row_baseline;
            }
            if record {
                let fallback_src = row_entries
                    .last()
                    .map_or(0, |e: &block::TableRowEntry| e.source_start);
                let src = row
                    .iter()
                    .find(|cell| !cell.map.is_empty())
                    .map_or(fallback_src, |cell| cell.source);
                row_entries.push(block::TableRowEntry {
                    items: item_start..out.items.len() as u32,
                    text_bytes: tb_start..self.text.len(),
                    chars: ch_start..self.chars,
                    top,
                    height: row_h,
                    source_start: src,
                    content_hash: block::table_row_content_hash(row),
                });
            }
            top += row_h;
            lines.push(top);
        }
        let lines_start = out.items.len() as u32;
        // 边框：圆角的外框，行之间一道横线；列之间不画竖线，按单元格的内边距区分各列。
        for &line_y in &lines[1..lines.len() - 1] {
            out.items.push(Item::Fill {
                rect: Rect::new(x + 1.0, line_y, total - 2.0, 1.0),
                color: self.colors.table_line,
                radius: 0.0,
            });
        }
        out.items.push(Item::path(
            rounded_frame(Rect::new(x, y, total, top - y + 1.0), TABLE_RADIUS),
            1.0,
            self.colors.table_frame,
        ));
        out.items.push(Item::Table {
            rect: Rect::new(x, y, total, top - y + 1.0),
            align: live
                .as_ref()
                .map(|(_, c, ..)| aligns.get(*c).copied().unwrap_or_default()),
            cols: widths.clone().into(),
            rows: lines.windows(2).map(|w| w[1] - w[0]).collect(),
        });
        let mut height = top - y + 1.0;
        // 比正文宽：记下表格的各项，绘制时横向平移；下方留出滚动条的位置。
        if total > width + 0.5 {
            height += SCROLL_BAR_ROOM;
            out.scrolls.push(Scroller {
                items: u32::try_from(first_item).unwrap_or(u32::MAX)
                    ..u32::try_from(out.items.len()).unwrap_or(u32::MAX),
                rect: Rect::new(x, y, width, height),
                content: total,
            });
        }
        if record && let Some(col_bounds) = col_bounds {
            out.patch = Some(Box::new(block::PatchIndex::Table(block::TablePatch {
                head,
                aligns: aligns.into(),
                width,
                base: self.base,
                color: env.color,
                widths,
                total,
                pad_x,
                pad_y,
                live_cell: live.as_ref().map(|(r, c, ..)| (*r, *c)),
                live_delta,
                col_bounds,
                row_entries,
                lines_start,
            })));
        }
        Flow {
            height,
            first_baseline,
        }
    }

    /// 排版表格中的单行，返回该行总高度与第一行基线（用于整表排版与超大表原位单行修补复用）。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn table_row(
        &mut self,
        r: usize,
        row: &[Inline],
        head: bool,
        aligns: &[Align],
        widths: &[f32],
        total: f32,
        x: f32,
        top: f32,
        base: f32,
        pad_x: f32,
        pad_y: f32,
        env: Env,
        live: &Option<CaretCell>,
        live_delta: isize,
        out: &mut Vec<Item>,
    ) -> (f32, Option<f32>) {
        let para = |header: bool, col: usize| Para {
            size: base,
            bold: header,
            color: env.color,
            line: 1.5,
            align: aligns.get(col).copied().unwrap_or(Align::None),
            keep_spaces: false,
        };
        let mono = base * 0.85;
        let raw_row = |room: f32, cx: &mut Self| {
            let m = cx.frame.metrics(FontKind::Mono, mono);
            Row {
                room: room.max(mono),
                size: mono,
                line_h: (base * 1.5).round(),
                ascent: m.ascent,
                descent: m.descent,
            }
        };
        let start = out.len();
        let mut cell_x = x;
        let mut row_h: f32 = 0.0;
        let mut first_baseline = None;
        for (c, w) in widths.iter().enumerate() {
            if let Some(cell) = row.get(c) {
                let is_live = live
                    .as_ref()
                    .is_some_and(|(lr, lc, ..)| (*lr, *lc) == (r, c));
                let after_live = live
                    .as_ref()
                    .is_some_and(|(lr, lc, ..)| (r, c) > (*lr, *lc));
                let shift = |src: usize| {
                    if after_live && live_delta != 0 {
                        src.saturating_add_signed(live_delta)
                    } else {
                        src
                    }
                };
                let (cell, cell_source) = match live {
                    Some((lr, lc, source, None)) if (*lr, *lc) == (r, c) => (Err(*source), 0),
                    Some((lr, lc, _, Some(live))) if (*lr, *lc) == (r, c) => {
                        (Ok(live), u32::try_from(live.source).unwrap_or(0))
                    }
                    // 空单元格同样保留一段空的原文，使其可点击，光标也有绘制位置。
                    _ if cell.text.is_empty() => (Err(shift(cell.source)), 0),
                    _ => (Ok(cell), u32::try_from(shift(cell.source)).unwrap_or(0)),
                };
                let flow = match cell {
                    Err(source) => {
                        let row = raw_row(w - pad_x * 2.0, self);
                        let source = u32::try_from(source).unwrap_or(0);
                        let mut state = winc_syntax::LineState::default();
                        let piece = self.code_piece("", source, "markdown", None, &mut state, row);
                        self.paste(&piece, cell_x + pad_x, top + pad_y, source, out);
                        Flow {
                            height: piece.height,
                            first_baseline: piece.first_baseline.map(|b| top + pad_y + b),
                        }
                    }
                    Ok(cell) => self.flow_at_source(
                        cell,
                        cell_source,
                        cell_x + pad_x,
                        top + pad_y,
                        w - pad_x * 2.0,
                        Para {
                            keep_spaces: is_live,
                            ..para(head && r == 0, c)
                        },
                        out,
                    ),
                };
                row_h = row_h.max(flow.height);
                if r == 0 {
                    first_baseline = first_baseline.or(flow.first_baseline);
                }
            }
            cell_x += w;
        }
        let row_h = row_h.max(base * 1.5) + pad_y * 2.0;
        // 表头铺底色（上方两角与外框同为圆角），与正文各行区分；正文各行之间只有横线。
        if head && r == 0 {
            out.insert(
                start,
                Item::Corners {
                    rect: Rect::new(x, top, total, row_h),
                    color: self.colors.table_head,
                    radii: [TABLE_RADIUS, TABLE_RADIUS, 0.0, 0.0],
                },
            );
        }
        (row_h, first_baseline)
    }

    /// 取走并返回光标所在的单元格：（行，列，内容在原文中的起点，按排版显示的内容）。
    ///
    /// 可编辑的预览中该单元格只显示光标接触到的标记，规则同正文；空单元格的内容为 `None`，
    /// 排版时保留一段空的原文，使光标有绘制位置。光标不在这张表中时返回 `None`。
    pub(crate) fn caret_cell(&mut self) -> Option<CaretCell> {
        let (r, c, range) = self.cell.take()?;
        let raw = self.raw.as_ref()?;
        let segment = raw.block.get(range.clone())?;
        let trimmed = segment.trim();
        let rel_caret = raw.caret.saturating_sub(range.start).min(segment.len());
        // 空单元格的原文位置与解析结果一致（左侧 `|` 之后一个空格处），光标进出时不跳动；
        // 有内容时保留光标所及的首尾空格，使在单元格末尾或开头输入空格都能实时推动光标。
        let (start, content) = if trimmed.is_empty() {
            let start = (range.start + 1).min(range.end);
            (start, "")
        } else {
            let lead = segment.len() - segment.trim_start().len();
            let trail = segment.trim_end().len();
            let start_rel = lead.min(rel_caret);
            let end_rel = trail.max(rel_caret);
            (range.start + start_rel, &segment[start_rel..end_rel])
        };
        let cell = (!content.is_empty()).then(|| {
            let caret = raw.caret.saturating_sub(start).min(content.len());
            let preedit = raw.preedit.as_deref();
            let mut block =
                winc_preview::markdown::reveal::reveal(content, Some(caret), preedit, false);
            let (Block::Paragraph(inline) | Block::Heading(_, inline)) = &mut block else {
                unreachable!("reveal 只排出段落、标题");
            };
            inline.source = start;
            std::mem::take(inline)
        });
        self.preedit |= cell.is_some() && raw.preedit.is_some();
        Some((r, c, start, cell))
    }

    /// 返回 `cols` 列各自最窄的宽度和不换行时的宽度（逻辑像素），以及大表格的列宽极值摘要。
    pub(crate) fn column_bounds_cached(
        &mut self,
        rows: &[Vec<Inline>],
        cols: usize,
        live: &Option<CaretCell>,
        pad_x: f32,
        para: impl Fn(usize, usize) -> Para,
    ) -> (Vec<f32>, Vec<f32>, Option<Arc<memo::TableCols>>) {
        if rows.len() >= 32 {
            let cache_key = memo::key(|h| {
                (rows.as_ptr() as usize).hash(h);
                rows.len().hash(h);
                cols.hash(h);
                pad_x.to_bits().hash(h);
                para(0, 0).size.to_bits().hash(h);
                para(0, 0).bold.hash(h);
                // 混合首尾行内容，防止内存地址复用误命中不同表格。
                rows.first().hash(h);
                rows.last().hash(h);
            });
            let cached = match self.memo.get_table_cols(cache_key) {
                Some(hit) => hit,
                None => {
                    let mut min = vec![(0.0f32, usize::MAX, 0.0f32); cols];
                    let mut max = vec![(0.0f32, usize::MAX, 0.0f32); cols];
                    for (r, row) in rows.iter().enumerate() {
                        for (c, cell) in row.iter().enumerate() {
                            let (lo, hi) = self.measure(cell, para(r, c));
                            let (lo, hi) = (lo + pad_x * 2.0, hi + pad_x * 2.0);
                            if lo >= min[c].0 {
                                min[c] = (lo, r, min[c].0);
                            } else if lo > min[c].2 {
                                min[c].2 = lo;
                            }
                            if hi >= max[c].0 {
                                max[c] = (hi, r, max[c].0);
                            } else if hi > max[c].2 {
                                max[c].2 = hi;
                            }
                        }
                    }
                    let built = Arc::new(memo::TableCols { min, max });
                    self.memo.insert_table_cols(cache_key, Arc::clone(&built));
                    built
                }
            };
            let (min, max) = self.apply_live_col_bounds(&cached, cols, live, pad_x, para);
            return (min, max, Some(cached));
        }
        let (mut min, mut max) = (vec![0f32; cols], vec![0f32; cols]);
        for (r, row) in rows.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                let cell = match live {
                    Some((lr, lc, _, live)) if (*lr, *lc) == (r, c) => live.as_ref(),
                    _ => Some(cell),
                };
                let (lo, hi) = match cell {
                    Some(cell) => self.measure(cell, para(r, c)),
                    None => (0.0, 0.0),
                };
                min[c] = min[c].max(lo + pad_x * 2.0);
                max[c] = max[c].max(hi + pad_x * 2.0);
            }
        }
        (min, max, None)
    }

    /// 结合大表格已缓存的各列极值与当前编辑单元格 `live`，O(1) 得出精确的 `(min, max)`。
    pub(crate) fn apply_live_col_bounds(
        &mut self,
        cached: &memo::TableCols,
        cols: usize,
        live: &Option<CaretCell>,
        pad_x: f32,
        para: impl Fn(usize, usize) -> Para,
    ) -> (Vec<f32>, Vec<f32>) {
        let mut min: Vec<f32> = cached.min.iter().map(|&(m1, _, _)| m1).collect();
        let mut max: Vec<f32> = cached.max.iter().map(|&(m1, _, _)| m1).collect();
        if let Some((lr, lc, _, live_cell)) = live
            && *lc < cols
        {
            let (lo, hi) = match live_cell {
                Some(cell) => self.measure(cell, para(*lr, *lc)),
                None => (0.0, 0.0),
            };
            let (lo, hi) = (lo + pad_x * 2.0, hi + pad_x * 2.0);
            let base_min = if cached.min[*lc].1 == *lr {
                cached.min[*lc].2
            } else {
                cached.min[*lc].0
            };
            let base_max = if cached.max[*lc].1 == *lr {
                cached.max[*lc].2
            } else {
                cached.max[*lc].0
            };
            min[*lc] = base_min.max(lo);
            max[*lc] = base_max.max(hi);
        }
        (min, max)
    }
}

/// 光标所在的单元格：（行，列，内容在原文中的起点，按排版显示的内容）。
type CaretCell = (usize, usize, usize, Option<Inline>);

/// 把宽度 `width` 分给各列，返回各列的宽度（逻辑像素）。
///
/// `min`、`max` 是各列最窄的宽度和不换行时的宽度。放得下时各列取不换行的宽度；放不下时每列
/// 最宽为 `width`，更长的内容在列内换行，表格超出的部分横向滚动，不把各列压窄到逐字换行。
/// 按该规则排出的表格比 `width` 窄时按比例放宽各列。
pub(crate) fn column_widths(min: &[f32], max: &[f32], width: f32) -> Vec<f32> {
    let (sum_min, sum_max): (f32, f32) = (min.iter().sum(), max.iter().sum());
    // 向上取整：向下取整会少零点几像素，最后一个字放不下而换行。
    let capped: Vec<f32> = (min.iter().zip(max))
        .map(|(lo, hi)| hi.min(width.max(*lo)).ceil())
        .collect();
    if sum_max <= width {
        max.to_vec()
    } else if capped.iter().sum::<f32>() > width || sum_min >= width {
        capped
    } else {
        let t = (width - sum_min) / (sum_max - sum_min);
        (min.iter().zip(max))
            .map(|(lo, hi)| (lo + (hi - lo) * t).floor())
            .collect()
    }
}
