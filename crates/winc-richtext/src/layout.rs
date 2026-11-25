//! Markdown 预览的排版结果与位置查询。
//!
//! [`Laid`] 是整篇文档的排版结果，由 [`Laid::build`] 按顶层块生成并缓存：文字变化时只重排变化的
//! 块，宽度、字号、配色或图片大小变化时整篇重排。位置查询在预览坐标与原文位置之间换算，
//! 供点击、选择、滚动同步和可编辑的预览使用。本模块不依赖 `App`，绘制在
//! `view::preview::markdown` 中。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use winc_render::{Color, Frame, Rect};
use winc_typeset::{Colors, Cx, Env, ImageSlot, Item, LaidBlock, Raw, Span, margins};

pub use winc_typeset::{DETAILS, Memo};

/// 正文字号与编辑器字号之比：编辑器字号 13 逻辑像素时，正文为 16 逻辑像素。
pub const BASE_RATIO: f32 = 16.0 / 13.0;
/// 正文宽度为「标准」（`markdown.textWidth`）时正文的最大宽度（逻辑像素，经验值）：更宽时居中、
/// 两侧留白。
pub const MAX_WIDTH: f32 = 720.0;
/// 「较宽」给表格和双语长段留出空间，保留两侧页边距。
pub const WIDE_WIDTH: f32 = 1040.0;
/// 区域不窄于 600 逻辑像素时文档四周的留白（逻辑像素，经验值）。
pub const PAD: f32 = 32.0;
/// 窄的时候（分栏，区域宽不足 600 逻辑像素）两边的留白（逻辑像素，经验值）。
pub const PAD_NARROW: f32 = 20.0;

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
    /// Source-addressable line boxes in the spaces between formatted blocks.
    gaps: Vec<BlankGap>,
    blank_offsets: Arc<[usize]>,
    /// 正文字体单个空格的宽度（逻辑像素），用于空行上键入空格时的光标定位与命中。
    space_width: f32,
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
}

#[derive(Clone, Copy)]
struct CachedCaret {
    source: usize,
    upstream: bool,
    point: Option<Point>,
}

/// One run of blank lines, with equally spaced hit targets and real source offsets.
struct BlankGap {
    top: f32,
    row: f32,
    offsets: std::ops::Range<usize>,
    /// 各空行在换行符之前已有的水平空白字节数（用于支持空行上键入空格）。
    row_cols: Vec<usize>,
    /// 位于该顶层块之前；文末空行为 `None`。
    before_block: Option<usize>,
    /// 是否位于文档末尾（下方没有后续块，末行光标可按正文完整高度绘制）。
    trailing: bool,
}

impl BlankGap {
    fn new(
        offsets: std::ops::Range<usize>,
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
                    let s = r.byte_slice(start..next).to_string();
                    s.trim_end_matches(['\r', '\n']).len()
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

/// 分帧排版的依据：排版条件变化而文字未变时，视口内的块立即重排，其余的块在超过时限后沿用
/// 旧结果，由之后的各帧继续重排。
pub struct Later<'a> {
    /// 旧的各顶层块，与本次排版的各块一一对应。
    pub old: &'a [(u64, Arc<LaidBlock>)],
    /// 必须按当前条件重排的块，即视口内的块。
    pub window: std::ops::Range<usize>,
    /// 截止时刻：此后 `window` 之外的块沿用旧结果。
    pub deadline: std::time::Instant,
}

/// 顶层块超过该数量（块）的文档只保留 [`Near`] 范围内的块的排版结果，其余的块释放要画的项
/// （见 [`LaidBlock::shed`]），排版缓存也只保留这些块用到的条目。
///
/// 取自调参：512 块以内的文档全部保留时，排版结果与缓存合计不超过几 MB。在 Apple M 系列芯片上
/// 实测，2 万段、4 MB 的文档全部保留时二者占用 91 MB 堆内存。
pub const SHED_BLOCKS: usize = 512;

/// 需要保留排版结果的块：内容坐标在 `rows` 范围内的块，以及序号在 `blocks` 范围内的块（光标和
/// 选区所在的块及其前后）。
#[derive(Debug, Clone, PartialEq)]
pub struct Near {
    /// 需要保留的内容纵坐标范围 `(top, bottom)`（逻辑像素）。
    pub rows: (f32, f32),
    /// 需要保留的顶层块序号区间列表。
    pub blocks: Vec<std::ops::Range<usize>>,
}

/// 光标所在的块前后各保留排版结果的块数。取自调参：32 块超过一屏能显示的段数，光标上下移动、
/// 翻页时落到的块都已排好。
pub const NEAR_BLOCKS: usize = 32;

impl Near {
    /// 按本范围保留的排版结果是否够 `wanted` 使用：`wanted` 的各块都在其中，视口（高 `view`）
    /// 前后各有半屏以上。
    pub fn covers(&self, wanted: &Near, view: f32) -> bool {
        let rows = (wanted.rows.0 + view * 1.5, wanted.rows.1 - view * 1.5);
        let inside = |want: &std::ops::Range<usize>| {
            (self.blocks.iter()).any(|kept| kept.start <= want.start && want.end <= kept.end)
        };
        self.rows.0 <= rows.0 && rows.1 <= self.rows.1 && wanted.blocks.iter().all(inside)
    }

