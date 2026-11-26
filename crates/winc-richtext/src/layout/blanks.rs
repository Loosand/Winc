//! 块间与文末可编辑空行槽位的几何存储、命中测试、光标与选区投影。

use std::ops::Range;
use std::sync::Arc;

use winc_preview::markdown::BlankRows;
use winc_render::Rect;

use super::types::{Point, line_box};

/// 一段连续可编辑空行，按行高均分命中与光标槽位并保留真实源码偏移。
struct BlankGap {
    top: f32,
    row: f32,
    offsets: Range<usize>,
    /// 各空行在换行符之前已有的水平空白字节数（用于支持空行上键入空格）。
    row_cols: Vec<usize>,
    /// 位于该顶层块之前；文末空行为 `None`。
    before_block: Option<usize>,
    /// 是否位于文档末尾（下方没有后续块，末行光标可按正文完整高度绘制）。
    trailing: bool,
}

impl BlankGap {
    fn new(
        offsets: Range<usize>,
        all_offsets: &[usize],
        rope: Option<&ropey::Rope>,
        top: f32,
        height: f32,
        before_block: Option<usize>,
        trailing: bool,
    ) -> Self {
        let count = offsets.len() - 1;
        let row = height / count as f32;
        let mut row_cols = Vec::with_capacity(count);
        for i in 0..count {
            let start = all_offsets[offsets.start + i];
            let next = all_offsets[offsets.start + i + 1];
            let cols = rope
                .filter(|r| start <= next && next <= r.len_bytes())
                .map_or(0, |r| {
                    let slice = r.byte_slice(start..next);
                    let mut len = slice.len_bytes();
                    while len > 0 && matches!(slice.byte(len - 1), b'\r' | b'\n') {
                        len -= 1;
                    }
                    len
                });
            row_cols.push(cols);
        }
        Self {
            top,
            row,
            offsets,
            row_cols,
            before_block,
            trailing,
        }
    }

    fn bottom(&self) -> f32 {
        self.top + self.row * (self.offsets.len() - 1) as f32
    }
}

/// 整篇排版文档中全部可编辑空行槽位的集合与坐标查询器。
pub(super) struct BlankGaps {
    gaps: Vec<BlankGap>,
    offsets: Arc<[usize]>,
    space_width: f32,
}

impl BlankGaps {
    /// 使用解析得到的空行偏移表与空格宽度构造空的空行槽集合。
    pub(super) fn new(offsets: Arc<[usize]>, space_width: f32) -> Self {
        Self {
            gaps: Vec::new(),
            offsets,
            space_width,
        }
    }

