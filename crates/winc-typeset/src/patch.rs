//! 超大单块（长代码块、长列表、长表格）的原位修补索引与快路径。
//!
//! 当单个顶层块包含大量子项（例如数千行代码、数千条列表项或数千行表格）时，
//! 实时输入若每次重排整块会产生数万次哈希查询、项拷贝与空间索引排序。
//! 本模块在首次排版超大块时记录轻量结构索引（小块不构建、零内存开销），
//! 后续单行/单项/单单元格输入时仅重排受影响的 1 处内容并原位平移其余项，
//! 结果与整块重排完全一致。

use std::ops::Range;
use std::sync::Arc;

use winc_preview::markdown::{Align, Block, Inline, Item as ListItem};
use winc_render::{Color, Rect};

use super::{
    BODY_LINE, Cx, Env, Item, LIST_GAP, LaidBlock, Out, Para, Raw, RawKind, Row, SCROLL_BAR_ROOM,
    TABLE_RADIUS, TABLE_TEXT, blanks, is_fence, rounded_frame,
};

/// 构建代码块原位修补索引的行数下限。
pub(crate) const HUGE_CODE_LINES: usize = 128;
/// 构建列表原位修补索引的项数下限。
pub(crate) const HUGE_LIST_ITEMS: usize = 64;
/// 构建表格原位修补索引的行数下限。
pub(crate) const HUGE_TABLE_ROWS: usize = 64;

/// 单个超大顶层块的原位修补索引。
pub(crate) enum PatchIndex {
    /// 围栏代码块（`raw_box` 编辑态）。
    CodeBox(CodeBoxPatch),
    /// 顶层长列表。
    List(ListPatch),
    /// 顶层长表格。
    Table(TablePatch),
}

/// 围栏代码块单行的布局切片元数据。
pub(crate) struct CodeLineSpan {
    /// 该行在 [`LaidBlock::items`] 中的项区间。
    pub(crate) items: Range<u32>,
    /// 该行在 [`LaidBlock::text`] 中的字节区间。
    pub(crate) text_bytes: Range<usize>,
    /// 该行在 [`LaidBlock::text`] 中的字符区间。
    pub(crate) chars: Range<usize>,
    /// 该行在顶层块原文中的字节区间（不含末尾 `\n`）。
    pub(crate) source_start: u32,
    pub(crate) source_end: u32,
    /// 该行在块内的上边缘与高度（逻辑像素）。
    pub(crate) top: f32,
    pub(crate) height: f32,
    /// 排完该行后的词法高亮状态。
    pub(crate) end_state: winc_syntax::LineState,
    /// 该行是否为围栏行。
    pub(crate) fence: bool,
    /// 该行当前是否绘制（围栏行仅光标位于其上时绘制）。
    pub(crate) shown: bool,
}

/// 围栏代码块原位修补索引。
pub(crate) struct CodeBoxPatch {
    pub(crate) lang: Box<str>,
    pub(crate) width: f32,
    pub(crate) pad: f32,
    pub(crate) row: Row,
    pub(crate) bg_item: u32,
    pub(crate) code_item: Option<u32>,
    pub(crate) lines: Vec<CodeLineSpan>,
    pub(crate) raw_len: usize,
    pub(crate) leaf_start: usize,
}

/// 长列表单项的布局切片元数据。
pub(crate) struct ListEntry {
    pub(crate) items: Range<u32>,
    pub(crate) text_bytes: Range<usize>,
    pub(crate) chars: Range<usize>,
    pub(crate) top: f32,
    pub(crate) height: f32,
    pub(crate) source_start: usize,
    /// 与原文位置无关的内容哈希，用于后台重解析完成后快速证明其余项内容未变。
    pub(crate) content_hash: Option<u64>,
}

/// 长列表原位修补索引。
pub(crate) struct ListPatch {
    pub(crate) start: Option<u64>,
    pub(crate) width: f32,
    pub(crate) base: f32,
    pub(crate) color: Color,
    pub(crate) live_item: Option<usize>,
    pub(crate) live_delta: isize,
    pub(crate) entries: Vec<ListEntry>,
}

/// 长表格单行的布局切片元数据。
pub(crate) struct TableRowEntry {
    pub(crate) items: Range<u32>,
    pub(crate) text_bytes: Range<usize>,
    pub(crate) chars: Range<usize>,
    pub(crate) top: f32,
    pub(crate) height: f32,
    pub(crate) source_start: usize,
    /// 与原文位置无关的行内容哈希，用于后台重解析完成后快速证明其余行内容未变。
    pub(crate) content_hash: u64,
}

/// 长表格原位修补索引。
pub(crate) struct TablePatch {
    pub(crate) head: bool,
    pub(crate) aligns: Box<[Align]>,
    pub(crate) width: f32,
    pub(crate) base: f32,
    pub(crate) color: Color,
    pub(crate) widths: Vec<f32>,
    pub(crate) total: f32,
    pub(crate) pad_x: f32,
    pub(crate) pad_y: f32,
    pub(crate) live_cell: Option<(usize, usize)>,
    pub(crate) live_delta: isize,
    pub(crate) col_bounds: Arc<crate::memo::TableCols>,
    pub(crate) row_entries: Vec<TableRowEntry>,
    pub(crate) lines_start: u32,
}