    /// 上边缘在 `top`、高 `height` 的第 `i` 块是否在范围内。
    fn holds(&self, i: usize, top: f32, height: f32) -> bool {
        (top < self.rows.1 && top + height > self.rows.0)
            || self.blocks.iter().any(|blocks| blocks.contains(&i))
    }
}

/// 一帧中排版 [`Later::window`] 之外的块的时限：4 毫秒，约为 60 Hz 下一帧间隔的四分之一。
pub const LAYOUT_BUDGET: std::time::Duration = std::time::Duration::from_millis(4);

/// 预览上的一个位置：左边缘（相对内容左边）、所在那一行铺底色的上边缘（和滚动位置同一个
/// 量法）和高度，均为逻辑像素；见 [`Laid::point_of`]。
pub type Point = (f32, f32, f32);

/// 决定排版结果的外部条件：任何一项变了都要整份重排。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Key {
    /// 正文排版区域宽度（逻辑像素）。
    pub width: f32,
    /// 正文字号（逻辑像素）。
    pub base: f32,
    /// 正文字色。
    pub text: Color,
    /// 背景底色。
    pub background: Color,
    /// 图片池版本号，变化时触发图片尺寸重排。
    pub images: u64,
    /// 代码块是否显示行号。
    pub numbers: bool,
    /// 预览视口高度，用于长图/公式比例控制。
    pub tall: f32,
}

/// 淡入的项：所在顶层块的序号及块内各项的序号，按块的序号排序。
pub type Fading = Vec<(usize, Box<[usize]>)>;

/// 重排后各块从原位置到新位置的过渡。
pub struct Moves {
    /// 各顶层块起始时相对新位置向下的偏移（逻辑像素）。
    pub from: Vec<f32>,
    /// 需要淡入的新增标记项。
    pub fading: Fading,
    /// 刚显示原文的块中渲染结果的首项序号及纵向偏移。
    pub inner: Vec<(usize, u32, f32)>,
    /// 刚恢复排版的块原来的原文残留项。
    pub leaving: Vec<Leaving>,
}

/// 重排之前各顶层块的排版和绘制位置，重排后据此计算过渡动画（见 `BlockMotion`）。
pub struct Before {
    blocks: Vec<(u64, Arc<LaidBlock>, f32)>,
    revision: Option<u64>,
}

/// 刚恢复排版的块原来显示的原文。
pub struct Leaving {
    /// 所属顶层块序号。
    pub block: usize,
    /// 恢复排版前的旧排版块。
    pub old: Arc<LaidBlock>,
    /// 旧排版块中需要淡出的项数。
    pub items: u32,
    /// 淡出时的纵向位置偏移。
    pub offset: f32,
}

/// 可编辑的预览中显示原文的块的缓存键；各字段均不变时沿用上一次的排版。
pub type LiveKey = (usize, std::ops::Range<usize>, u64, Option<usize>, String);

/// 可编辑的预览中光标所在的叶子块（见 [`winc_preview::markdown::Markdown::leaves`]），该块显示原文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveLeaf {
    /// 所在顶层块序号。
    pub block: usize,
    /// 所在顶层块在原文中的字节范围。
    pub range: std::ops::Range<usize>,
    /// 当前叶子块在原文中的字节范围。
    pub leaf: std::ops::Range<usize>,
    /// 上一版解析记录的叶子块末尾字节偏移。
    pub parsed_end: usize,
    /// 当前文档版本号。
    pub revision: u64,
    /// 是否为正文叶子块（段落、标题、块间空行）。
    pub prose: bool,
    /// 是否为表格叶子块。
    pub table: bool,
    /// 光标当前行是否为叶子块内的空白行。
    pub blank: bool,
    /// 光标在原文中的字节偏移。
    pub at: usize,
    /// 非正文块中光标位于首行或末行时的行号。
    pub edge: Option<usize>,
}