    /// 追加一段连续空行槽位。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn push(
        &mut self,
        offsets: Range<usize>,
        rope: Option<&ropey::Rope>,
        top: f32,
        height: f32,
        before_block: Option<usize>,
        trailing: bool,
    ) {
        self.gaps.push(BlankGap::new(
            offsets,
            &self.offsets,
            rope,
            top,
            height,
            before_block,
            trailing,
        ));
    }

    /// 判断新解析的 `blank` 空行索引结构是否与当前空行槽完全一致，以决定能否走单块就地平移快路径。
    pub(super) fn matches_layout(&self, blank: &BlankRows) -> bool {
        if blank.offsets().len() != self.offsets.len() {
            return false;
        }
        for gap in &self.gaps {
            let expected = match gap.before_block {
                Some(b) => blank.before(b),
                None => blank.after(),
            };
            if gap.offsets != expected {
                return false;
            }
        }
        true
    }

    /// 块级单步编辑完成后，按各块高度变化同步平移其后的空行槽，并更新最新解析的空行偏移表。
    pub(super) fn commit_edit(&mut self, block_shifts: &[(usize, f32)], offsets: Arc<[usize]>) {
        for &(block, dy) in block_shifts {
            if dy.abs() > 0.0 {
                for gap in &mut self.gaps {
                    if gap.before_block.is_none_or(|b| b > block) {
                        gap.top += dy;
                    }
                }
            }
        }
        self.offsets = offsets;
    }

    /// 命中内容坐标 `(x, y)` 处的空行原文字节偏移。
    pub(super) fn hit(&self, x: f32, y: f32) -> Option<usize> {
        let i = self
            .gaps
            .partition_point(|gap| gap.top <= y)
            .checked_sub(1)?;
        let gap = &self.gaps[i];
        let row = ((y - gap.top) / gap.row).floor() as usize;
        let start = *self.offsets[gap.offsets.start..gap.offsets.end - 1].get(row)?;
        let max_col = gap.row_cols.get(row).copied().unwrap_or(0);
        let col = if max_col > 0 && x > 0.0 {
            ((x / self.space_width).round() as usize).min(max_col)
        } else {
            0
        };
        Some(start + col)
    }

    /// 返回显式空行上原文偏移 `source` 对应的光标位置与高度。
    pub(super) fn point(&self, source: usize, base: f32, pad: f32) -> Option<Point> {
        let i = self
            .gaps
            .partition_point(|gap| self.offsets[gap.offsets.start] <= source)
            .checked_sub(1)?;
        let gap = &self.gaps[i];
        let (end, starts) = self.offsets[gap.offsets.clone()].split_last()?;
        if source >= *end {
            return None;
        }
        let count = starts.len();
        let row = starts.partition_point(|&s| s <= source) - 1;
        let col = source
            .saturating_sub(starts[row])
            .min(gap.row_cols.get(row).copied().unwrap_or(0));
        let x = col as f32 * self.space_width;
        let slot_top = gap.top + row as f32 * gap.row;
        let (_, body_height) = line_box(0.0, base);
        // 空行光标与正文光标保持一致高度；在整段空行槽内居中并留出上下 1px 间隙，
        // 既不会因为单行槽高偏窄而变短，也不会侵入上下相邻段落。
        let (caret_y, caret_h) = if gap.trailing && row + 1 == count {
            (slot_top, body_height)
        } else {
            let min_y = gap.top + 1.0;
            let max_bottom = gap.bottom() - 1.0;
            let available = (max_bottom - min_y).max(0.0);
            let h = body_height.min(available).max(12.0);
            let ideal = slot_top + (gap.row - h) / 2.0;
            let y = ideal.clamp(min_y, (max_bottom - h).max(min_y));
            (y, h)
        };
        Some((x, pad + caret_y, caret_h))
    }

    /// 若第 `block` 块与下一块之间存在可编辑空行槽，返回该空行槽的上边缘（相对内容顶部）。
    pub(super) fn top_after(&self, block: usize) -> Option<f32> {
        self.gaps
            .iter()
            .find(|gap| gap.before_block == Some(block + 1))
            .map(|gap| gap.top)
    }

    /// 返回与原文选区 `from..to` 相交的空行高亮矩形（相对内容左上角，逻辑像素）。
    pub(super) fn selection_rects(
        &self,
        from: usize,
        to: usize,
        base: f32,
        width: f32,
    ) -> Vec<Rect> {
        if from >= to {
            return Vec::new();
        }
        let (_, body_height) = line_box(0.0, base);
        let mut rects = Vec::new();
        for gap in &self.gaps {
            let Some((&end, starts)) = self.offsets[gap.offsets.clone()].split_last() else {
                continue;
            };
            if end <= from || starts[0] >= to {
                continue;
            }
            for (row, &row_start) in starts.iter().enumerate() {
                let row_next = self.offsets[gap.offsets.start + row + 1];
                if to <= row_start || from >= row_next {
                    continue;
                }
                let cols = gap.row_cols.get(row).copied().unwrap_or(0);
                let text_end = row_start + cols;
                let a = from.saturating_sub(row_start).min(cols);
                let b = to.saturating_sub(row_start).min(cols);
                let continues = to > text_end;
                let x0 = a as f32 * self.space_width;
                let x1 = if continues {
                    width
                } else {
                    b as f32 * self.space_width
                };
                if x1 <= x0 {
                    continue;
                }
                let slot_top = gap.top + row as f32 * gap.row;
                let single_row = from >= row_start && to <= text_end;
                let (y, h) = if single_row {
                    let h = body_height.min(gap.row);
                    (slot_top + (gap.row - h) / 2.0, h)
                } else {
                    (slot_top, gap.row)
                };
                rects.push(Rect::new(x0, y, x1 - x0, h));
            }
        }
        rects
    }

    /// 方向键按正文行高跨过较矮的空行时，先停在空行槽中心，避免直接跳过空行。
    pub(super) fn limit_step(&self, from: f32, target: f32, pad: f32) -> f32 {
        let down = target > from;
        let at = from - pad + if down { 0.5 } else { -0.5 };
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
        let center = pad + gap.top + (row + 0.5) * gap.row;
        if down {
            target.min(center)
        } else {
            target.max(center)
        }
    }
}
