//! Markdown 预览的排版结果与位置查询。
//!
//! [`Laid`] 是整篇文档的排版结果，由 [`Laid::build`] 按顶层块生成并缓存：文字变化时只重排变化的
//! 块，宽度、字号、配色或图片大小变化时整篇重排。位置查询在预览坐标与原文位置之间换算，
//! 供点击、选择、滚动同步和可编辑的预览使用。本模块不依赖 `App`，绘制在
//! `view::preview::markdown` 中。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use winc_render::{Color, Frame, Rect};
use winc_typeset::{Cx, Env, Item, LaidBlock, Raw, Span, margins};

use crate::decorations::{DecorationId, DecorationSet};
use crate::mapping::{LiveKey, LiveLeaf};

mod blanks;
mod motion;
mod query;
mod types;
mod widgets;

use blanks::BlankGaps;
pub use motion::{Before, Fading, Leaving, Moves};
use query::Hit;
pub use types::{
    BASE_RATIO, COPY, EXPAND, Key, LAYOUT_BUDGET, Later, LayoutInput, MAX_WIDTH, NEAR_BLOCKS, Near,
    PAD, PAD_NARROW, Point, SHED_BLOCKS, TABLE, TASK, WIDE_WIDTH, interpolate, line_box,
};
use types::{CONTENT_SLACK, block_has_toggle, build_cx, build_live_raw, prepare_memo};
pub use widgets::{LaidWidget, WIDGET_GAP};
use widgets::{WidgetBuckets, WidgetSlots};
pub use winc_typeset::{DETAILS, Memo};

/// 排版完成的文档。
pub struct Laid {
    /// 本次排版使用的外部条件键。
    pub key: Key,
    /// 排版依据的解析结果（`Arc` 的地址），换了说明文字改了。
    pub source: usize,
    /// 各顶层块：内容哈希和排版结果。
    blocks: Vec<(u64, Arc<LaidBlock>)>,
    /// 各顶层块的上边缘（相对内容顶部）。
    tops: Vec<f32>,
    /// 各顶层块的文字在整份文字（各块的 [`LaidBlock::text`] 连起来）里从第几个字符开始。
    starts: Vec<usize>,
    /// 整份文字的字符数。
    chars: usize,
    /// 各顶层块在原文里从第几行开始（见 `Markdown::lines`），分栏对齐滚动用。
    lines: Vec<usize>,
    /// 各顶层块在原文中的字节范围（见 `Markdown::ranges`），预览与原文互相定位用。
    ranges: Vec<std::ops::Range<usize>>,
    /// 块间与文末可编辑空行槽位集合。
    blanks: BlankGaps,
    /// 排版时光标所在的叶子块，见 [`LiveKey`]。
    ///
    /// 不能编辑时为 `None`；光标换了叶子块（表格里换了一格）、或者打了字，这一块要重排。
    live: Option<LiveKey>,
    /// 上一次找的光标位置和结果（见 [`Laid::point_of`]）：滚动时光标不动，不必每帧在大块里再找。
    caret: std::cell::Cell<Option<CachedCaret>>,
    /// 这一帧画在上一显示行末尾的光标位置（原文字节偏移）。
    drawn_upstream: Option<usize>,
    /// 内容总高度。
    pub height: f32,
    /// 上一次画的时候内容的左边缘（窗口坐标，逻辑像素），鼠标定位用。
    left: f32,
    /// 上一次画的时候内容四周的留白（逻辑像素），鼠标定位用。
    pad: f32,
    /// 比正文宽的表格的横向滚动位置（逻辑像素）：键为（顶层块序号，块中表格的序号，见
    /// [`LaidBlock::scrollers`]）。重排后按序号沿用。
    hscroll: HashMap<(usize, usize), f32>,
    /// 上一次横向滚动使其可见的光标位置（原文字节偏移），见 [`Laid::follow_caret`]。
    followed: Option<usize>,
    /// 各顶层块是否仍沿用旧条件下的排版结果（见 [`Later`]）；全部按当前条件排好时为空。
    stale: Vec<bool>,
    /// 排版时保留了排版结果的范围（见 [`Near`]）；文档不长、全部保留时为 `None`。
    near: Option<Near>,
    /// 本次排版在块间预留出的块级虚拟挂件槽位集合。
    widgets: WidgetSlots,
}

#[derive(Clone, Copy)]
struct CachedCaret {
    source: usize,
    upstream: bool,
    point: Option<Point>,
}