impl LiveLeaf {
    /// 构造本叶子块的缓存键；`preedit` 是输入法正在组合的文字，在正文中排入光标所在的行，
    /// 因此其变化同样需要重排。
    pub fn key(&self, preedit: &str) -> LiveKey {
        (
            self.block,
            self.leaf.clone(),
            self.revision,
            (self.table || self.prose).then_some(self.at).or(self.edge),
            if self.prose || self.table {
                preedit.to_owned()
            } else {
                String::new()
            },
        )
    }
}

/// [`Laid::build`] 所需的外部上下文：只传入排版真正依赖的原文、配色、图片池与缓存，
/// 不依赖顶层 `App`。
pub struct LayoutInput<'a> {
    /// 当前文档原文（用于光标所在叶子块的即时排版）。
    pub rope: Option<&'a ropey::Rope>,
    /// 输入法正在组合的预编辑文字。
    pub preedit: &'a str,
    /// 界面表层背景色。
    pub surface: Color,
    /// 强调色（用于选区、链接、勾选框等）。
    pub accent: Color,
    /// 排版配色表。
    pub colors: Colors,
    /// 是否为深色主题。
    pub dark: bool,
    /// 代码块语法高亮配色回调。
    pub token: &'a dyn Fn(winc_syntax::TokenKind) -> Color,
    /// 已加载/解码的图片槽位表。
    pub images: &'a HashMap<PathBuf, ImageSlot>,
    /// 将 Markdown 图片地址解析为本地路径的回调。
    pub image_path: &'a dyn Fn(&str) -> Option<PathBuf>,
    /// 跨帧复用的排版缓存。
    pub memo: &'a mut Memo,
    /// 用户手动切换过开合状态的 `<details>` 折叠块集合。
    pub toggled: &'a HashSet<u64>,
    /// 当前是否处于可编辑预览模式。
    pub editable: bool,
    /// 解析结果是否与当前文档版本一致。
    pub checked: bool,
    /// 顶部文档属性面板块（若存在）。
    pub properties: Option<winc_preview::markdown::Block>,
}