impl PatchIndex {
    /// 按原文字节偏移（相对顶层块）返回可能包含该偏移的局部项范围，用于加速超大块光标定位。
    pub(crate) fn source_item_window(&self, source: usize) -> Option<Range<usize>> {
        match self {
            PatchIndex::CodeBox(p) => {
                if p.lines.is_empty() {
                    return None;
                }
                let k = p
                    .lines
                    .partition_point(|l| l.source_start as usize <= source)
                    .saturating_sub(1);
                let lo = k.saturating_sub(1);
                let hi = (k + 1).min(p.lines.len() - 1);
                let start = p.lines[lo].items.start as usize;
                let end = p.lines[hi].items.end as usize;
                (start <= end).then_some(start..end)
            }
            PatchIndex::List(p) => {
                if p.entries.is_empty() {
                    return None;
                }
                let live = p.live_item;
                let delta = p.live_delta;
                let entry_src = |i: usize| {
                    let s = p.entries[i].source_start;
                    if live.is_some_and(|l| i > l) && delta != 0 {
                        s.saturating_add_signed(delta)
                    } else {
                        s
                    }
                };
                // 用二分找第一个 `entry_src(i) > source` 的位置。
                let mut lo = 0usize;
                let mut hi = p.entries.len();
                while lo < hi {
                    let mid = lo + (hi - lo) / 2;
                    if entry_src(mid) <= source {
                        lo = mid + 1;
                    } else {
                        hi = mid;
                    }
                }
                let k = lo.saturating_sub(1);
                let start = p.entries[k.saturating_sub(1)].items.start as usize;
                let end = p.entries[(k + 1).min(p.entries.len() - 1)].items.end as usize;
                (start <= end).then_some(start..end)
            }
            PatchIndex::Table(p) => {
                if p.row_entries.is_empty() {
                    return None;
                }
                let live_row = p.live_cell.map(|(r, _)| r);
                let delta = p.live_delta;
                let row_src = |i: usize| {
                    let s = p.row_entries[i].source_start;
                    if live_row.is_some_and(|lr| i > lr) && delta != 0 {
                        s.saturating_add_signed(delta)
                    } else {
                        s
                    }
                };
                let mut lo = 0usize;
                let mut hi = p.row_entries.len();
                while lo < hi {
                    let mid = lo + (hi - lo) / 2;
                    if row_src(mid) <= source {
                        lo = mid + 1;
                    } else {
                        hi = mid;
                    }
                }
                let k = lo.saturating_sub(1);
                let start = p.row_entries[k.saturating_sub(1)].items.start as usize;
                let end = p.row_entries[(k + 1).min(p.row_entries.len() - 1)]
                    .items
                    .end as usize;
                (start <= end).then_some(start..end)
            }
        }
    }
}