impl Laid {
    /// 编辑器核心 · 排版入口：把解析块变成带屏幕坐标的绘制项，由 `winc-typeset` 排字、折行。
    /// `reuse` 里有同样内容的块直接沿用；长文只在需要的范围保留绘制项。
    ///
    /// 返回排好的 [`Laid`] 以及排版期间遇到、尚未读取的图片列表（本地路径与原地址）。
    /// 光标所在的叶子块（`live`）结合当前原文即时排版，不能当作普通块在不同光标位置复用。
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        input: LayoutInput<'_>,
        frame: &mut Frame<'_>,
        parsed: &winc_preview::markdown::Markdown,
        key: Key,
        source: usize,
        live: Option<&LiveLeaf>,
        reuse: Vec<(u64, Arc<LaidBlock>)>,
        later: Option<Later<'_>>,
        near: Option<Near>,
    ) -> (Laid, Vec<(PathBuf, String)>) {
        let reuse: HashMap<u64, Arc<LaidBlock>> = reuse.into_iter().collect();
        let rope = input.rope;
        let preedit = input.preedit;
        let editable = input.editable;
        let input_checked = input.checked;
        let decorations = input.decorations;
        let raw = build_live_raw(rope, preedit, live);
        prepare_memo(input.memo, key);
        let space_width = frame
            .advance_faux(
                winc_render::FontKind::Ui,
                winc_render::Faux::default(),
                key.base,
                ' ',
            )
            .max(1.0);
        let (mut cx, properties) = build_cx(input, frame, key);
        let env = Env {
            color: cx.colors.text,
            depth: 0,
        };
        let mut blocks: Vec<(u64, Arc<LaidBlock>)> = Vec::with_capacity(parsed.blocks.len());
        let mut tops = Vec::with_capacity(parsed.blocks.len());
        let mut starts = Vec::with_capacity(parsed.blocks.len());
        // 预先算出当前文档下各顶层块的字节范围，便于在排版流中准确锚定块级虚拟挂件。
        let empty = if parsed.blocks.is_empty() && editable {
            rope.filter(|rope| rope.chars().all(char::is_whitespace))
                .map(|rope| 0..rope.len_bytes())
        } else {
            None
        };
        let ranges = {
            // 解析之后打的字都在光标所在的块里：这一块换成现在的范围，原文在它后面的块按文字
            // 长度的变化挪。
            let mut ranges = empty
                .as_ref()
                .map_or_else(|| parsed.ranges.clone(), |range| vec![range.clone()]);
            if let Some(live) = live
                && let Some(start) = ranges.get(live.block).map(|r| r.start)
            {
                let len = rope.map(|r| r.len_bytes());
                let delta = len.map_or(0, |len| len as i64 - parsed.len as i64);
                let shift = |at: usize| usize::try_from(at as i64 + delta).unwrap_or(0);
                for range in ranges.iter_mut().filter(|r| r.start > start) {
                    *range = shift(range.start)..shift(range.end);
                }
                ranges[live.block] = live.range.clone();
            }
            ranges
        };
        let widget_buckets = WidgetBuckets::collect(decorations, parsed.blocks.len(), &ranges);
        let mut widgets: Vec<LaidWidget> = Vec::new();
        let (mut y, mut below, mut chars) = (0.0, None, 0);
        let blank_row = winc_typeset::blank_row_height(key.base);
        // During an asynchronous reparse, keep the raw editing fallback rather than attaching
        // old source offsets to the newly edited note.
        let checked = editable && input_checked;
        let blank = &parsed.blank_rows;
        let mut blanks = BlankGaps::new(Arc::clone(blank.offsets()), space_width);
        let blank_before = |i: usize, offsets: std::ops::Range<usize>| {
            (checked && !offsets.is_empty() && parsed.collapsed_separator_before(i).is_none())
                .then_some(offsets)
        };
        let after = blank.after();
        // A single conventional final newline need not add a permanent row.
        let blank_after = checked && after.len() > 2;
        let mapped = live.is_some_and(|live| {
            [
                blank_before(live.block, blank.before(live.block)),
                blank_before(live.block + 1, blank.before(live.block + 1)),
                blank_after.then(|| after.clone()),
            ]
            .into_iter()
            .flatten()
            .any(|r| (blank.offsets()[r.start]..blank.offsets()[r.end - 1]).contains(&live.at))
        });
        let mut stale = Vec::new();
        let near = near.filter(|_| parsed.blocks.len() > SHED_BLOCKS);
        let mut fresh = 0usize;
        // 仅含 `<details>` 的块按重复序号区分开合状态，避免对无折叠块的长文档构造整表 HashMap。
        let mut repeats: HashMap<u64, u32> = HashMap::new();
        let live_key_for_hash = live.map(|l| l.key(preedit));
        for (i, (block, &hash)) in parsed.blocks.iter().zip(&parsed.hashes).enumerate() {
            let (block, hash) = if let (0, Some(panel)) = (i, &properties) {
                (
                    panel,
                    winc_typeset::memo_key(|h| std::hash::Hash::hash(&(hash, panel), h)),
                )
            } else {
                (block, hash)
            };
            let repeat = if block_has_toggle(block) {
                *repeats.entry(hash).and_modify(|n| *n += 1).or_insert(0)
            } else {
                0
            };
            let hash = match repeat {
                0 => hash,
                n => winc_typeset::memo_key(|h| std::hash::Hash::hash(&(hash, n), h)),
            };
            cx.repeat = repeat;
            cx.toggles.clear();
            let raw = raw
                .as_ref()
                .filter(|r| !mapped && r.0 == i && !(i == 0 && properties.is_some()));
            // 显示原文的块用 O(1) 的 `live_key` 区分缓存键，避免对超大块原文做全量哈希。
            let hash = match (raw, &live_key_for_hash) {
                (Some(_), Some(lk)) => winc_typeset::memo_key(|h| {
                    use std::hash::Hash;
                    (hash, lk).hash(h);
                }),
                _ => hash,
            };
            let kept = later.as_ref().and_then(|later| {
                let (old, block) = later.old.get(i)?;
                let keep = *old == hash
                    && !later.window.contains(&i)
                    && !reuse.contains_key(&hash)
                    && std::time::Instant::now() >= later.deadline;
                keep.then(|| Arc::clone(block))
            });
            if kept.is_some() {
                stale.resize(parsed.blocks.len(), false);
                stale[i] = true;
            }
            // 沿用的结果已经释放了要画的项，而这一块在需要保留的范围内：重新排版。
            let (margin, margin_bottom) = margins(block, key.base, false);
            let already = blocks.last().map_or(0.0, |(_, b)| b.editing_blank_height());
            let offsets = blank.before(i);
            let extra_rows = offsets.len().saturating_sub(1 + usize::from(i != 0));
            let has_blank_gap =
                editable && !offsets.is_empty() && parsed.collapsed_separator_before(i).is_none();
            let mapped_gap = blank_before(i, offsets.clone());
            let gap = if editable {
                (extra_rows as f32 * blank_row - already).max(0.0)
            } else {
                0.0
            };
            let mut space = below.map_or(0.0, |below: f32| below.max(margin)) + gap;
            if already == 0.0 && has_blank_gap {
                // 无论后台解析是否追上当前版本，保持完全一致的空行高度，避免异步解析完成时
                // 上方间距跳变导致页面持续下滚。
                space = space.max(blank_row);
            }
            if already == 0.0
                && let Some(offsets) = mapped_gap
            {
                blanks.push(offsets, rope, y, space, Some(i), false);
            }
            let top = widget_buckets.place_before(i, key.width, y + space, &mut widgets);
            let wanted = |laid: &LaidBlock| {
                let needed = near
                    .as_ref()
                    .is_none_or(|near| near.holds(i, top, laid.height));
                !(needed && laid.is_shed())
            };
            let reused = reuse.get(&hash).filter(|laid| wanted(laid)).cloned();
            let kept = kept.filter(|laid| wanted(laid));
            let laid = reused.or(kept).unwrap_or_else(|| {
                // 很长的文档每排完 [`SHED_BLOCKS`] 块清空一次排版缓存，排版期间缓存不随文档增长。
                fresh += 1;
                if near.is_some() && fresh.is_multiple_of(SHED_BLOCKS) {
                    cx.memo.keep_only(std::iter::empty());
                }
                Arc::new(cx.layout_block(block, key.width, env, raw.map(|(_, r)| r)))
            });
            // 不在需要保留的范围内的块释放要画的项；显示原文的块（光标所在）总是保留。
            let far = near
                .as_ref()
                .is_some_and(|near| !near.holds(i, top, laid.height));
            let laid = if far && raw.is_none() && !laid.is_shed() {
                Arc::new(laid.shed())
            } else {
                laid
            };
            starts.push(chars);
            chars += laid.chars;
            tops.push(top);
            y = widget_buckets.place_after(i, key.width, top + laid.height, &mut widgets);
            below = Some(margin_bottom);
            blocks.push((hash, laid));
        }
        if editable && !blocks.is_empty() {
            let already = blocks.last().map_or(0.0, |(_, b)| b.editing_blank_height());
            let extra = after.len().saturating_sub(2) as f32 * blank_row;
            if already == 0.0 && blank_after {
                blanks.push(after, rope, y, extra, None, true);
                y += extra;
            } else {
                y += (extra - already).max(0.0);
            }
        }
        // A note with only blank lines has no Markdown blocks, but those rows still
        // need real source positions in the editable view.
        if let (Some(range), Some(rope)) = (&empty, rope) {
            let raw = Raw::new(
                rope.to_string().into(),
                range.clone(),
                range.end,
                0,
                true,
                None,
            );
            let laid = cx.layout_raw(raw, key.width);
            starts.push(0);
            tops.push(0.0);
            y = laid.height;
            blocks.push((0, Arc::new(laid)));
        }
        y = widget_buckets.place_orphans(key.width, y, &mut widgets);
        cx.memo.sweep(blocks.iter().map(|(_, b)| &*b.memo));
        let wanted = std::mem::take(&mut cx.wanted);
        let laid = Laid {
            key,
            source,
            blocks,
            tops,
            starts,
            chars,
            lines: if empty.is_some() {
                vec![0]
            } else {
                parsed.lines.clone()
            },
            ranges,
            blanks,
            live: live.map(|l| l.key(preedit)),
            caret: std::cell::Cell::new(None),
            drawn_upstream: None,
            height: y,
            left: 0.0,
            pad: 0.0,
            hscroll: HashMap::new(),
            followed: None,
            stale,
            near,
            widgets: WidgetSlots::new(widgets, widget_buckets.hash()),
        };
        (laid, wanted)
    }

    /// 是否仍有沿用旧结果的块。
    pub fn pending(&self) -> bool {
        !self.stale.is_empty()
    }

    /// 检查 `range` 内是否仍有沿用旧结果的块。
    pub fn has_stale_in(&self, range: std::ops::Range<usize>) -> bool {
        range
            .into_iter()
            .any(|i| self.stale.get(i).copied().unwrap_or(false))
    }

    /// 沿用旧结果的块数量（用于检查分帧排版进度）。
    pub fn stale_count(&self) -> usize {
        self.stale.iter().filter(|s| **s).count()
    }

    /// 第 `index` 块是否仍沿用旧结果。
    pub fn is_stale(&self, index: usize) -> bool {
        self.stale.get(index).copied().unwrap_or(false)
    }

    /// 排版时保留了绘制项的范围。
    pub fn near(&self) -> Option<&Near> {
        self.near.as_ref()
    }

    /// 当前帧条件、源码、光标状态、块级挂件指纹与保留范围是否全部命中缓存。
    pub fn matches_frame(
        &self,
        key: Key,
        source: usize,
        live: Option<&LiveKey>,
        widgets_hash: u64,
        near: &Near,
        view_h: f32,
    ) -> bool {
        self.key == key
            && self.source == source
            && self.live.as_ref() == live
            && self.widgets.fingerprint() == widgets_hash
            && !self.pending()
            && self
                .near
                .as_ref()
                .is_none_or(|kept| kept.covers(near, view_h))
    }

    /// 是否只是续排上一帧未完的块或更新保留范围（条件、源码、光标、挂件均未变）。
    pub fn can_resume(
        &self,
        key: Key,
        source: usize,
        live: Option<&LiveKey>,
        widgets_hash: u64,
    ) -> bool {
        self.key == key
            && self.source == source
            && self.live.as_ref() == live
            && self.widgets.fingerprint() == widgets_hash
    }

    /// 收集 `near` 覆盖到的所有顶层块索引区间（按 `tops` 二分，O(log N)）。
    fn near_ranges(&self, near: &Near) -> Vec<std::ops::Range<usize>> {
        let len = self.blocks.len();
        let mut ranges = Vec::with_capacity(1 + near.blocks.len());
        let vis = self.blocks_in(near.rows.0, near.rows.1);
        if vis.start < vis.end {
            ranges.push(vis.start.min(len)..vis.end.min(len));
        }
        for r in &near.blocks {
            let s = r.start.min(len);
            let e = r.end.min(len);
            if s < e {
                ranges.push(s..e);
            }
        }
        ranges
    }

    /// 纯滚动或跳转跨出 [`Near`] 缓冲范围时的就地刷新：只释放离开范围的块、只重排新进入且已释放的块，
    /// 避免重建整篇文档的块表、哈希表与空行表。
    pub fn refresh_near(
        &mut self,
        input: LayoutInput<'_>,
        frame: &mut Frame<'_>,
        parsed: &winc_preview::markdown::Markdown,
        live: Option<&LiveLeaf>,
        near: Near,
    ) -> Vec<(PathBuf, String)> {
        if self.blocks.len() <= SHED_BLOCKS || self.blocks.len() != parsed.blocks.len() {
            return Vec::new();
        }
        let old_ranges = self
            .near
            .as_ref()
            .map(|n| self.near_ranges(n))
            .unwrap_or_default();
        let new_ranges = self.near_ranges(&near);
        let live_block = live.map(|l| l.block);

        // 1. 释放已离开新 `near` 范围的旧块。
        for range in old_ranges {
            for i in range {
                if live_block == Some(i) {
                    continue;
                }
                if !near.holds(i, self.tops[i], self.blocks[i].1.height)
                    && !self.blocks[i].1.is_shed()
                {
                    let shed = self.blocks[i].1.shed();
                    self.blocks[i].1 = Arc::new(shed);
                }
            }
        }

        // 2. 仅对新 `near` 范围内已释放的块执行局部排版。
        let needs_layout = new_ranges
            .iter()
            .any(|r| self.blocks[r.clone()].iter().any(|(_, b)| b.is_shed()));
        let mut wanted = Vec::new();
        if needs_layout {
            let key = self.key;
            let raw = build_live_raw(input.rope, input.preedit, live);
            prepare_memo(input.memo, key);
            let (mut cx, properties) = build_cx(input, frame, key);
            let env = Env {
                color: cx.colors.text,
                depth: 0,
            };
            for range in &new_ranges {
                for i in range.clone() {
                    if !self.blocks[i].1.is_shed() {
                        continue;
                    }
                    let block = if let (0, Some(panel)) = (i, &properties) {
                        panel
                    } else {
                        &parsed.blocks[i]
                    };
                    cx.repeat = if block_has_toggle(block) {
                        let h = parsed.hashes[i];
                        parsed.hashes[..i].iter().filter(|&&x| x == h).count() as u32
                    } else {
                        0
                    };
                    cx.toggles.clear();
                    let raw_i = raw
                        .as_ref()
                        .filter(|r| r.0 == i && !(i == 0 && properties.is_some()))
                        .map(|(_, r)| r);
                    let laid = cx.layout_block(block, key.width, env, raw_i);
                    self.blocks[i].1 = Arc::new(laid);
                }
            }
            wanted = std::mem::take(&mut cx.wanted);
            // 只遍历新 `near` 范围内的活跃块清理缓存，不扫描全篇 3 万块。
            cx.memo.keep_only(
                new_ranges
                    .iter()
                    .flat_map(|r| self.blocks[r.clone()].iter().map(|(_, b)| &*b.memo)),
            );
        } else {
            input.memo.keep_only(
                new_ranges
                    .iter()
                    .flat_map(|r| self.blocks[r.clone()].iter().map(|(_, b)| &*b.memo)),
            );
        }

        self.near = Some(near);
        self.caret.set(None);
        wanted
    }

    /// 块数未变的常规单块实时输入快路径：仅重排改动的 1–2 个顶层块，就地平移后续块的 `tops`、
    /// `starts` 与 `gaps`，跳过全篇哈希表构建与空行表重建。不满足快路径前提时返回 `None` 交由
    /// [`Laid::build`] 完整重建。
    pub fn patch_edited(
        &mut self,
        input: LayoutInput<'_>,
        frame: &mut Frame<'_>,
        parsed: &winc_preview::markdown::Markdown,
        source: usize,
        live: Option<&LiveLeaf>,
        near: Near,
    ) -> Option<Vec<(PathBuf, String)>> {
        let n = self.blocks.len();
        if n == 0
            || n != parsed.blocks.len()
            || self.pending()
            || self.widgets.fingerprint() != 0
            || input
                .decorations
                .map_or(0, DecorationSet::block_widget_fingerprint)
                != 0
            || input.properties.is_some()
            || parsed.blank_rows.before_len() != n
            || !self.blanks.matches_layout(&parsed.blank_rows)
        {
            return None;
        }
        // 若光标落在空行上或任意已有块处于尾空行编辑态，回退完整排版以保持空行高度完全一致。
        if live.is_some_and(|l| l.blank) {
            return None;
        }
        let checked = input.editable && input.checked;
        let blank = &parsed.blank_rows;
        let blank_before = |i: usize, offsets: std::ops::Range<usize>| {
            (checked && !offsets.is_empty() && parsed.collapsed_separator_before(i).is_none())
                .then_some(offsets)
        };
        let after = blank.after();
        let blank_after = checked && after.len() > 2;
        if live.is_some_and(|live| {
            [
                blank_before(live.block, blank.before(live.block)),
                blank_before(live.block + 1, blank.before(live.block + 1)),
                blank_after.then(|| after.clone()),
            ]
            .into_iter()
            .flatten()
            .any(|r| (blank.offsets()[r.start]..blank.offsets()[r.end - 1]).contains(&live.at))
        }) {
            return None;
        }

        // 找出所有需要重排的顶层块：旧光标块、新光标块，以及内容哈希发生变化的块。
        let old_live_block = self.live.as_ref().map(|k| k.0).filter(|&i| i < n);
        let new_live_block = live.map(|l| l.block).filter(|&i| i < n);
        let mut dirty: Vec<usize> = Vec::with_capacity(4);
        if let Some(b) = old_live_block {
            dirty.push(b);
        }
        if let Some(b) = new_live_block
            && !dirty.contains(&b)
        {
            dirty.push(b);
        }
        for i in 0..n {
            if self.blocks[i].0 != parsed.hashes[i] && !dirty.contains(&i) {
                dirty.push(i);
                if dirty.len() > 4 {
                    return None;
                }
            }
        }
        if dirty.is_empty() {
            return None;
        }
        dirty.sort_unstable();
        for &i in &dirty {
            if block_has_toggle(&parsed.blocks[i]) || self.blocks[i].1.editing_blank_height() > 0.0
            {
                return None;
            }
        }

        let key = self.key;
        let live_key = live.map(|l| l.key(input.preedit));
        let raw = build_live_raw(input.rope, input.preedit, live);
        prepare_memo(input.memo, key);

        let mut new_blocks: Vec<(usize, u64, LaidBlock)> = Vec::with_capacity(dirty.len());
        let mut patched_in_place: Option<(usize, f32, isize)> = None;
        let same_parsed = self.source == source;
        let wanted = {
            let mut cx = Cx {
                frame,
                base: key.base,
                token: input.token,
                surface: input.surface,
                accent: input.accent,
                colors: input.colors,
                dark: input.dark,
                images: input.images,
                image_path: input.image_path,
                missing_image: None,
                wanted: Vec::new(),
                memo: input.memo,
                toggled: input.toggled,
                text: String::new(),
                chars: 0,
                raw: None,
                cell: None,
                preedit: false,
                line_numbers: key.numbers,
                tall: key.tall,
                repeat: 0,
                toggles: HashMap::new(),
            };
            let env = Env {
                color: cx.colors.text,
                depth: 0,
            };
            // 超大单块（长代码/长列表/长表格）原位修补快路径：若当前独占该块且仅改动单个子项，
            // 直接原位修补 `LaidBlock`，跳过整块数万项的重建与空间索引排序。
            if dirty.len() == 1 {
                let i = dirty[0];
                let block = &parsed.blocks[i];
                let base_hash = parsed.hashes[i];
                let raw_i = raw.as_ref().filter(|r| r.0 == i).map(|(_, r)| r);
                if raw_i.is_some()
                    && let Some(laid_mut) = Arc::get_mut(&mut self.blocks[i].1)
                {
                    let old_h = laid_mut.height;
                    let old_chars = laid_mut.chars;
                    cx.repeat = 0;
                    cx.toggles.clear();
                    if cx.patch_block(laid_mut, block, same_parsed, key.width, env, raw_i) {
                        let hash = match &live_key {
                            Some(lk) => winc_typeset::memo_key(|h| {
                                use std::hash::Hash;
                                (base_hash, lk).hash(h);
                            }),
                            None => base_hash,
                        };
                        let dy = laid_mut.height - old_h;
                        let dc = laid_mut.chars as isize - old_chars as isize;
                        self.blocks[i].0 = hash;
                        patched_in_place = Some((i, dy, dc));
                    }
                }
            }
            if patched_in_place.is_none() {
                for &i in &dirty {
                    let block = &parsed.blocks[i];
                    let base_hash = parsed.hashes[i];
                    let raw_i = raw.as_ref().filter(|r| r.0 == i).map(|(_, r)| r);
                    let hash = match (raw_i, &live_key) {
                        (Some(_), Some(lk)) => winc_typeset::memo_key(|h| {
                            use std::hash::Hash;
                            (base_hash, lk).hash(h);
                        }),
                        _ => base_hash,
                    };
                    cx.repeat = 0;
                    cx.toggles.clear();
                    let laid = cx.layout_block(block, key.width, env, raw_i);
                    // 若编辑产生额外空行高度（如段尾按回车），交给完整 `Laid::build` 精确处理块间距。
                    if laid.editing_blank_height() > 0.0 {
                        return None;
                    }
                    new_blocks.push((i, hash, laid));
                }
            }
            std::mem::take(&mut cx.wanted)
        };

        let mut blank_shifts: Vec<(usize, f32)> = Vec::with_capacity(4);
        if let Some((i, dy, dc)) = patched_in_place {
            if dy.abs() > 0.0 {
                for top in &mut self.tops[i + 1..] {
                    *top += dy;
                }
                self.height += dy;
                blank_shifts.push((i, dy));
            }
            if dc != 0 {
                for s in &mut self.starts[i + 1..] {
                    *s = usize::try_from(*s as isize + dc).unwrap_or(0);
                }
                self.chars = usize::try_from(self.chars as isize + dc).unwrap_or(0);
            }
        }

        // 就地替换改动的块并按高度差与字符数差平移后续块。
        for (i, hash, new_block) in new_blocks {
            let old_h = self.blocks[i].1.height;
            let old_chars = self.blocks[i].1.chars;
            let dy = new_block.height - old_h;
            let dc = new_block.chars as isize - old_chars as isize;
            let far = self
                .near
                .as_ref()
                .is_some_and(|kept| !kept.holds(i, self.tops[i], new_block.height));
            let is_live = new_live_block == Some(i);
            let stored = if far && !is_live {
                Arc::new(new_block.shed())
            } else {
                Arc::new(new_block)
            };
            self.blocks[i] = (hash, stored);
            if dy.abs() > 0.0 {
                for top in &mut self.tops[i + 1..] {
                    *top += dy;
                }
                self.height += dy;
                blank_shifts.push((i, dy));
            }
            if dc != 0 {
                for s in &mut self.starts[i + 1..] {
                    *s = usize::try_from(*s as isize + dc).unwrap_or(0);
                }
                self.chars = usize::try_from(self.chars as isize + dc).unwrap_or(0);
            }
        }

        // 就地更新原文范围、行号与空行偏移表。
        self.ranges.clone_from(&parsed.ranges);
        if let Some(live) = live
            && let Some(start) = self.ranges.get(live.block).map(|r| r.start)
        {
            let len = input.rope.map(|r| r.len_bytes());
            let delta = len.map_or(0, |len| len as i64 - parsed.len as i64);
            if delta != 0 {
                let shift = |at: usize| usize::try_from(at as i64 + delta).unwrap_or(0);
                for range in self.ranges.iter_mut().filter(|r| r.start > start) {
                    *range = shift(range.start)..shift(range.end);
                }
            }
            self.ranges[live.block] = live.range.clone();
        }
        self.lines.clone_from(&parsed.lines);
        self.blanks
            .commit_edit(&blank_shifts, Arc::clone(blank.offsets()));
        self.source = source;
        self.live = live_key;
        self.caret.set(None);

        // 清理缓存（大文档只保留活跃块对应缓存，避免长期键入导致缓存增长）。
        if let Some(kept) = self.near.clone() {
            let active = self.near_ranges(&kept);
            input.memo.sweep(
                active
                    .iter()
                    .flat_map(|r| self.blocks[r.clone()].iter().map(|(_, b)| &*b.memo)),
            );
            if !kept.covers(&near, key.tall) {
                let extra = self.refresh_near(input, frame, parsed, live, near);
                let mut wanted = wanted;
                wanted.extend(extra);
                return Some(wanted);
            }
        } else {
            input.memo.sweep(self.blocks.iter().map(|(_, b)| &*b.memo));
        }

        Some(wanted)
    }

    /// 当前排版是否包含任意块级虚拟挂件槽位。
    pub fn has_widgets(&self) -> bool {
        self.widgets.has_any()
    }

    /// 本次排版预留出的块级虚拟挂件槽位数量。
    pub fn widget_count(&self) -> usize {
        self.widgets.len()
    }

    /// 按装饰标识查找对应的已排版块级虚拟挂件槽位。
    pub fn widget_by_id(&self, id: DecorationId) -> Option<&LaidWidget> {
        self.widgets.find_by_id(id)
    }

    /// 枚举与视口裁剪区 `clip` 相交的块级虚拟挂件槽位及其窗口坐标系矩形。
    pub fn visible_widgets(
        &self,
        left: f32,
        origin_y: f32,
        clip: Rect,
    ) -> impl Iterator<Item = (&LaidWidget, Rect)> {
        self.widgets.visible(left, origin_y, clip)
    }

    /// 返回窗口坐标 `(x, y)` 命中的块级虚拟挂件槽位（若存在）。
    pub fn widget_at(&self, area_y: f32, scroll: f32, x: f32, y: f32) -> Option<&LaidWidget> {
        self.widgets
            .hit(x - self.left, y - (area_y + self.pad - scroll))
    }

    /// 若第 `block` 块下方存在 `after` 块级虚拟挂件，返回首个挂件的上边缘（相对内容顶部）。
    pub fn widget_top_after(&self, block: usize) -> Option<f32> {
        self.widgets.top_after(block)
    }

    /// 写入本帧的排版位置与折行归属；留白或归属变化时清除缓存的光标坐标。
    pub fn place(&mut self, left: f32, pad: f32, upstream: Option<usize>) {
        if self.drawn_upstream != upstream || (self.pad - pad).abs() >= 0.01 {
            self.caret.set(None);
        }
        self.left = left;
        self.pad = pad;
        self.drawn_upstream = upstream;
    }

    /// 沿用上一次排版记录的宽表格横向滚动位置。
    pub fn restore_hscroll_from(&mut self, old: Option<&Laid>) {
        if let Some(old) = old {
            self.hscroll = old.hscroll.clone();
        }
    }

    /// 设置表格 `key` 的横向滚动位置（逻辑像素）。
    pub fn set_hscroll(&mut self, key: (usize, usize), value: f32) {
        self.hscroll.insert(key, value);
    }

    /// 内容一栏的左右边界（窗口横坐标，逻辑像素）。
    pub fn column_bounds(&self) -> (f32, f32) {
        (self.left, self.left + self.key.width)
    }

    /// 给定区域顶边 `area_y` 与滚动量 `scroll`，返回内容顶边在窗口中的纵坐标。
    pub fn origin_y(&self, area_y: f32, scroll: f32) -> f32 {
        area_y + self.pad - scroll
    }

    /// 把相对预览顶部的纵坐标 `y` 换算为相对首块顶边的内容坐标（减去上留白）。
    pub fn content_y(&self, y: f32) -> f32 {
        y - self.pad
    }

    /// 正文末尾（含上留白、不含下留白）相对预览顶部的纵坐标。
    pub fn body_bottom(&self) -> f32 {
        self.height + self.pad
    }

    /// 光标所在叶子块对应的文档版本号；不在实时编辑态时为 `None`。
    pub fn live_revision(&self) -> Option<u64> {
        self.live.as_ref().map(|k| k.2)
    }

    /// 当前排版是否包含实时编辑叶子块。
    pub fn has_live(&self) -> bool {
        self.live.is_some()
    }

    /// 按顺序枚举各顶层块的（序号，上边缘，排版结果）。
    pub fn iter_blocks(&self) -> impl Iterator<Item = (usize, f32, &LaidBlock)> {
        self.blocks
            .iter()
            .zip(&self.tops)
            .enumerate()
            .map(|(i, ((_, block), &top))| (i, top, block.as_ref()))
    }

    /// 返回第 `index` 个顶层块的排版结果。
    pub fn block_at(&self, index: usize) -> Option<&LaidBlock> {
        self.blocks.get(index).map(|(_, block)| block.as_ref())
    }

    /// 顶层块数量。
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// 复制顶层块列表供分帧排版沿用。
    pub fn clone_blocks(&self) -> Vec<(u64, Arc<LaidBlock>)> {
        self.blocks.clone()
    }

    /// 消费自身并取出顶层块列表。
    pub fn into_blocks(self) -> Vec<(u64, Arc<LaidBlock>)> {
        self.blocks
    }

    /// 若启用了远块卸载，清理排版缓存中不再使用的条目。
    pub fn sweep_memo(&self, memo: &mut Memo) {
        if self.near.is_some() {
            memo.keep_only(self.blocks.iter().map(|(_, block)| &*block.memo));
        }
    }

    /// 各顶层块的上边缘（相对内容顶部，逻辑像素）。
    pub fn tops(&self) -> &[f32] {
        &self.tops
    }

    /// 第 `index` 块的上边缘（相对内容顶部，逻辑像素）。
    pub fn top(&self, index: usize) -> f32 {
        self.tops[index]
    }

    /// 第 `index` 块的上边缘到下一块上边缘（末块到文末）的区间（逻辑像素）。
    pub fn block_span_y(&self, index: usize) -> Option<(f32, f32)> {
        let top = *self.tops.get(index)?;
        let bottom = self.tops.get(index + 1).copied().unwrap_or(self.height);
        Some((top, bottom))
    }

    /// 返回纵坐标 `y`（相对内容顶部）所在的顶层块序号。
    pub fn block_at_y(&self, y: f32) -> Option<usize> {
        self.tops.partition_point(|&top| top <= y).checked_sub(1)
    }

    /// 第 `index` 块的上沿到下一块上沿的距离（逻辑像素）。
    pub fn room_below(&self, index: usize) -> f32 {
        let top = self.tops[index];
        self.tops
            .get(index + 1)
            .map_or(self.blocks[index].1.height, |next| next - top)
    }

    /// 返回视口顶边 `view_top` 下首个可能需要绘制的顶层块序号。
    pub fn first_visible_block(&self, origin_y: f32, view_top: f32) -> usize {
        self.tops
            .partition_point(|&t| origin_y + t <= view_top)
            .saturating_sub(1)
    }

    /// 字符选区 `a..b` 覆盖的顶层块范围（前后各多留一块）。
    pub fn block_range_for_selection(&self, a: usize, b: usize) -> std::ops::Range<usize> {
        let block = |at: usize| self.starts.partition_point(|&s| s <= at).saturating_sub(1);
        block(a.min(b)).saturating_sub(1)..block(a.max(b)) + 2
    }

    /// 第 `index` 块的文字在整份文字里的起始字符偏移。
    pub fn block_start(&self, index: usize) -> usize {
        self.starts[index]
    }

    /// 第 `index` 块在原文中的字节范围。
    pub fn block_range(&self, index: usize) -> Option<&std::ops::Range<usize>> {
        self.ranges.get(index)
    }

    /// 返回第 `block` 块第 `item` 项图片的原文开头、所在块结尾，以及是否位于块开头。
    pub fn image_source_at(&self, block: usize, item: usize) -> Option<(usize, usize, bool)> {
        let at = self.blocks.get(block)?.1.image_source(item)? as usize;
        let range = self.ranges.get(block)?;
        Some((range.start + at, range.end, at == 0))
    }

    /// 是否有任意块已排入输入法组字文字。
    pub fn has_preedit(&self) -> bool {
        self.blocks.iter().any(|(_, b)| b.has_preedit())
    }

    /// 记录各块在滚动位置 `scroll` 下的当前绘制位置，供重排后计算过渡动画。
    pub fn snapshot_before(
        &self,
        scroll: f32,
        offset: impl Fn(usize) -> f32,
        current_revision: Option<u64>,
    ) -> Before {
        Before::capture(
            &self.blocks,
            &self.tops,
            scroll,
            offset,
            self.live.as_ref().map(|k| k.2).or(current_revision),
        )
    }

    /// 计算从重排前 `before` 过渡到当前排版所需的各块偏移与淡入项。
    pub fn motion_from(&self, before: &Before, scroll: f32, muted: Color, slide: bool) -> Moves {
        before.motion_to(
            &self.blocks,
            &self.tops,
            self.live.as_ref().map(|k| k.2),
            scroll,
            muted,
            slide,
        )
    }

    /// 返回内容坐标 `y` 所在的顶层块，以及 `y` 到该块上边缘的距离（逻辑像素）。
    pub fn place_at(&self, y: f32) -> Option<(usize, f32)> {
        let i = self.tops.partition_point(|&t| t <= y).checked_sub(1)?;
        Some((i, y - self.tops[i]))
    }

    /// 返回与内容坐标 `from` 到 `to` 相交的顶层块。
    pub fn blocks_in(&self, from: f32, to: f32) -> std::ops::Range<usize> {
        let first = self.tops.partition_point(|&t| t <= from).saturating_sub(1);
        first..self.tops.partition_point(|&t| t < to).max(first)
    }

    /// 返回已按当前条件排好的块。
    pub fn fresh(&self) -> Vec<(u64, Arc<LaidBlock>)> {
        let fresh = |i: &usize| !self.stale.get(*i).copied().unwrap_or(false);
        let blocks = (0..self.blocks.len()).filter(fresh);
        blocks.map(|i| self.blocks[i].clone()).collect()
    }

    /// `blocks` 范围里的各顶层块中能选中的文字，逐段返回：所在块的上边缘（相对内容顶部）、在
    /// 整份文字里的起点、横坐标、基线、字号、各字宽度和原文位置，以及所在的块和文字本身。
    pub fn spans(
        &self,
        blocks: std::ops::Range<usize>,
    ) -> impl Iterator<Item = (f32, usize, f32, f32, f32, &Span, usize, &str)> {
        let blocks = blocks.start.min(self.blocks.len())..blocks.end.min(self.blocks.len());
        self.blocks[blocks.clone()]
            .iter()
            .zip(blocks)
            .flat_map(move |((_, block), i)| {
                block
                    .items()
                    .iter()
                    .enumerate()
                    .filter_map(move |(k, item)| match item {
                        Item::Text {
                            x,
                            baseline,
                            size,
                            text,
                            span: Some(span),
                            ..
                        } => Some((
                            self.tops[i],
                            self.starts[i] + span.start,
                            *x - self.shift(i, k).0,
                            *baseline,
                            *size,
                            span,
                            i,
                            &**text,
                        )),
                        _ => None,
                    })
            })
    }

    /// 第 `block` 块第 `item` 项的横向滚动位置（逻辑像素）和所在表格的显示区域（块里的坐标）；
    /// 不在可横向滚动的表格中时为 `(0.0, None)`。
    pub fn shift(&self, block: usize, item: usize) -> (f32, Option<Rect>) {
        let Some((_, laid)) = self.blocks.get(block) else {
            return (0.0, None);
        };
        laid.scroller_of(item).map_or((0.0, None), |s| {
            (
                self.hscroll_of(block, s),
                laid.scroller(s).map(|scroller| scroller.rect),
            )
        })
    }

    /// 第 `block` 块第 `scroller` 个可横向滚动的表格当前的滚动位置，限制在可滚动的范围内。
    pub fn hscroll_of(&self, block: usize, scroller: usize) -> f32 {
        let max = self
            .blocks
            .get(block)
            .and_then(|(_, b)| b.scroller(scroller))
            .map_or(0.0, |s| s.max());
        self.hscroll
            .get(&(block, scroller))
            .copied()
            .unwrap_or(0.0)
            .clamp(0.0, max)
    }

    /// 光标（原文位置 `source`）落在可横向滚动的表格中、位于显示区域之外时，横向滚动该表格使
    /// 光标可见。只在光标移动后对齐一次，不干扰用户自行滚动。
    pub fn follow_caret(&mut self, source: usize) {
        if self.followed == Some(source) {
            return;
        }
        self.followed = Some(source);
        let Some((x, y, _)) = self.caret_point(source) else {
            return;
        };
        let Some(block) = self.block_at_source(source) else {
            return;
        };
        let y = y - self.pad - self.tops[block];
        let Some((s, scroller)) = self.blocks[block].1.scroller_at_y(y) else {
            return;
        };
        let rect = scroller.rect;
        let max = scroller.max();
        // 光标两侧至少留出的距离（逻辑像素），与单元格的内边距相近。
        let margin = (self.key.base * 0.8125).round();
        let now = self.hscroll_of(block, s);
        let next = if x < rect.x + margin {
            now - (rect.x + margin - x)
        } else if x > rect.right() - margin {
            now + (x - (rect.right() - margin))
        } else {
            return;
        };
        self.hscroll.insert((block, s), next.clamp(0.0, max));
        // 光标位置按滚动位置算过，滚动后重新计算。
        self.caret.set(None);
    }

    /// 内容坐标 `(x, y)`（相对内容左上角）处可横向滚动的表格：（块序号，表格序号，最大滚动距离）。
    fn scroller_at(&self, x: f32, y: f32) -> Option<(usize, usize, f32)> {
        let i = self.tops.partition_point(|&t| t <= y).checked_sub(1)?;
        let (_, block) = self.blocks.get(i)?;
        let y = y - self.tops[i];
        block.scroller_at(x, y).map(|(s, max)| (i, s, max))
    }

    /// 返回整份文字里第 `from` 到 `to` 个字符。
    pub fn text(&self, from: usize, to: usize) -> String {
        let mut out = String::new();
        let first = self
            .starts
            .partition_point(|&s| s <= from)
            .saturating_sub(1);
        for (i, (_, block)) in self.blocks.iter().enumerate().skip(first) {
            let start = self.starts[i];
            if start >= to {
                break;
            }
            let (a, b) = (from.saturating_sub(start), to - start);
            block.append_text_range(a, b, &mut out);
        }
        out
    }

    /// 整份文字（各块的文字连起来），预览里查找用。
    pub fn full_text(&self) -> String {
        self.blocks.iter().map(|(_, b)| b.text()).collect()
    }

    /// 规范化（已做百分号解码并转小写）后的锚点 `anchor` 的范围：上边缘和高度。
    ///
    /// 以 `fnref-` 开头的锚点指向引用脚注的编号；脚注是整条脚注，标题是标题所在的块。
    pub fn anchor_box(&self, anchor: &str) -> Option<(f32, f32)> {
        // 脚注末尾的 ↩ 跳回正文里第一次引用该脚注的位置，即链到脚注的编号。
        let reference = anchor.strip_prefix("fnref-").map(|n| format!("#fn-{n}"));
        self.blocks
            .iter()
            .zip(&self.tops)
            .find_map(|((_, block), top)| {
                let (y, h) = match &reference {
                    Some(url) => block.link_box(url),
                    None => block.anchor_box(anchor),
                }?;
                Some((top + y, h))
            })
    }

    /// 规范化后的锚点 `anchor` 在内容里的高度（逻辑像素）；找不到时返回 `None`。
    pub fn anchor_top(&self, anchor: &str) -> Option<f32> {
        self.anchor_box(anchor).map(|(top, _)| top)
    }

    /// 收集整篇排版中每个标题锚点首次出现的纵坐标（相对内容顶部，逻辑像素）。
    pub fn anchor_positions(&self) -> HashMap<&str, f32> {
        let mut found = HashMap::new();
        for ((_, block), top) in self.blocks.iter().zip(&self.tops) {
            for (id, y) in block.anchors() {
                found.entry(id.as_str()).or_insert(top + y);
            }
        }
        found
    }

    /// 返回整份文字的字符数。
    pub fn chars(&self) -> usize {
        self.chars
    }

    /// 原文的行和预览滚动位置的对应点：文首、各块的开头、文末（原文共 `total` 行），行号和
    /// 位置都递增。
    ///
    /// 两点之间按比例算。
    fn sync_points(&self, total: usize) -> Vec<(f32, f32)> {
        let mut points = vec![(0.0, 0.0)];
        for (line, top) in self.lines.iter().zip(&self.tops) {
            let (line, y) = (*line as f32, self.pad + top);
            if points.last().is_some_and(|&(l, py)| line > l && y >= py) {
                points.push((line, y));
            }
        }
        let end = (total.max(1) as f32, self.height + self.pad * 2.0);
        if points.last().is_some_and(|&(l, y)| end.0 > l && end.1 >= y) {
            points.push(end);
        }
        points
    }

    /// 原文第 `line` 行（可以是小数）对应的预览滚动位置。
    pub fn y_of_line(&self, line: f32, total: usize) -> f32 {
        interpolate(&self.sync_points(total), line, false)
    }

    /// 预览滚到 `y` 时对应原文的第几行（小数）。
    pub fn line_of_y(&self, y: f32, total: usize) -> f32 {
        interpolate(&self.sync_points(total), y, true)
    }

    /// 正文一栏的宽度（逻辑像素）。
    pub fn width(&self) -> f32 {
        self.key.width
    }

    /// 内容总高度（含四周留白）。
    ///
    /// 分栏对齐滚动时判断排版变了没有。
    pub fn height(&self) -> f32 {
        self.height + self.pad * 2.0
    }

    /// 窗口横坐标 `x` 是否落在预览的内容一栏内（左右各放宽 `CONTENT_SLACK`）。
    pub fn in_content(&self, x: f32) -> bool {
        let left = self.left - CONTENT_SLACK;
        let right = self.left + self.key.width + CONTENT_SLACK;
        (left..=right).contains(&x)
    }

    /// 预览上窗口坐标 `(x, y)`（逻辑像素）处是整份文字里的第几个字符。
    pub fn offset_at(&self, area_y: f32, scroll: f32, x: f32, y: f32) -> Option<usize> {
        self.hit(area_y, scroll, x, y)?.offset
    }

    /// 预览上窗口坐标 `(x, y)` 处的字在原文中的位置（字节偏移）。
    pub fn source_at(&self, area_y: f32, scroll: f32, x: f32, y: f32) -> Option<usize> {
        self.hit(area_y, scroll, x, y)?.source
    }

    /// 把窗口坐标 `(x, y)` 处可横向滚动的表格横向滚动 `dx`，返回被滚动的表格键。
    pub fn scroll_table(
        &mut self,
        area_y: f32,
        scroll: f32,
        x: f32,
        y: f32,
        dx: f32,
    ) -> Option<(usize, usize)> {
        let (cx, cy) = (x - self.left, y - (area_y + self.pad - scroll));
        let (block, s, max) = self.scroller_at(cx, cy).filter(|&(.., max)| max > 0.0)?;
        let now = self.hscroll_of(block, s);
        self.hscroll.insert((block, s), (now - dx).clamp(0.0, max));
        Some((block, s))
    }

    fn hit(&self, area_y: f32, scroll: f32, x: f32, y: f32) -> Option<Hit> {
        let origin = area_y + self.pad - scroll;
        let (x, y) = (x - self.left, y - origin);
        query::hit_content(
            &self.blocks,
            &self.tops,
            &self.starts,
            &self.ranges,
            &self.blanks,
            x,
            y,
            &|block, item| self.shift(block, item),
        )
    }

    /// 查找原文字节所属的显示块。脚注的显示顺序与原文不同。
    pub fn block_at_source(&self, source: usize) -> Option<usize> {
        self.ranges
            .iter()
            .rposition(|range| range.contains(&source))
            .or_else(|| self.ranges.iter().rposition(|range| range.end == source))
    }

    /// Position and hit height of an explicit blank row.
    pub fn blank_point(&self, source: usize) -> Option<Point> {
        self.blanks.point(source, self.key.base, self.pad)
    }

    /// 若第 `block` 块与下一块之间存在可编辑空行槽，返回该空行槽的上边缘（相对内容顶部）。
    pub fn gap_top_after(&self, block: usize) -> Option<f32> {
        self.blanks.top_after(block)
    }

    /// 返回与原文选区 `from..to` 相交的空行高亮矩形（相对内容左上角，逻辑像素）。
    pub fn blank_selection_rects(&self, from: usize, to: usize) -> Vec<Rect> {
        self.blanks
            .selection_rects(from, to, self.key.base, self.key.width)
    }

    /// 将任意原文区间 `range` 投影为内容坐标系下的高亮矩形列表（覆盖单行、多行折行、
    /// 跨内联样式与空行空格，供统一装饰层绘制批注、搜索匹配与 AI 修订高亮）。
    pub fn mark_rects(&self, range: std::ops::Range<usize>) -> Vec<Rect> {
        if range.is_empty() {
            return Vec::new();
        }
        let mut rects = self.blank_selection_rects(range.start, range.end);
        query::append_mark_rects(
            &self.blocks,
            &self.tops,
            &self.ranges,
            range,
            &|block, item| self.shift(block, item),
            &mut rects,
        );
        rects
    }

    /// 返回行内虚拟挂件（如 AI 幽灵补全）在原文偏移 `at` 处的锚点 `(x, y, h)`（相对预览顶部）。
    pub fn inline_widget_point(&self, at: usize) -> Option<Point> {
        self.caret_point(at)
    }

    /// Whether a byte begins an actual glyph in the visible source mapping.
    pub(crate) fn shows_source(&self, source: usize) -> bool {
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
    pub(crate) fn limit_blank_step(&self, from: f32, target: f32) -> f32 {
        self.blanks.limit_step(from, target, self.pad)
    }

    /// 原文第 `source` 个字节按这一帧的光标归属显示的位置。
    pub fn caret_point(&self, source: usize) -> Option<Point> {
        self.caret_point_with_affinity(source, self.drawn_upstream == Some(source))
    }

    /// 按指定的折行归属定位。`upstream` 为真时，优先取上一显示行末尾。
    pub fn caret_point_with_affinity(&self, source: usize, upstream: bool) -> Option<Point> {
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
        let block = self.block_at_source(source)?;
        let source = source - self.ranges[block].start;
        // 超大块通过原位修补索引先查局部项窗口（O(log N)），未命中时再回退全块扫描。
        if let Some(win) = self.blocks[block].1.source_item_window(source)
            && let Some(point) = self.point_in_items(block, win, source, upstream)
        {
            return Some(point);
        }
        let len = self.blocks[block].1.items().len();
        self.point_in_items(block, 0..len, source, upstream)
    }

    fn point_in_items(
        &self,
        block: usize,
        items_range: std::ops::Range<usize>,
        source: usize,
        upstream: bool,
    ) -> Option<Point> {
        query::point_in_items(
            &self.blocks[block].1,
            self.tops[block],
            self.pad,
            block,
            items_range,
            source,
            upstream,
            &|b, item| self.shift(b, item),
        )
    }

    /// 可编辑的预览左侧写行号的各显示行：原文行号（从 0 起）和显示行的基线（相对内容顶部）。
    pub fn source_lines(
        &self,
        rope: &ropey::Rope,
        view: std::ops::Range<f32>,
    ) -> Vec<(usize, f32)> {
        query::source_lines_in_view(
            &self.blocks,
            &self.tops,
            &self.ranges,
            rope,
            view,
            &|block, item| self.shift(block, item),
        )
    }

    /// 原文第 `source` 个字节在预览上的位置。
    pub fn point_of(&self, source: usize) -> Option<Point> {
        self.point_of_with_affinity(source, self.drawn_upstream == Some(source))
    }

    /// 原文第 `byte` 个字节所在行或块的上边缘（含留白），用于定位跳转。
    pub fn source_top(&self, byte: usize) -> Option<f32> {
        self.caret_point(byte).map(|(_, top, _)| top).or_else(|| {
            let i = self
                .ranges
                .partition_point(|range| range.start <= byte)
                .checked_sub(1)?;
            Some(self.tops[i] + self.pad)
        })
    }

    /// 返回原文位置 `source` 处文字的字体、模拟粗斜体和字号。
    pub fn font_at(
        &self,
        source: usize,
    ) -> Option<(winc_render::FontKind, winc_render::Faux, f32)> {
        let block = self.block_at_source(source)?;
        let source = source - self.ranges[block].start;
        self.blocks[block].1.font_at_source(source)
    }

    /// 整份文字里第 `offset` 个字符所在那一行的上边缘（和滚动位置同一个量法）。
    pub fn offset_top(&self, offset: usize) -> Option<f32> {
        let i = self
            .starts
            .partition_point(|&s| s <= offset)
            .saturating_sub(1);
        let (_, block) = self.blocks.get(i)?;
        if block.is_shed() {
            let part = (offset - self.starts[i]) as f32 / block.chars.max(1) as f32;
            return Some(self.pad + self.tops[i] + block.height * part.min(1.0));
        }
        self.spans(i..i + 1)
            .find(|(_, start, _, _, _, span, _, _)| offset < start + span.advances.len())
            .map(|(top, _, _, baseline, size, ..)| self.pad + line_box(top + baseline, size).0)
    }
}