/// 返回基线在 `baseline`、字号 `size` 的一行字在选中、查找时铺底色的范围：上边缘和高度
/// （逻辑像素）。
///
/// 上边缘在基线之上 1.05 倍字号处，高度取 1.4 倍字号（经验值，且能盖住上下标）。
pub fn line_box(baseline: f32, size: f32) -> (f32, f32) {
    (baseline - size * 1.05, size * 1.4)
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
        // 光标所在顶层块现在的原文，和叶子块在其中的范围。
        let raw = live.and_then(|live| {
            let rope = input.rope?;
            let snap = |b: usize| rope.char_to_byte(rope.byte_to_char(b.min(rope.len_bytes())));
            let (start, end) = (snap(live.range.start), snap(live.range.end));
            let text: Arc<str> = rope.byte_slice(start..end).to_string().into();
            let leaf = snap(live.leaf.start).saturating_sub(start)
                ..snap(live.leaf.end).saturating_sub(start);
            let caret = snap(live.at).saturating_sub(start);
            let preedit = ((live.prose || live.table) && !input.preedit.is_empty())
                .then(|| Arc::from(input.preedit));
            let parsed_end = live.parsed_end.saturating_sub(start);
            let raw = Raw::new(text, leaf, parsed_end, caret, live.prose, preedit);
            Some((live.block, raw))
        });
        input.memo.prepare(winc_typeset::memo_key(|h| {
            use std::hash::Hash;
            for v in [key.width, key.base, key.tall] {
                v.to_bits().hash(h);
            }
            for c in [key.text, key.background] {
                [c.r, c.g, c.b, c.a].map(f32::to_bits).hash(h);
            }
            key.images.hash(h);
            key.numbers.hash(h);
        }));
        let editable = input.editable;
        let properties = input.properties;
        let space_width = frame
            .advance_faux(
                winc_render::FontKind::Ui,
                winc_render::Faux::default(),
                key.base,
                ' ',
            )
            .max(1.0);
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
        let mut blocks: Vec<(u64, Arc<LaidBlock>)> = Vec::with_capacity(parsed.blocks.len());
        let mut tops = Vec::with_capacity(parsed.blocks.len());
        let mut starts = Vec::with_capacity(parsed.blocks.len());
        let (mut y, mut below, mut chars) = (0.0, None, 0);
        let blank_row = winc_typeset::blank_row_height(key.base);
        let mut gaps = Vec::new();
        // During an asynchronous reparse, keep the raw editing fallback rather than attaching
        // old source offsets to the newly edited note.
        let checked = editable && input.checked;
        let blank = &parsed.blank_rows;
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
        // 内容相同的块从第二个起使用不同的键，并带上序号，各自记录开合状态。
        let mut repeats: HashMap<u64, u32> = HashMap::new();
        for (i, (block, &hash)) in parsed.blocks.iter().zip(&parsed.hashes).enumerate() {
            let (block, hash) = if let (0, Some(panel)) = (i, &properties) {
                (
                    panel,
                    winc_typeset::memo_key(|h| std::hash::Hash::hash(&(hash, panel), h)),
                )
            } else {
                (block, hash)
            };
            let repeat = *repeats.entry(hash).and_modify(|n| *n += 1).or_insert(0);
            let hash = match repeat {
                0 => hash,
                n => winc_typeset::memo_key(|h| std::hash::Hash::hash(&(hash, n), h)),
            };
            cx.repeat = repeat;
            cx.toggles.clear();
            let raw = raw
                .as_ref()
                .filter(|r| !mapped && r.0 == i && !(i == 0 && properties.is_some()));
            // 显示原文的块换一个键存：光标离开后同样内容的块要重新按排版显示。
            let hash = match raw {
                Some((_, raw)) => winc_typeset::memo_key(|h| {
                    use std::hash::Hash;
                    (hash, raw).hash(h);
                }),
                None => hash,
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
                gaps.push(BlankGap::new(
                    offsets,
                    blank.offsets(),
                    input.rope,
                    y,
                    space,
                    Some(i),
                    false,
                ));
            }
            let top = y + space;
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
            y = top + laid.height;
            below = Some(margin_bottom);
            blocks.push((hash, laid));
        }
        if editable && !blocks.is_empty() {
            let already = blocks.last().map_or(0.0, |(_, b)| b.editing_blank_height());
            let extra = after.len().saturating_sub(2) as f32 * blank_row;
            if already == 0.0 && blank_after {
                gaps.push(BlankGap::new(
                    after,
                    blank.offsets(),
                    input.rope,
                    y,
                    extra,
                    None,
                    true,
                ));
                y += extra;
            } else {
                y += (extra - already).max(0.0);
            }
        }
        // A note with only blank lines has no Markdown blocks, but those rows still
        // need real source positions in the editable view.
        let empty = if blocks.is_empty() && editable {
            input
                .rope
                .filter(|rope| rope.chars().all(char::is_whitespace))
                .map(|rope| 0..rope.len_bytes())
        } else {
            None
        };
        if let (Some(range), Some(rope)) = (&empty, input.rope) {
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
            ranges: {
                // 解析之后打的字都在光标所在的块里：这一块换成现在的范围，原文在它后面的块按文字
                // 长度的变化挪。
                let mut ranges = empty.map_or_else(|| parsed.ranges.clone(), |range| vec![range]);
                if let Some(live) = live
                    && let Some(start) = ranges.get(live.block).map(|r| r.start)
                {
                    let len = input.rope.map(|r| r.len_bytes());
                    let delta = len.map_or(0, |len| len as i64 - parsed.len as i64);
                    let shift = |at: usize| usize::try_from(at as i64 + delta).unwrap_or(0);
                    for range in ranges.iter_mut().filter(|r| r.start > start) {
                        *range = shift(range.start)..shift(range.end);
                    }
                    ranges[live.block] = live.range.clone();
                }
                ranges
            },
            gaps,
            blank_offsets: Arc::clone(blank.offsets()),
            space_width,
            live: live.map(|l| l.key(input.preedit)),
            caret: std::cell::Cell::new(None),
            drawn_upstream: None,
            height: y,
            left: 0.0,
            pad: 0.0,
            hscroll: HashMap::new(),
            followed: None,
            stale,
            near,
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

    /// 当前帧条件、源码、光标状态与保留范围是否全部命中缓存。
    pub fn matches_frame(
        &self,
        key: Key,
        source: usize,
        live: Option<&LiveKey>,
        near: &Near,
        view_h: f32,
    ) -> bool {
        self.key == key
            && self.source == source
            && self.live.as_ref() == live
            && !self.pending()
            && self
                .near
                .as_ref()
                .is_none_or(|kept| kept.covers(near, view_h))
    }

    /// 是否只是续排上一帧未完的块或更新保留范围（条件、源码、光标均未变）。
    pub fn can_resume(&self, key: Key, source: usize, live: Option<&LiveKey>) -> bool {
        self.key == key && self.source == source && self.live.as_ref() == live
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
        let blocks = self
            .blocks
            .iter()
            .zip(&self.tops)
            .enumerate()
            .map(|(i, ((hash, block), top))| (*hash, Arc::clone(block), top + offset(i) - scroll))
            .collect();
        Before {
            blocks,
            revision: self.live.as_ref().map(|k| k.2).or(current_revision),
        }
    }

    /// 计算从重排前 `before` 过渡到当前排版所需的各块偏移与淡入项。
    pub fn motion_from(&self, before: &Before, scroll: f32, muted: Color, slide: bool) -> Moves {
        let (old, new) = (&before.blocks, &self.blocks);
        let matched: Vec<Option<usize>> = if old.len() == new.len() {
            (0..new.len()).map(Some).collect()
        } else {
            let same = |i: usize, j: usize| new[i].0 == old[j].0;
            let short = old.len().min(new.len());
            let head = (0..short).take_while(|&i| same(i, i)).count();
            let tail = (0..short - head)
                .take_while(|&k| same(new.len() - 1 - k, old.len() - 1 - k))
                .count();
            (0..new.len())
                .map(|i| {
                    if i < head {
                        Some(i)
                    } else if i >= new.len() - tail {
                        Some(i + old.len() - new.len())
                    } else {
                        None
                    }
                })
                .collect()
        };
        let mut from = Vec::with_capacity(new.len());
        let mut last = 0.0;
        for (i, j) in matched.iter().enumerate() {
            if let Some(j) = j {
                last = old[*j].2 - (self.tops[i] - scroll);
            }
            from.push(last);
        }
        let moved = self.live.as_ref().map(|k| k.2) == before.revision && before.revision.is_some();
        if !moved && !slide {
            from.fill(0.0);
        }
        let (mut inner, mut leaving) = (Vec::new(), Vec::new());
        for (i, j) in matched.iter().enumerate() {
            let Some(j) = *j else { continue };
            if !moved && !slide {
                break;
            }
            match (old[j].1.preview_split(), new[i].1.preview_split()) {
                (None, Some((first, top))) => inner.push((i, first, -top)),
                (Some((first, top)), None) => {
                    leaving.push(Leaving {
                        block: i,
                        old: Arc::clone(&old[j].1),
                        items: first,
                        offset: from[i],
                    });
                    from[i] += top;
                }
                _ => {}
            }
        }
        let mut fading = Vec::new();
        for (i, j) in matched.iter().enumerate() {
            let Some(j) = *j else { continue };
            if !moved || new[i].0 == old[j].0 {
                continue;
            }
            let mut seen: HashMap<Arc<str>, usize> = HashMap::new();
            for (_, text) in old[j].1.muted_text_marks(muted) {
                *seen.entry(text).or_default() += 1;
            }
            let fresh: Box<[usize]> = new[i]
                .1
                .muted_text_marks(muted)
                .into_iter()
                .filter(|(_, text)| match seen.get_mut(text) {
                    Some(n) if *n > 0 => {
                        *n -= 1;
                        false
                    }
                    _ => true,
                })
                .map(|(k, _)| k)
                .collect();
            if !fresh.is_empty() {
                fading.push((i, fresh));
            }
        }
        Moves {
            from,
            fading,
            inner,
            leaving,
        }
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
    pub fn scroller_at(&self, x: f32, y: f32) -> Option<(usize, usize, f32)> {
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
    pub fn sync_points(&self, total: usize) -> Vec<(f32, f32)> {
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

    fn blank_at(&self, x: f32, y: f32) -> Option<usize> {
        let i = self
            .gaps
            .partition_point(|gap| gap.top <= y)
            .checked_sub(1)?;
        let gap = &self.gaps[i];
        let row = ((y - gap.top) / gap.row).floor() as usize;
        let start = *self.blank_offsets[gap.offsets.start..gap.offsets.end - 1].get(row)?;
        let max_col = gap.row_cols.get(row).copied().unwrap_or(0);
        let col = if max_col > 0 && x > 0.0 {
            ((x / self.space_width).round() as usize).min(max_col)
        } else {
            0
        };
        Some(start + col)
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

    fn table_cell(&self, blocks: std::ops::Range<usize>, x: f32, y: f32) -> Option<(usize, Rect)> {
        for block in blocks {
            let y = y - self.tops[block];
            for (k, rect, cols, rows) in self.blocks[block].1.tables() {
                let (off, strip) = self.shift(block, k);
                if strip.is_some_and(|s| !s.contains(x, y)) {
                    continue;
                }
                let rect = Rect::new(rect.x - off, rect.y, rect.w, rect.h);
                if !rect.contains(x, y) {
                    continue;
                }
                let (mut right, mut bottom) = (rect.x, rect.y);
                let c = cols.iter().position(|w| {
                    right += w;
                    x < right
                })?;
                let r = rows.iter().position(|h| {
                    bottom += h;
                    y < bottom
                })?;
                let cell = Rect::new(right - cols[c], bottom - rows[r], cols[c], rows[r]);
                return Some((block, cell));
            }
        }
        None
    }

    fn hit(&self, area_y: f32, scroll: f32, x: f32, y: f32) -> Option<Hit> {
        let origin = area_y + self.pad - scroll;
        let (x, y) = (x - self.left, y - origin);
        let mut best: Option<((f32, f32), Hit)> = None;
        let i = self.tops.partition_point(|&t| t <= y).saturating_sub(1);
        let blocks = i.saturating_sub(1)..(i + 2).min(self.blocks.len());
        let table = self.table_cell(blocks.clone(), x, y);
        let mut inside: Option<((f32, f32), Hit)> = None;
        for (top, start, ix, baseline, size, span, block, text) in self.spans(blocks.clone()) {
            let (y0, h) = line_box(top + baseline, size);
            let width: f32 = span.advances.iter().sum();
            let dy = (y0 - y).max(y - (y0 + h)).max(0.0);
            let dx = (ix - x).max(x - (ix + width)).max(0.0);
            let here = table.as_ref().is_some_and(|(b, cell)| {
                *b == block && cell.contains(ix, y0 + h / 2.0 - self.tops[*b])
            });
            let slot = if here { &mut inside } else { &mut best };
            if slot.as_ref().is_some_and(|(d, _)| *d <= (dy, dx)) {
                continue;
            }
            let mut pen = ix;
            let mut at = span.advances.len();
            for (k, w) in span.advances.iter().enumerate() {
                if x < pen + w / 2.0 {
                    at = k;
                    break;
                }
                pen += w;
            }
            let hit = Hit {
                offset: Some(start + at),
                source: self
                    .ranges
                    .get(block)
                    .map(|r| r.start + span.source_of(text, at).unwrap_or(0)),
            };
            *slot = Some(((dy, dx), hit));
        }
        if inside.is_some() {
            return inside.map(|(_, hit)| hit);
        }
        for block in blocks {
            let top = self.tops[block];
            for (k, rect, (start, source)) in self.blocks[block].1.source_spots() {
                let off = self.shift(block, k).0;
                if rect.contains(x + off, y - top) {
                    return Some(Hit {
                        offset: Some(self.starts[block] + start),
                        source: self.ranges.get(block).map(|r| r.start + source as usize),
                    });
                }
            }
        }
        if best.as_ref().is_none_or(|((dy, _), _)| *dy > 0.0)
            && let Some(source) = self.blank_at(x, y)
        {
            return Some(Hit {
                offset: None,
                source: Some(source),
            });
        }
        best.map(|(_, hit)| hit)
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
        let i = self
            .gaps
            .partition_point(|gap| self.blank_offsets[gap.offsets.start] <= source)
            .checked_sub(1)?;
        let gap = &self.gaps[i];
        let (end, starts) = self.blank_offsets[gap.offsets.clone()].split_last()?;
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
        let (_, body_height) = line_box(0.0, self.key.base);
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
        Some((x, self.pad + caret_y, caret_h))
    }

    /// 若第 `block` 块与下一块之间存在可编辑空行槽，返回该空行槽的上边缘（相对内容顶部）。
    pub fn gap_top_after(&self, block: usize) -> Option<f32> {
        self.gaps
            .iter()
            .find(|gap| gap.before_block == Some(block + 1))
            .map(|gap| gap.top)
    }

    /// 返回与原文选区 `from..to` 相交的空行高亮矩形（相对内容左上角，逻辑像素）。
    pub fn blank_selection_rects(&self, from: usize, to: usize) -> Vec<Rect> {
        if from >= to {
            return Vec::new();
        }
        let (_, body_height) = line_box(0.0, self.key.base);
        let mut rects = Vec::new();
        for gap in &self.gaps {
            let Some((&end, starts)) = self.blank_offsets[gap.offsets.clone()].split_last() else {
                continue;
            };
            if end <= from || starts[0] >= to {
                continue;
            }
            for (row, &row_start) in starts.iter().enumerate() {
                let row_next = self.blank_offsets[gap.offsets.start + row + 1];
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
                    self.key.width
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

    /// Whether a byte begins an actual glyph in the visible source mapping.
    pub fn shows_source(&self, source: usize) -> bool {
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
    pub fn limit_blank_step(&self, from: f32, target: f32) -> f32 {
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

    /// 可编辑的预览左侧写行号的各显示行：原文行号（从 0 起）和显示行的基线（相对内容顶部）。
    pub fn source_lines(
        &self,
        rope: &ropey::Rope,
        view: std::ops::Range<f32>,
    ) -> Vec<(usize, f32)> {
        let first = self
            .tops
            .partition_point(|&t| t <= view.start)
            .saturating_sub(1);
        let last = self.tops.partition_point(|&t| t < view.end);
        let mut rows: Vec<(f32, f32, usize)> = Vec::new();
        for (top, _, x, baseline, _, span, block, _) in self.spans(first..last) {
            let (Some(start), Some(range)) = (span.source, self.ranges.get(block)) else {
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
                let from = rope.char_to_byte(rope.byte_to_char(from));
                let line = rope.byte_to_line(from);
                let prefix = rope.byte_slice(rope.line_to_byte(line)..from);
                prefix.chars().all(is_markup).then_some((line, y))
            })
            .collect()
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

/// 内容一栏两侧仍按点在文字上处理的宽度（逻辑像素，经验值）。
const CONTENT_SLACK: f32 = 12.0;

/// 鼠标落在的显示字符和原文字节位置。空行没有显示文字，但可以定位到原文。
struct Hit {
    offset: Option<usize>,
    source: Option<usize>,
}

/// `c` 是否可以出现在一行原文开头、正文之前的标记中。
fn is_markup(c: char) -> bool {
    c.is_ascii_digit()
        || matches!(
            c,
            ' ' | '\t' | '#' | '>' | '-' | '*' | '+' | '.' | ')' | '|' | '[' | ']' | 'x' | 'X'
        )
}

/// 在递增的对应点 `(行, 位置)` 之间按比例换算：`inverse` 为假时由行算位置，为真时反过来。
pub fn interpolate(points: &[(f32, f32)], at: f32, inverse: bool) -> f32 {
    let key = |p: &(f32, f32)| if inverse { p.1 } else { p.0 };
    let value = |p: &(f32, f32)| if inverse { p.0 } else { p.1 };
    let i = points.partition_point(|p| key(p) <= at);
    match (i.checked_sub(1).map(|i| &points[i]), points.get(i)) {
        (Some(a), Some(b)) if key(b) > key(a) => {
            value(a) + (at - key(a)) / (key(b) - key(a)) * (value(b) - value(a))
        }
        (Some(a), _) => value(a),
        (None, _) => 0.0,
    }
}

/// 复制按钮的「链接」地址前缀，后面接代码：点击时把代码放进剪贴板。
///
/// 开头的 `\u{1}` 不是合法的 URL 字符，用来和文档里的真实链接区分。
pub const COPY: &str = "\u{1}copy:";

/// 图的全屏按钮的「链接」地址：点击后全屏查看 `MarkdownPreview::expand` 中的图。
pub const EXPAND: &str = "\u{1}expand";

/// 勾选框链接的地址前缀，其后为 `[ ]` 在原文中的位置（字节偏移）；点击时切换勾选状态。
pub const TASK: &str = "\u{1}task:";
/// 表格工具栏按钮链接的地址前缀，其后为操作名（见 `table_tools`）。
pub const TABLE: &str = "\u{1}table:";