/// 返回块列表中首个可定位的原文偏移（相对顶层块）。
pub(crate) fn first_block_source(blocks: &[Block]) -> Option<usize> {
    for b in blocks {
        match b {
            Block::Paragraph(inline) | Block::Heading(_, inline) if !inline.map.is_empty() => {
                return Some(inline.source);
            }
            Block::Code { map, .. } => {
                if let Some(s) = map.source(0) {
                    return Some(s);
                }
            }
            Block::Rule(Some(s)) => return Some(*s),
            Block::Table(_, rows, _) => {
                if let Some(c) = rows.iter().flatten().find(|c| !c.map.is_empty()) {
                    return Some(c.source);
                }
            }
            Block::Math(_, s) => return Some(*s),
            Block::Quote(_, inner)
            | Block::Footnote(_, inner)
            | Block::Details { blocks: inner, .. } => {
                if let Some(s) = first_block_source(inner) {
                    return Some(s);
                }
            }
            Block::List(_, items) => {
                for it in items {
                    if let Some(s) = first_block_source(&it.blocks) {
                        return Some(s);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// 对 [`Inline`] 的可视内容（不含原文偏移与源码映射）计算轻量 FNV-1a 哈希。
fn hash_inline_content(inline: &Inline, h: &mut u64) {
    const PRIME: u64 = 1_099_511_628_211;
    let mix = |h: &mut u64, v: u64| {
        *h ^= v;
        *h = h.wrapping_mul(PRIME);
    };
    mix(h, inline.text.len() as u64);
    for &b in inline.text.as_bytes() {
        mix(h, u64::from(b));
    }
    mix(h, inline.runs.len() as u64);
    for r in &inline.runs {
        mix(h, r.end as u64);
        mix(h, u64::from(r.style.flags));
        mix(h, u64::from(r.style.link));
    }
    mix(h, inline.links.len() as u64);
    mix(h, inline.images.len() as u64);
}

/// 计算单条列表项与原文偏移无关的内容哈希（仅针对常见段落/标题项，嵌套复杂块返回 `None`）。
pub(crate) fn list_item_content_hash(item: &ListItem) -> Option<u64> {
    let mut h = 14_695_981_039_346_656_037u64;
    h ^= u64::from(item.bare);
    h = h.wrapping_mul(1_099_511_628_211);
    h ^= match item.task {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    };
    h = h.wrapping_mul(1_099_511_628_211);
    for b in &item.blocks {
        match b {
            Block::Paragraph(inline) => {
                h ^= 11;
                hash_inline_content(inline, &mut h);
            }
            Block::Heading(lvl, inline) => {
                h ^= 20 + u64::from(*lvl);
                hash_inline_content(inline, &mut h);
            }
            _ => return None,
        }
    }
    Some(h)
}

/// 计算单行表格与原文偏移无关的内容哈希。
pub(crate) fn table_row_content_hash(row: &[Inline]) -> u64 {
    let mut h = 14_695_981_039_346_656_037u64;
    for cell in row {
        hash_inline_content(cell, &mut h);
    }
    h
}

/// 返回围栏代码块正文（去掉首行围栏与可选末行围栏，零分配切片）。
pub(crate) fn fenced_body(text: &str) -> &str {
    match text.find('\n') {
        Some(nl) => {
            let rest = &text[nl + 1..];
            match rest.rfind('\n') {
                Some(last_nl) if is_fence(&rest[last_nl + 1..]) => &rest[..last_nl],
                None if is_fence(rest) => "",
                _ => rest,
            }
        }
        None => "",
    }
}

/// 把新排出的项中的字符偏移统一加上 `char_offset`。
fn offset_item_chars(items: &mut [Item], char_offset: usize) {
    if char_offset == 0 {
        return;
    }
    for item in items {
        match item {
            Item::Text {
                span: Some(span), ..
            } => span.start += char_offset,
            Item::Image {
                at: Some((c, _)), ..
            }
            | Item::Spot { at: (c, _), .. } => *c += char_offset,
            _ => {}
        }
    }
}

/// 把 `old_tb` 替换为 `replacement`，返回新的 `Box<str>`。
fn splice_text(text: &str, old_tb: Range<usize>, replacement: &str) -> Box<str> {
    let mut out = String::with_capacity(text.len() - old_tb.len() + replacement.len());
    out.push_str(&text[..old_tb.start]);
    out.push_str(replacement);
    out.push_str(&text[old_tb.end..]);
    out.into_boxed_str()
}

impl Cx<'_, '_> {
    /// 尝试对超大单块执行原位修补；成功时返回 `true`（直接修改 `laid`），
    /// 不满足原位修补条件时返回 `false` 交由 [`Cx::layout_block`] 完整重排。
    pub fn patch_block(
        &mut self,
        laid: &mut LaidBlock,
        block: &Block,
        same_parsed: bool,
        width: f32,
        env: Env,
        raw: Option<&Raw>,
    ) -> bool {
        let Some(raw) = raw else {
            return false;
        };
        if laid.is_shed() || laid.patch.is_none() {
            return false;
        }
        // 取出修补索引以便同时持有 `&mut laid` 的其余字段，结束后归还。
        let Some(mut patch) = laid.patch.take() else {
            return false;
        };
        self.text.clear();
        self.chars = 0;
        self.raw = None;
        self.cell = None;
        self.preedit = false;
        self.memo.take_touched();

        let ok = match (&mut *patch, block) {
            (PatchIndex::CodeBox(p), Block::Code { .. }) => {
                self.patch_code_box(laid, p, raw, width)
            }
            (PatchIndex::List(p), Block::List(start, items)) => {
                self.patch_list(laid, p, same_parsed, *start, items, width, env, raw)
            }
            (PatchIndex::Table(p), Block::Table(aligns, rows, head)) => {
                self.patch_table(laid, p, same_parsed, aligns, rows, *head, width, env, raw)
            }
            _ => false,
        };

        self.raw = None;
        self.cell = None;
        self.memo.take_touched();
        laid.patch = Some(patch);
        ok
    }

    /// 围栏代码块单行编辑原位修补。
    fn patch_code_box(
        &mut self,
        laid: &mut LaidBlock,
        p: &mut CodeBoxPatch,
        raw: &Raw,
        width: f32,
    ) -> bool {
        if !matches!(&raw.kind, RawKind::Fenced(l) if **l == *p.lang)
            || (p.width - width).abs() >= 0.01
            || p.leaf_start != raw.leaf.start
            || !blanks(raw).is_empty()
            || p.lines.len() < 3
            || p.lines[0].shown
            || p.lines.last().is_some_and(|l| l.shown)
        {
            return false;
        }
        let Some(code_idx) = p.code_item.map(|i| i as usize) else {
            return false;
        };
        let Some(old_body) = (match laid.items.get(code_idx) {
            Some(Item::Code { text, .. }) => Some(Arc::clone(text)),
            _ => None,
        }) else {
            return false;
        };
        let Some(raw_text) = raw.block.get(raw.leaf.clone()) else {
            return false;
        };
        let trimmed = &raw_text[..raw_text.trim_end().len()];
        let Some(open_nl) = trimmed.find('\n') else {
            return false;
        };
        let Some(close_nl) = trimmed.rfind('\n') else {
            return false;
        };
        if close_nl <= open_nl || !is_fence(&trimmed[close_nl + 1..]) {
            return false;
        }
        let rel_caret = raw.caret.saturating_sub(p.leaf_start);
        // 光标进入首尾围栏行时围栏会显露，退回完整重排。
        if rel_caret <= open_nl || rel_caret > close_nl {
            return false;
        }
        let ds = trimmed.len() as isize - p.raw_len as isize;
        let new_body = fenced_body(trimmed);
        if new_body.len() as isize - old_body.len() as isize != ds {
            return false;
        }
        // 二分查找光标所在行（必须在首尾围栏之间的非围栏行）。
        let mut k = p
            .lines
            .partition_point(|l| l.source_start as usize <= raw.caret)
            .saturating_sub(1);
        if k == 0 || k + 1 >= p.lines.len() {
            return false;
        }
        if !p.lines[k].shown || p.lines[k].fence {
            return false;
        }
        let mut s_k = p.lines[k].source_start as usize - p.leaf_start;
        if rel_caret < s_k && k > 1 {
            k -= 1;
            s_k = p.lines[k].source_start as usize - p.leaf_start;
        }
        if k == 0 || k + 1 >= p.lines.len() || p.lines[k].fence || !p.lines[k].shown {
            return false;
        }
        let old_e_k = p.lines[k].source_end as usize - p.leaf_start;
        let Some(new_e_k) = old_e_k.checked_add_signed(ds) else {
            return false;
        };
        if new_e_k < s_k || new_e_k >= trimmed.len() {
            return false;
        }
        if s_k == 0 || trimmed.as_bytes()[s_k - 1] != b'\n' || trimmed.as_bytes()[new_e_k] != b'\n'
        {
            return false;
        }
        if trimmed[s_k..new_e_k].contains('\n') {
            return false;
        }
        let body_off = open_nl + 1;
        if s_k < body_off || old_e_k < body_off || new_e_k < body_off {
            return false;
        }
        let (bs, old_be, new_be) = (s_k - body_off, old_e_k - body_off, new_e_k - body_off);
        if old_be > old_body.len() || new_be > new_body.len() {
            return false;
        }
        // 通过前后缀字节比对（memcmp）严格证明除第 `k` 行外未发生任何其他改动。
        if old_body.as_bytes()[..bs] != new_body.as_bytes()[..bs]
            || old_body.as_bytes()[old_be..] != new_body.as_bytes()[new_be..]
        {
            return false;
        }

        // 重排第 `k` 行；若行末词法状态变化，继续重排紧随其后的行直至状态收敛（最多 32 行）。
        let language = winc_syntax::find(&p.lang);
        let mut state = p.lines[k - 1].end_state;
        let mut new_items: Vec<Item> = Vec::new();
        let mut new_spans: Vec<CodeLineSpan> = Vec::new();
        let mut top = p.lines[k].top;
        let mut end_k = k;
        let max_end = (k + 32).min(p.lines.len() - 2);
        let mut cur_s = s_k;
        let mut cur_e = new_e_k;
        loop {
            let line_str = &trimmed[cur_s..cur_e];
            let source = u32::try_from(p.leaf_start + cur_s).unwrap_or(0);
            let item_start = new_items.len() as u32;
            let tb_start = self.text.len();
            let ch_start = self.chars;
            let mut line_items = Vec::new();
            let flow = self.code_line(
                line_str,
                Some(source),
                language,
                &mut state,
                p.row,
                &mut line_items,
            );
            for item in line_items {
                new_items.push(item.moved(p.pad, top, 0));
            }
            let item_end = new_items.len() as u32;
            let tb_end = self.text.len();
            let ch_end = self.chars;
            new_spans.push(CodeLineSpan {
                items: item_start..item_end,
                text_bytes: tb_start..tb_end,
                chars: ch_start..ch_end,
                source_start: source,
                source_end: u32::try_from(p.leaf_start + cur_e).unwrap_or(0),
                top,
                height: flow.height,
                end_state: state,
                fence: false,
                shown: true,
            });
            top += flow.height;
            if state == p.lines[end_k].end_state {
                break;
            }
            if end_k >= max_end {
                return false;
            }
            end_k += 1;
            if p.lines[end_k].fence || !p.lines[end_k].shown {
                return false;
            }
            cur_s = (p.lines[end_k].source_start as isize + ds) as usize - p.leaf_start;
            cur_e = (p.lines[end_k].source_end as isize + ds) as usize - p.leaf_start;
        }

        let old_items_range = p.lines[k].items.start as usize..p.lines[end_k].items.end as usize;
        let old_tb = p.lines[k].text_bytes.start..p.lines[end_k].text_bytes.end;
        let old_ch = p.lines[k].chars.start..p.lines[end_k].chars.end;
        let old_bottom = p.lines[end_k].top + p.lines[end_k].height;
        let dy = top - old_bottom;
        let di = new_items.len() as isize - old_items_range.len() as isize;
        let dc = self.chars as isize - old_ch.len() as isize;
        let dtb = self.text.len() as isize - old_tb.len() as isize;

        offset_item_chars(&mut new_items, old_ch.start);
        let new_len = new_items.len();
        let new_end = old_items_range.start + new_len;
        let same_shape = dy == 0.0
            && di == 0
            && laid.items[old_items_range.clone()]
                .iter()
                .zip(&new_items)
                .all(|(a, b)| a.span_y() == b.span_y());

        if di == 0 {
            for (slot, item) in laid.items[old_items_range.clone()]
                .iter_mut()
                .zip(new_items)
            {
                *slot = item;
            }
        } else {
            laid.items.splice(old_items_range.clone(), new_items);
        }

        let new_code_idx = (code_idx as isize + di) as usize;
        if dy != 0.0 || dc != 0 || ds != 0 {
            for item in &mut laid.items[new_end..new_code_idx] {
                item.shift_in_place(dy, dc, ds, true);
            }
        }
        if dy != 0.0 {
            if let Some(Item::Fill { rect, .. }) = laid.items.get_mut(p.bg_item as usize) {
                rect.h += dy;
            }
            laid.height += dy;
        }
        if let Some(Item::Code { rect, text, .. }) = laid.items.get_mut(new_code_idx) {
            rect.h += dy;
            *text = new_body.into();
        }
        p.code_item = Some(new_code_idx as u32);
        laid.text = splice_text(&laid.text, old_tb.clone(), &self.text);
        laid.chars = (laid.chars as isize + dc) as usize;

        if !same_shape {
            laid.patch_spatial_index(old_items_range.clone(), new_len, dy);
        }

        let base_item = old_items_range.start as u32;
        let base_tb = old_tb.start;
        let base_ch = old_ch.start;
        for (idx, span) in new_spans.into_iter().enumerate() {
            let line = &mut p.lines[k + idx];
            line.items = (base_item + span.items.start)..(base_item + span.items.end);
            line.text_bytes = (base_tb + span.text_bytes.start)..(base_tb + span.text_bytes.end);
            line.chars = (base_ch + span.chars.start)..(base_ch + span.chars.end);
            line.source_start = span.source_start;
            line.source_end = span.source_end;
            line.top = span.top;
            line.height = span.height;
            line.end_state = span.end_state;
        }
        if di != 0 || dtb != 0 || dc != 0 || ds != 0 || dy != 0.0 {
            for line in &mut p.lines[end_k + 1..] {
                line.items =
                    (line.items.start as isize + di) as u32..(line.items.end as isize + di) as u32;
                line.text_bytes = (line.text_bytes.start as isize + dtb) as usize
                    ..(line.text_bytes.end as isize + dtb) as usize;
                line.chars = (line.chars.start as isize + dc) as usize
                    ..(line.chars.end as isize + dc) as usize;
                line.source_start = (line.source_start as isize + ds).max(0) as u32;
                line.source_end = (line.source_end as isize + ds).max(0) as u32;
                line.top += dy;
            }
        }
        p.raw_len = trimmed.len();
        true
    }

    /// 长列表单项实时编辑原位修补（兼容后台重解析完成后的跨版本接续修补）。
    #[allow(clippy::too_many_arguments)]
    fn patch_list(
        &mut self,
        laid: &mut LaidBlock,
        p: &mut ListPatch,
        same_parsed: bool,
        start: Option<u64>,
        list_items: &[ListItem],
        width: f32,
        env: Env,
        raw: &Raw,
    ) -> bool {
        if p.start != start
            || list_items.len() != p.entries.len()
            || (p.width - width).abs() >= 0.01
            || (p.base - self.base).abs() >= 0.01
            || p.color != env.color
        {
            return false;
        }
        let Some(k) = p.live_item else {
            return false;
        };
        if k >= list_items.len() {
            return false;
        }
        // 后台重解析完成时，验证除第 `k` 项外所有项的可视内容哈希未变，并计算后续项的解析偏移增量。
        let parsed_delta = if same_parsed {
            0isize
        } else {
            for (i, item) in list_items.iter().enumerate() {
                if i == k {
                    continue;
                }
                let Some(expected) = p.entries[i].content_hash else {
                    return false;
                };
                if list_item_content_hash(item) != Some(expected) {
                    return false;
                }
            }
            let Some(new_k_src) = first_block_source(&list_items[k].blocks) else {
                return false;
            };
            if new_k_src != p.entries[k].source_start {
                return false;
            }
            if k + 1 < list_items.len() {
                let Some(new_next) = first_block_source(&list_items[k + 1].blocks) else {
                    return false;
                };
                new_next as isize - p.entries[k + 1].source_start as isize
            } else {
                0
            }
        };
        // 验证当前编辑叶子仍在第 `k` 项内。
        if raw.leaf.start < p.entries[k].source_start {
            return false;
        }
        if k + 1 < p.entries.len()
            && raw.parsed_end > (p.entries[k + 1].source_start as isize + parsed_delta) as usize
        {
            return false;
        }

        let base = self.base;
        let indent = base * 2.0;
        let content_x = indent;
        let room = (width - indent).max(1.0);
        let inner = Env {
            depth: env.depth + 1,
            ..env
        };
        let item = &list_items[k];
        let item_top = p.entries[k].top;
        let (item_x, item_room) = if item.bare {
            (0.0, width)
        } else {
            (content_x, room)
        };

        let mut item_out = Out::default();
        self.raw = Some(raw.clone());
        let flow = self.blocks(
            &item.blocks,
            item_x,
            item_top,
            item_room,
            inner,
            &mut item_out,
        );
        let consumed = self.raw.as_ref().is_some_and(|r| r.done);
        if !consumed
            || !item_out.anchors.is_empty()
            || !item_out.scrolls.is_empty()
            || item_out.editing_blank_height > 0.0
        {
            return false;
        }
        let baseline = flow.first_baseline;
        match (item.task, start) {
            _ if item.bare => {}
            (Some(checked), _) => {
                let size = (base * 0.85).round();
                let line_mid = baseline.map_or(item_top + base * 0.75, |b| b - base * 0.35);
                item_out.items.push(Item::Check {
                    rect: Rect::new(
                        content_x - size - base * 0.45,
                        line_mid - size / 2.0,
                        size,
                        size,
                    ),
                    checked,
                    source: u32::try_from(item.task_source).unwrap_or(u32::MAX),
                });
            }
            (None, Some(start)) => {
                let marker = format!("{}.", start + k as u64);
                item_out
                    .items
                    .push(self.marker(&marker, content_x, item_top, baseline, env.color));
            }
            (None, None) => self.bullet(content_x, item_top, baseline, env, &mut item_out.items),
        }
        let new_h = flow.height.max(if item.blocks.is_empty() {
            (base * BODY_LINE).round()
        } else {
            0.0
        });

        let old_entry = &p.entries[k];
        let old_items_range = old_entry.items.start as usize..old_entry.items.end as usize;
        let old_tb = old_entry.text_bytes.clone();
        let old_ch = old_entry.chars.clone();
        let dy = new_h - old_entry.height;
        let new_items_len = item_out.items.len();
        let di = new_items_len as isize - old_items_range.len() as isize;
        let dc = self.chars as isize - old_ch.len() as isize;
        let dtb = self.text.len() as isize - old_tb.len() as isize;
        let new_live_delta = raw.leaf.end as isize - raw.parsed_end as isize;
        let ds = (new_live_delta + parsed_delta) - p.live_delta;

        offset_item_chars(&mut item_out.items, old_ch.start);
        let new_end = old_items_range.start + new_items_len;
        let same_shape = dy == 0.0
            && di == 0
            && laid.items[old_items_range.clone()]
                .iter()
                .zip(&item_out.items)
                .all(|(a, b)| a.span_y() == b.span_y());

        if di == 0 {
            for (slot, it) in laid.items[old_items_range.clone()]
                .iter_mut()
                .zip(item_out.items)
            {
                *slot = it;
            }
        } else {
            laid.items.splice(old_items_range.clone(), item_out.items);
        }

        if dy != 0.0 || dc != 0 || ds != 0 {
            for it in &mut laid.items[new_end..] {
                it.shift_in_place(dy, dc, ds, false);
            }
        }
        laid.text = splice_text(&laid.text, old_tb.clone(), &self.text);
        laid.chars = (laid.chars as isize + dc) as usize;
        laid.height += dy;
        laid.preedit = self.preedit;

        if !same_shape {
            laid.patch_spatial_index(old_items_range.clone(), new_items_len, dy);
        }

        let entry = &mut p.entries[k];
        entry.items = old_items_range.start as u32..new_end as u32;
        entry.text_bytes = old_tb.start..(old_tb.start + self.text.len());
        entry.chars = old_ch.start..(old_ch.start + self.chars);
        entry.height = new_h;
        entry.content_hash = list_item_content_hash(item);
        if di != 0 || dtb != 0 || dc != 0 || dy != 0.0 || parsed_delta != 0 {
            for e in &mut p.entries[k + 1..] {
                e.items = (e.items.start as isize + di) as u32..(e.items.end as isize + di) as u32;
                e.text_bytes = (e.text_bytes.start as isize + dtb) as usize
                    ..(e.text_bytes.end as isize + dtb) as usize;
                e.chars =
                    (e.chars.start as isize + dc) as usize..(e.chars.end as isize + dc) as usize;
                e.top += dy;
                e.source_start = e.source_start.saturating_add_signed(parsed_delta);
            }
        }
        p.live_delta = new_live_delta;
        let _ = LIST_GAP;
        true
    }

    /// 长表格单单元格实时编辑原位修补（支持后台重解析接续与未折行单列变宽时的 O(N) 平移）。
    #[allow(clippy::too_many_arguments)]
    fn patch_table(
        &mut self,
        laid: &mut LaidBlock,
        p: &mut TablePatch,
        same_parsed: bool,
        aligns: &[Align],
        rows: &[Vec<Inline>],
        head: bool,
        width: f32,
        env: Env,
        raw: &Raw,
    ) -> bool {
        if head != p.head
            || aligns != &*p.aligns
            || rows.len() != p.row_entries.len()
            || (p.width - width).abs() >= 0.01
            || (p.base - self.base).abs() >= 0.01
            || p.color != env.color
        {
            return false;
        }
        self.raw = Some(raw.clone());
        let Some(cell) = self.live_cell(rows) else {
            return false;
        };
        let (lr, lc) = (cell.0, cell.1);
        if p.live_cell != Some((lr, lc)) || lr >= rows.len() || lc >= p.widths.len() {
            return false;
        }
        // 后台重解析完成时，验证除第 `lr` 行外所有行内容哈希未变，并计算后续行的解析偏移增量。
        let parsed_delta = if same_parsed {
            0isize
        } else {
            for (r, row) in rows.iter().enumerate() {
                if r == lr {
                    continue;
                }
                if table_row_content_hash(row) != p.row_entries[r].content_hash {
                    return false;
                }
            }
            if lr + 1 < rows.len() {
                let Some(new_next) = rows[lr + 1]
                    .iter()
                    .find(|c| !c.map.is_empty())
                    .map(|c| c.source)
                else {
                    return false;
                };
                new_next as isize - p.row_entries[lr + 1].source_start as isize
            } else {
                0
            }
        };
        self.cell = Some(cell);
        let live = self.caret_cell();
        let base = (self.base * TABLE_TEXT).round();
        let cols = p.widths.len();
        let para = |header: bool, col: usize| Para {
            size: base,
            bold: header,
            color: env.color,
            line: 1.5,
            align: aligns.get(col).copied().unwrap_or(Align::None),
            keep_spaces: false,
        };
        let (min, max) = self.apply_live_col_bounds(&p.col_bounds, cols, &live, p.pad_x, |r, c| {
            para(head && r == 0, c)
        });
        let new_widths = crate::table::column_widths(&min, &max, width);
        let new_total: f32 = new_widths.iter().sum::<f32>() + 1.0;

        // 判断列宽变化是否属于安全的单列宽度微调：仅编辑列 `lc` 宽度变化、表格未溢出、
        // 该列左对齐且前后均不触发其余行折行，此时其余行只需将右侧列整体横移 `dx`。
        let col_dx = new_widths[lc] - p.widths[lc];
        let widths_changed = new_widths != p.widths;
        if widths_changed {
            let only_lc = new_widths
                .iter()
                .zip(&p.widths)
                .enumerate()
                .all(|(c, (nw, ow))| c == lc || (*nw - *ow).abs() < 0.001);
            let left_aligned = matches!(
                aligns.get(lc).copied().unwrap_or(Align::None),
                Align::None | Align::Left
            );
            let unwrapped_before = p.total <= width + 0.5 && max.iter().sum::<f32>() <= width;
            let unwrapped_after = new_total <= width + 0.5;
            if !(only_lc && left_aligned && unwrapped_before && unwrapped_after) {
                return false;
            }
        }

        let old_col_right: f32 = p.widths[..=lc].iter().sum();
        let new_live_delta = raw.leaf.end as isize - raw.parsed_end as isize;
        let row_top = p.row_entries[lr].top;
        let mut row_items: Vec<Item> = Vec::new();
        let (new_row_h, _) = self.table_row(
            lr,
            &rows[lr],
            head,
            aligns,
            &new_widths,
            new_total,
            0.0,
            row_top,
            base,
            p.pad_x,
            p.pad_y,
            env,
            &live,
            new_live_delta,
            &mut row_items,
        );

        let old_row = &p.row_entries[lr];
        let old_items_range = old_row.items.start as usize..old_row.items.end as usize;
        let old_tb = old_row.text_bytes.clone();
        let old_ch = old_row.chars.clone();
        let dy = new_row_h - old_row.height;
        let new_len = row_items.len();
        let di = new_len as isize - old_items_range.len() as isize;
        let dc = self.chars as isize - old_ch.len() as isize;
        let dtb = self.text.len() as isize - old_tb.len() as isize;
        let ds = (new_live_delta + parsed_delta) - p.live_delta;

        offset_item_chars(&mut row_items, old_ch.start);
        let new_end = old_items_range.start + new_len;
        let same_shape = dy == 0.0
            && di == 0
            && laid.items[old_items_range.clone()]
                .iter()
                .zip(&row_items)
                .all(|(a, b)| a.span_y() == b.span_y());

        if di == 0 {
            for (slot, it) in laid.items[old_items_range.clone()]
                .iter_mut()
                .zip(row_items)
            {
                *slot = it;
            }
        } else {
            laid.items.splice(old_items_range.clone(), row_items);
        }

        let new_lines_start = (p.lines_start as isize + di) as usize;
        // 若编辑列 `lc` 宽度变化，把其余行中位于 `lc` 右侧的所有项横向平移 `col_dx`。
        if widths_changed && col_dx != 0.0 {
            for it in &mut laid.items[..old_items_range.start] {
                if !matches!(it, Item::Corners { .. }) && it.span_x().0 >= old_col_right - 0.5 {
                    it.shift_x(col_dx);
                }
            }
            if head
                && lr != 0
                && let Some(Item::Corners { rect, .. }) = laid.items.first_mut()
            {
                rect.w = new_total;
            }
        }
        if dy != 0.0 || dc != 0 || ds != 0 || (widths_changed && col_dx != 0.0) {
            for it in &mut laid.items[new_end..new_lines_start] {
                if widths_changed && col_dx != 0.0 && it.span_x().0 >= old_col_right - 0.5 {
                    it.shift_x(col_dx);
                }
                if dy != 0.0 || dc != 0 || ds != 0 {
                    it.shift_in_place(dy, dc, ds, false);
                }
            }
        }
        let divider_count = rows.len().saturating_sub(1);
        let frame_idx = new_lines_start + divider_count;
        let table_idx = frame_idx + 1;
        if widths_changed && col_dx != 0.0 {
            for div in &mut laid.items[new_lines_start..frame_idx] {
                if let Item::Fill { rect, .. } = div {
                    rect.w = new_total - 2.0;
                }
            }
        }
        if dy != 0.0 {
            // 行 `lr` 之后的横线（第 `lr` 条起）向下平移 `dy`。
            for div in &mut laid.items[new_lines_start + lr..frame_idx] {
                div.shift_in_place(dy, 0, 0, false);
            }
        }
        if dy != 0.0 || widths_changed {
            let new_table_h = laid.height + dy
                - if new_total > width + 0.5 {
                    SCROLL_BAR_ROOM
                } else {
                    0.0
                };
            if let Some(frame_item) = laid.items.get_mut(frame_idx) {
                *frame_item = Item::path(
                    rounded_frame(Rect::new(0.0, 0.0, new_total, new_table_h), TABLE_RADIUS),
                    1.0,
                    self.colors.table_frame,
                );
            }
            if let Some(Item::Table {
                rect,
                cols: cwidths,
                rows: rh,
                ..
            }) = laid.items.get_mut(table_idx)
            {
                rect.w = new_total;
                rect.h = new_table_h;
                if widths_changed {
                    *cwidths = new_widths.clone().into();
                }
                if dy != 0.0 {
                    let mut heights: Vec<f32> = rh.to_vec();
                    heights[lr] = new_row_h;
                    *rh = heights.into();
                }
            }
        }
        for scroller in &mut laid.scrolls {
            scroller.items.end = (scroller.items.end as isize + di) as u32;
            scroller.rect.h += dy;
            scroller.content = new_total;
        }
        laid.text = splice_text(&laid.text, old_tb.clone(), &self.text);
        laid.chars = (laid.chars as isize + dc) as usize;
        laid.height += dy;
        laid.preedit = self.preedit;

        if !same_shape {
            laid.patch_spatial_index(old_items_range.clone(), new_len, dy);
        }

        let entry = &mut p.row_entries[lr];
        entry.items = old_items_range.start as u32..new_end as u32;
        entry.text_bytes = old_tb.start..(old_tb.start + self.text.len());
        entry.chars = old_ch.start..(old_ch.start + self.chars);
        entry.height = new_row_h;
        entry.content_hash = table_row_content_hash(&rows[lr]);
        if di != 0 || dtb != 0 || dc != 0 || dy != 0.0 || parsed_delta != 0 {
            for e in &mut p.row_entries[lr + 1..] {
                e.items = (e.items.start as isize + di) as u32..(e.items.end as isize + di) as u32;
                e.text_bytes = (e.text_bytes.start as isize + dtb) as usize
                    ..(e.text_bytes.end as isize + dtb) as usize;
                e.chars =
                    (e.chars.start as isize + dc) as usize..(e.chars.end as isize + dc) as usize;
                e.top += dy;
                e.source_start = e.source_start.saturating_add_signed(parsed_delta);
            }
        }
        // 若后台重解析已完成，顺带把 `col_bounds` 中当前单元格的极值更新到最新解析结果。
        if !same_parsed {
            let mut updated = (*p.col_bounds).clone();
            if let Some(cell) = rows[lr].get(lc) {
                let (lo, hi) = self.measure(cell, para(head && lr == 0, lc));
                let (lo, hi) = (lo + p.pad_x * 2.0, hi + p.pad_x * 2.0);
                if updated.min[lc].1 == lr {
                    if lo >= updated.min[lc].2 {
                        updated.min[lc].0 = lo;
                    } else {
                        updated.min[lc] = (updated.min[lc].2, usize::MAX, lo);
                    }
                } else if lo >= updated.min[lc].0 {
                    updated.min[lc] = (lo, lr, updated.min[lc].0);
                } else if lo > updated.min[lc].2 {
                    updated.min[lc].2 = lo;
                }
                if updated.max[lc].1 == lr {
                    if hi >= updated.max[lc].2 {
                        updated.max[lc].0 = hi;
                    } else {
                        updated.max[lc] = (updated.max[lc].2, usize::MAX, hi);
                    }
                } else if hi >= updated.max[lc].0 {
                    updated.max[lc] = (hi, lr, updated.max[lc].0);
                } else if hi > updated.max[lc].2 {
                    updated.max[lc].2 = hi;
                }
            }
            p.col_bounds = Arc::new(updated);
        }
        p.widths = new_widths;
        p.total = new_total;
        p.lines_start = new_lines_start as u32;
        p.live_delta = new_live_delta;
        true
    }
}
