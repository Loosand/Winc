//! Markdown 预览的排版结果与位置查询。
//!
//! [`Laid`] 是整篇文档的排版结果，由 [`lay_out`] 按顶层块生成并缓存：文字变化时只重排变化的块，
//! 宽度、字号、配色或图片大小变化时整篇重排。位置查询在预览坐标与原文位置之间换算，
//! 供点击、选择、滚动同步和可编辑的预览使用。绘制在 `view::preview::markdown` 中。

use std::collections::HashMap;
use std::sync::Arc;

use winc_render::{Color, Frame, Rect};
use winc_typeset::{Colors, Cx, Env, Item, LaidBlock, Out, Raw, Span, margins};

pub(crate) use winc_typeset::{DETAILS, Memo};

use crate::app::{App, Focus};

mod hit_test;
mod source_map;

pub(crate) use hit_test::{in_content, offset_at, scroll_table, source_at};
pub(crate) use source_map::{font_at, offset_top, point_of, source_lines};

/// 正文字号与编辑器字号之比：编辑器字号 13 逻辑像素时，正文为 16 逻辑像素。
pub(crate) const BASE_RATIO: f32 = 16.0 / 13.0;
/// 正文宽度为「标准」（`markdown.textWidth`）时正文的最大宽度（逻辑像素，经验值）：更宽时居中、
/// 两侧留白。
pub(crate) const MAX_WIDTH: f32 = 720.0;
/// 「较宽」给表格和双语长段留出空间，保留两侧页边距。
pub(crate) const WIDE_WIDTH: f32 = 1040.0;
/// 区域不窄于 600 逻辑像素时文档四周的留白（逻辑像素，经验值）。
pub(crate) const PAD: f32 = 32.0;
/// 窄的时候（分栏，区域宽不足 600 逻辑像素）两边的留白（逻辑像素，经验值）。
pub(crate) const PAD_NARROW: f32 = 20.0;

/// 排版完成的文档。
pub(crate) struct Laid {
    pub(crate) key: Key,
    /// 排版依据的解析结果（`Arc` 的地址），换了说明文字改了。
    pub(crate) source: usize,
    /// 各顶层块：内容哈希和排版结果。
    pub(crate) blocks: Vec<(u64, Arc<LaidBlock>)>,
    /// 各顶层块的上边缘（相对内容顶部）。
    pub(crate) tops: Vec<f32>,
    /// 各顶层块的文字在整份文字（各块的 [`LaidBlock::text`] 连起来）里从第几个字符开始。
    pub(crate) starts: Vec<usize>,
    /// 整份文字的字符数。
    pub(crate) chars: usize,
    /// 各顶层块在原文里从第几行开始（见 `Markdown::lines`），分栏对齐滚动用。
    pub(crate) lines: Vec<usize>,
    /// 各顶层块在原文中的字节范围（见 `Markdown::ranges`），预览与原文互相定位用。
    pub(crate) ranges: Vec<std::ops::Range<usize>>,
    /// Source-addressable line boxes in the spaces between formatted blocks.
    gaps: Vec<BlankGap>,
    blank_offsets: Arc<[usize]>,
    /// 排版时光标所在的叶子块，见 [`LiveKey`]。
    ///
    /// 不能编辑时为 `None`；光标换了叶子块（表格里换了一格）、或者打了字，这一块要重排。
    pub(crate) live: Option<LiveKey>,
    /// 上一次找的光标位置和结果（见 [`point_of`]）：滚动时光标不动，不必每帧在大块里再找。
    caret: std::cell::Cell<Option<CachedCaret>>,
    /// 这一帧画在上一显示行末尾的光标位置（原文字节偏移）。
    pub(crate) drawn_upstream: Option<usize>,
    /// 内容总高度。
    pub(crate) height: f32,
    /// 上一次画的时候内容的左边缘（窗口坐标，逻辑像素），鼠标定位用。
    pub(crate) left: f32,
    /// 上一次画的时候内容四周的留白（逻辑像素），鼠标定位用。
    pub(crate) pad: f32,
    /// 比正文宽的表格的横向滚动位置（逻辑像素）：键为（顶层块序号，块中表格的序号，见
    /// [`LaidBlock::scrolls`]）。重排后按序号沿用。
    pub(crate) hscroll: HashMap<(usize, usize), f32>,
    /// 上一次横向滚动使其可见的光标位置（原文字节偏移），见 [`Laid::follow_caret`]。
    pub(crate) followed: Option<usize>,
    /// 各顶层块是否仍沿用旧条件下的排版结果（见 [`Later`]）；全部按当前条件排好时为空。
    pub(crate) stale: Vec<bool>,
    /// 排版时保留了排版结果的范围（见 [`Near`]）；文档不长、全部保留时为 `None`。
    pub(crate) near: Option<Near>,
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
}

impl BlankGap {
    fn new(offsets: std::ops::Range<usize>, top: f32, height: f32) -> Self {
        let row = height / (offsets.len() - 1) as f32;
        Self { top, row, offsets }
    }

    fn bottom(&self) -> f32 {
        self.top + self.row * (self.offsets.len() - 1) as f32
    }
}

/// 分帧排版的依据：排版条件变化而文字未变时，视口内的块立即重排，其余的块在超过时限后沿用
/// 旧结果，由之后的各帧继续重排。
pub(crate) struct Later<'a> {
    /// 旧的各顶层块，与本次排版的各块一一对应。
    pub(crate) old: &'a [(u64, Arc<LaidBlock>)],
    /// 必须按当前条件重排的块，即视口内的块。
    pub(crate) window: std::ops::Range<usize>,
    /// 截止时刻：此后 `window` 之外的块沿用旧结果。
    pub(crate) deadline: std::time::Instant,
}

/// 顶层块超过该数量（块）的文档只保留 [`Near`] 范围内的块的排版结果，其余的块释放要画的项
/// （见 [`LaidBlock::shed`]），排版缓存也只保留这些块用到的条目。
///
/// 取自调参：512 块以内的文档全部保留时，排版结果与缓存合计不超过几 MB。在 Apple M 系列芯片上
/// 实测，2 万段、4 MB 的文档全部保留时二者占用 91 MB 堆内存。
pub(crate) const SHED_BLOCKS: usize = 512;

/// 需要保留排版结果的块：内容坐标在 `rows` 范围内的块，以及序号在 `blocks` 范围内的块（光标和
/// 选区所在的块及其前后）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Near {
    pub(crate) rows: (f32, f32),
    pub(crate) blocks: Vec<std::ops::Range<usize>>,
}

/// 光标所在的块前后各保留排版结果的块数。取自调参：32 块超过一屏能显示的段数，光标上下移动、
/// 翻页时落到的块都已排好。
pub(crate) const NEAR_BLOCKS: usize = 32;

impl Near {
    /// 按本范围保留的排版结果是否够 `wanted` 使用：`wanted` 的各块都在其中，视口（高 `view`）
    /// 前后各有半屏以上。
    pub(crate) fn covers(&self, wanted: &Near, view: f32) -> bool {
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
pub(crate) const LAYOUT_BUDGET: std::time::Duration = std::time::Duration::from_millis(4);

/// 预览上的一个位置：左边缘（相对内容左边）、所在那一行铺底色的上边缘（和滚动位置同一个
/// 量法）和高度，均为逻辑像素；见 [`point_of`]。
pub(crate) type Point = (f32, f32, f32);

/// 决定排版结果的外部条件：任何一项变了都要整份重排。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Key {
    /// 正文宽度（逻辑像素，已减去两侧留白）。
    pub(crate) width: f32,
    /// 正文字号（逻辑像素）。
    pub(crate) base: f32,
    /// 正文颜色。
    pub(crate) text: Color,
    /// 底色，决定差异色、强调色混在白底还是黑底上。
    pub(crate) background: Color,
    /// 图片的版本号，有图片读好后就加一。
    pub(crate) images: u64,
    /// 代码块是否显示从 1 起的行号（见 `Cx::line_numbers`）。
    pub(crate) numbers: bool,
    /// mermaid 图的高度上限（逻辑像素，见 `Cx::tall`）。
    pub(crate) tall: f32,
}

/// 重排之前各顶层块的排版和绘制位置，重排后据此计算过渡动画（见 `BlockMotion`）。
pub(crate) struct Before {
    /// 各块的内容哈希、排版结果和绘制时的上边缘（相对内容顶部、已减去滚动位置，含滑动偏移）。
    pub(crate) blocks: Vec<(u64, Arc<LaidBlock>, f32)>,
    /// 排版时的文档版本（见 [`LiveKey`]）；版本未变表示只移动了光标。
    pub(crate) revision: Option<u64>,
}

impl Before {
    pub(crate) fn of(app: &App, laid: &Laid) -> Before {
        let scroll = app.preview.markdown.scroll();
        let motion = app.preview.markdown.motion.as_ref();
        let blocks = laid
            .blocks
            .iter()
            .zip(&laid.tops)
            .enumerate()
            .map(|(i, ((hash, block), top))| {
                let offset = motion.map_or(0.0, |m| m.offset(i));
                (*hash, Arc::clone(block), top + offset - scroll)
            })
            .collect();
        // 排版中没有光标时（未在预览中编辑）取文档当前的版本号。
        let current = app.editor.active().map(|d| d.revision);
        Before {
            blocks,
            revision: laid.live.as_ref().map(|k| k.2).or(current),
        }
    }

    /// 计算重排为 `laid`（滚动位置 `scroll`）后各块的起始偏移（见 `BlockMotion::from`）和淡入的项。
    ///
    /// 块数不变时按序号对应；块数变化（删除了一块、解析出新的块）时，按开头和末尾哈希相同的块
    /// 对应，中间新增的块随前一块移动。仅移动光标时，外观变化的块中新增的标记（次要色 `muted`
    /// 的文字中比原来多出的段）淡入。
    ///
    /// 文字已修改时各块直接到位、偏移全为 0，输入的结果在同一帧显示在最终位置；`slide` 为真
    /// （表格工具增删行列）时仍然滑动。
    pub(crate) fn motion(
        &self,
        laid: &Laid,
        scroll: f32,
        muted: Color,
        slide: bool,
    ) -> crate::app::preview::markdown::Moves {
        let (old, new) = (&self.blocks, &laid.blocks);
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
                last = old[*j].2 - (laid.tops[i] - scroll);
            }
            from.push(last);
        }
        let moved = laid.live.as_ref().map(|k| k.2) == self.revision && self.revision.is_some();
        if !moved && !slide {
            from.fill(0.0);
        }
        // 原文与排版互换的块：渲染结果随下方的块一起滑动。
        let (mut inner, mut leaving) = (Vec::new(), Vec::new());
        for (i, j) in matched.iter().enumerate() {
            let Some(j) = *j else { continue };
            if !moved && !slide {
                break;
            }
            match (old[j].1.preview, new[i].1.preview) {
                // 显示原文：渲染结果从原位置下移。
                (None, Some((first, top))) => inner.push((i, first, -top)),
                // 恢复排版：排版从原来渲染结果的位置上移，原文淡出。
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
            let marks = |block: &LaidBlock| {
                block
                    .items
                    .iter()
                    .enumerate()
                    .filter_map(|(k, item)| match item {
                        Item::Text { text, color, .. }
                            if *color == muted && !text.trim().is_empty() =>
                        {
                            Some((k, Arc::clone(text)))
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            };
            let mut seen: HashMap<Arc<str>, usize> = HashMap::new();
            for (_, text) in marks(&old[j].1) {
                *seen.entry(text).or_default() += 1;
            }
            let fresh: Box<[usize]> = marks(&new[i].1)
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
        crate::app::preview::markdown::Moves {
            from,
            fading,
            inner,
            leaving,
        }
    }
}

/// 刚恢复排版的块原来显示的原文。
pub(crate) struct Leaving {
    /// 块序号（新排版中）。
    pub(crate) block: usize,
    /// 原来的排版。
    pub(crate) old: Arc<LaidBlock>,
    /// 原文所占的项数（其后为渲染结果）。
    pub(crate) items: u32,
    /// 起始时相对新位置向下的偏移（逻辑像素）。
    pub(crate) offset: f32,
}

/// 可编辑的预览中显示原文的块的缓存键；各字段均不变时沿用上一次的排版。
///
/// 包含顶层块、叶子块和文档版本；位于表格或正文中时还包含光标位置（切换单元格需要重排，
/// 正文中光标接触的标记需要显示）；位于代码块的首尾几行时包含所在的行（围栏行需要显示，
/// 见 `LiveLeaf::edge`）。
pub(crate) type LiveKey = (usize, std::ops::Range<usize>, u64, Option<usize>, String);

/// 可编辑的预览中光标所在的叶子块（见 [`winc_preview::markdown::Markdown::leaves`]），该块显示原文。
///
/// 原文位置均按当前文字计算：解析之后输入的文字都位于该块内（光标在该块中），块的末尾
/// 按文字长度的变化移动。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveLeaf {
    /// 所在的顶层块（`Markdown::blocks` 的序号）。
    pub block: usize,
    /// 所在顶层块当前的范围（字节偏移）。
    pub range: std::ops::Range<usize>,
    /// 叶子块当前的范围（字节偏移）；光标位于其后的空行上时延伸到光标所在行，位于其前的
    /// 块标记上时从光标所在行起算。
    pub leaf: std::ops::Range<usize>,
    /// 叶子块在解析结果中的终点（字节偏移）：输入之后、重新解析之前与 `leaf` 的终点不同。
    pub parsed_end: usize,
    /// 文档版本号：输入文字后该块需要重排。
    pub revision: u64,
    /// 是否为正文（段落、标题），见 `LeafRange::prose`。
    pub prose: bool,
    /// 是否为表格；表格按单元格编辑。
    pub table: bool,
    /// 光标是否位于段落、标题之后的空行上。
    pub blank: bool,
    /// 查找该块所用的位置（字节偏移）。
    ///
    /// 由调用者选择实际光标或起选时的排版位置。表格中该位置所在的单元格显示原文，
    /// 正文中显示该位置接触到的标记。
    pub at: usize,
    /// 光标位于代码块、公式等叶子块的第一行或最后两行时，光标所在的行号。
    ///
    /// 围栏行只在光标位于其上时显示，因此光标进出这些行时需要重排；在中间各行移动时不重排。
    pub edge: Option<usize>,
}

/// `preedit` 是输入法正在组合的文字，在正文中排入光标所在的行，因此其变化同样需要重排。
pub(crate) fn live_key(live: &LiveLeaf, preedit: &str) -> LiveKey {
    (
        live.block,
        live.leaf.clone(),
        live.revision,
        (live.table || live.prose).then_some(live.at).or(live.edge),
        if live.prose || live.table {
            preedit.to_owned()
        } else {
            String::new()
        },
    )
}

/// 重排之前光标所在的那一行的高度（相对内容顶部，不含留白），重排后据此保持它在屏幕上不动。
///
/// `clicked` 是刚单击放下的光标，与现在的光标相同时返回 `None`。
pub(crate) fn old_caret(
    app: &App,
    old: Option<&Laid>,
    source: usize,
    live: Option<&LiveKey>,
    clicked: Option<usize>,
    caret: usize,
) -> Option<(usize, f32)> {
    let old = old?;
    if clicked == Some(caret) && !app.scroll.keyed {
        return None;
    }
    let (_, y, h) = old.caret_point(caret)?;
    if old.source != source {
        // A background parse can replace an approximate layout for the same
        // edit. Anchor only its unchanged, visible caret; never chase one the
        // user left offscreen by scrolling or moved since the last frame.
        let doc = app.editor.active()?;
        if app.preview.markdown.table_edit_revision == Some((doc.id, doc.revision)) {
            return None;
        }
        let scroll = app.preview.markdown.scroll();
        if app.preview.markdown.followed != Some((doc.id, caret, doc.revision))
            || y + h <= scroll
            || y >= scroll + app.preview.markdown.area.h
        {
            return None;
        }
    } else if old.live.as_ref().map(|k| k.2) != live.map(|k| k.2) {
        return None;
    }
    Some((caret, y - old.pad))
}

/// 重排后光标所在的那一行在屏幕上不动：光标换了叶子块时，上面的块从原文换回排版（或反过来），
/// 高度变了，内容整体上下错开；按光标的新旧高度之差滚动。
pub(crate) fn keep_caret_still(app: &mut App, still: Option<(usize, f32)>, laid: &Laid) {
    let Some((source, before)) = still else {
        return;
    };
    let Some((_, y, _)) = laid.caret_point(source) else {
        return;
    };
    let dy = y - laid.pad - before;
    if dy.abs() >= 0.5 {
        let scroll = app.preview.markdown.scroll();
        app.preview.markdown.scroll_to(scroll + dy);
    }
}

/// 编辑器核心 · 排版入口：把解析块变成带屏幕坐标的绘制项，由 `winc-typeset` 排字、折行。
/// `reuse` 里有同样内容的块直接沿用；长文只在需要的范围保留绘制项。
///
/// 光标所在的叶子块（`live`）结合当前原文即时排版，不能当作普通块在不同光标位置复用。
#[allow(clippy::too_many_arguments)]
pub(crate) fn lay_out(
    app: &mut App,
    frame: &mut Frame<'_>,
    parsed: &winc_preview::markdown::Markdown,
    key: Key,
    source: usize,
    live: Option<&LiveLeaf>,
    reuse: Vec<(u64, Arc<LaidBlock>)>,
    later: Option<Later<'_>>,
    near: Option<Near>,
) -> Laid {
    let reuse: HashMap<u64, Arc<LaidBlock>> = reuse.into_iter().collect();
    // 光标所在顶层块现在的原文，和叶子块在其中的范围。
    let raw = live.and_then(|live| {
        let rope = app.editor.active()?.text().rope()?;
        let snap = |b: usize| rope.char_to_byte(rope.byte_to_char(b.min(rope.len_bytes())));
        let (start, end) = (snap(live.range.start), snap(live.range.end));
        let text: Arc<str> = rope.byte_slice(start..end).to_string().into();
        let leaf =
            snap(live.leaf.start).saturating_sub(start)..snap(live.leaf.end).saturating_sub(start);
        let caret = snap(live.at).saturating_sub(start);
        let preedit = app.preedit_at(Focus::Editor);
        let preedit =
            ((live.prose || live.table) && !preedit.is_empty()).then(|| Arc::from(preedit));
        let parsed_end = live.parsed_end.saturating_sub(start);
        let raw = Raw::new(text, leaf, parsed_end, caret, live.prose, preedit);
        Some((live.block, raw))
    });
    let p = app.palette;
    app.preview
        .markdown
        .memo
        .prepare(winc_typeset::memo_key(|h| {
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
    let doc = app.editor.active().and_then(|d| d.path.clone());
    let remote = app.config.markdown_remote_images != Some(false);
    let image_path =
        |url: &str| crate::app::preview::markdown::image_path(doc.as_deref(), url, remote);
    let token = |kind| p.token(kind);
    let editable = app.markdown_live();
    let properties = parsed
        .properties
        .as_ref()
        .filter(|_| app.note_properties_available())
        .map(|(_, fields)| app.property_panel(fields));
    let mut cx = Cx {
        frame,
        base: key.base,
        token: &token,
        surface: p.editor_bg,
        accent: p.accent,
        colors: Colors::new(p.text, p.text_muted, p.accent, p.dark),
        dark: p.dark,
        images: &app.preview.markdown.images,
        image_path: &image_path,
        missing_image: None,
        wanted: Vec::new(),
        memo: &mut app.preview.markdown.memo,
        toggled: &app.preview.markdown.toggled,
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
    let checked = editable
        && app
            .editor
            .active()
            .is_some_and(|d| app.preview.markdown.checked == Some(d.revision));
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
        let already = blocks.last().map_or(0.0, |(_, b)| b.editing_blank_height);
        let offsets = blank.before(i);
        let extra_rows = offsets.len().saturating_sub(1 + usize::from(i != 0));
        let mapped_gap = blank_before(i, offsets);
        let gap = if editable {
            (extra_rows as f32 * blank_row - already).max(0.0)
        } else {
            0.0
        };
        let mut space = below.map_or(0.0, |below: f32| below.max(margin)) + gap;
        if already == 0.0
            && let Some(offsets) = mapped_gap
        {
            space = space.max(blank_row);
            gaps.push(BlankGap::new(offsets, y, space));
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
            let mut out = Out::default();
            cx.text.clear();
            cx.chars = 0;
            cx.memo.take_touched();
            cx.raw = raw.map(|(_, raw)| raw.clone());
            cx.preedit = false;
            cx.cell = None;
            let mut flow = cx.block(block, 0.0, 0.0, key.width, env, &mut out);
            // 光标所在的叶子块在块里没有找到（写在列表里的公式这类没有原文对照的块）：整个
            // 顶层块显示原文。
            if let Some(raw) = cx.raw.as_mut().filter(|r| !r.done) {
                raw.leaf = 0..raw.block.len();
                raw.parsed_end = raw.block.len();
                raw.kind = raw.guess();
                out = Out::default();
                cx.text.clear();
                cx.chars = 0;
                flow = cx.raw_with_preview(block, 0.0, 0.0, key.width, env, &mut out);
            }
            cx.raw = None;
            let mut laid = LaidBlock::new(
                flow.height,
                out.items,
                out.anchors,
                cx.text.as_str().into(),
                cx.chars,
                cx.memo.take_touched(),
            );
            laid.preedit = cx.preedit;
            laid.editing_blank_height = out.editing_blank_height;
            laid.scrolls = out.scrolls;
            laid.preview = out.preview;
            Arc::new(laid)
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
        let already = blocks.last().map_or(0.0, |(_, b)| b.editing_blank_height);
        let extra = after.len().saturating_sub(2) as f32 * blank_row;
        if already == 0.0 && blank_after {
            gaps.push(BlankGap::new(after, y, extra));
            y += extra;
        } else {
            y += (extra - already).max(0.0);
        }
    }
    // A note with only blank lines has no Markdown blocks, but those rows still
    // need real source positions in the editable view.
    let empty = if blocks.is_empty() && editable {
        app.editor
            .active()
            .and_then(|doc| doc.text().rope())
            .filter(|rope| rope.chars().all(char::is_whitespace))
            .map(|rope| 0..rope.len_bytes())
    } else {
        None
    };
    if let Some(range) = &empty {
        let rope = app
            .editor
            .active()
            .and_then(|doc| doc.text().rope())
            .unwrap();
        cx.raw = Some(Raw::new(
            rope.to_string().into(),
            range.clone(),
            range.end,
            0,
            true,
            None,
        ));
        let mut items = Vec::new();
        cx.memo.take_touched();
        let flow = cx.raw(0.0, 0.0, key.width, &mut items);
        starts.push(0);
        tops.push(0.0);
        y = flow.height;
        blocks.push((
            0,
            Arc::new(LaidBlock::new(
                y,
                items,
                Vec::new(),
                "".into(),
                0,
                cx.memo.take_touched(),
            )),
        ));
    }
    cx.memo.sweep(blocks.iter().map(|(_, b)| &*b.memo));
    let wanted = std::mem::take(&mut cx.wanted);
    for (path, url) in wanted {
        app.request_markdown_image(&path, &url);
    }
    Laid {
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
                let len = app
                    .editor
                    .active()
                    .and_then(|d| d.text().rope())
                    .map(|r| r.len_bytes());
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
        live: live.map(|l| live_key(l, app.preedit_at(Focus::Editor))),
        caret: std::cell::Cell::new(None),
        drawn_upstream: None,
        height: y,
        left: 0.0,
        pad: 0.0,
        hscroll: HashMap::new(),
        followed: None,
        stale,
        near,
    }
}

/// 返回基线在 `baseline`、字号 `size` 的一行字在选中、查找时铺底色的范围：上边缘和高度
/// （逻辑像素）。
///
/// 上边缘在基线之上 1.05 倍字号处，高度取 1.4 倍字号（经验值，且能盖住上下标）。
pub(crate) fn line_box(baseline: f32, size: f32) -> (f32, f32) {
    (baseline - size * 1.05, size * 1.4)
}

impl Laid {
    /// 是否仍有沿用旧结果的块。
    pub(crate) fn pending(&self) -> bool {
        !self.stale.is_empty()
    }

    /// 返回内容坐标 `y` 所在的顶层块，以及 `y` 到该块上边缘的距离（逻辑像素）。
    pub(crate) fn place_at(&self, y: f32) -> Option<(usize, f32)> {
        let i = self.tops.partition_point(|&t| t <= y).checked_sub(1)?;
        Some((i, y - self.tops[i]))
    }

    /// 返回与内容坐标 `from` 到 `to` 相交的顶层块。
    pub(crate) fn blocks_in(&self, from: f32, to: f32) -> std::ops::Range<usize> {
        let first = self.tops.partition_point(|&t| t <= from).saturating_sub(1);
        first..self.tops.partition_point(|&t| t < to).max(first)
    }

    /// 返回已按当前条件排好的块。
    pub(crate) fn fresh(&self) -> Vec<(u64, Arc<LaidBlock>)> {
        let fresh = |i: &usize| !self.stale.get(*i).copied().unwrap_or(false);
        let blocks = (0..self.blocks.len()).filter(fresh);
        blocks.map(|i| self.blocks[i].clone()).collect()
    }

    /// `blocks` 范围里的各顶层块中能选中的文字，逐段返回：所在块的上边缘（相对内容顶部）、在
    /// 整份文字里的起点、横坐标、基线、字号、各字宽度和原文位置，以及所在的块和文字本身。
    pub(crate) fn spans(
        &self,
        blocks: std::ops::Range<usize>,
    ) -> impl Iterator<Item = (f32, usize, f32, f32, f32, &Span, usize, &str)> {
        let blocks = blocks.start.min(self.blocks.len())..blocks.end.min(self.blocks.len());
        self.blocks[blocks.clone()]
            .iter()
            .zip(blocks)
            .flat_map(move |((_, block), i)| {
                block
                    .items
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
    pub(crate) fn shift(&self, block: usize, item: usize) -> (f32, Option<Rect>) {
        let Some((_, laid)) = self.blocks.get(block) else {
            return (0.0, None);
        };
        laid.scroller_of(item).map_or((0.0, None), |s| {
            (self.hscroll_of(block, s), Some(laid.scrolls[s].rect))
        })
    }

    /// 第 `block` 块第 `scroller` 个可横向滚动的表格当前的滚动位置，限制在可滚动的范围内。
    pub(crate) fn hscroll_of(&self, block: usize, scroller: usize) -> f32 {
        let max = self
            .blocks
            .get(block)
            .and_then(|(_, b)| b.scrolls.get(scroller))
            .map_or(0.0, |s| s.max());
        self.hscroll
            .get(&(block, scroller))
            .copied()
            .unwrap_or(0.0)
            .clamp(0.0, max)
    }

    /// 光标（原文位置 `source`）落在可横向滚动的表格中、位于显示区域之外时，横向滚动该表格使
    /// 光标可见。只在光标移动后对齐一次，不干扰用户自行滚动。
    pub(crate) fn follow_caret(&mut self, source: usize) {
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
        let Some(s) = self.blocks[block]
            .1
            .scrolls
            .iter()
            .position(|s| (s.rect.y..s.rect.bottom()).contains(&y))
        else {
            return;
        };
        let rect = self.blocks[block].1.scrolls[s].rect;
        let max = self.blocks[block].1.scrolls[s].max();
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
    pub(crate) fn scroller_at(&self, x: f32, y: f32) -> Option<(usize, usize, f32)> {
        let i = self.tops.partition_point(|&t| t <= y).checked_sub(1)?;
        let (_, block) = self.blocks.get(i)?;
        let y = y - self.tops[i];
        block
            .scrolls
            .iter()
            .position(|s| s.rect.contains(x, y))
            .map(|s| (i, s, block.scrolls[s].max()))
    }

    /// 返回整份文字里第 `from` 到 `to` 个字符。
    pub(crate) fn text(&self, from: usize, to: usize) -> String {
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
            let (a, b) = (from.saturating_sub(start), (to - start).min(block.chars));
            out.extend(block.text.chars().skip(a).take(b.saturating_sub(a)));
        }
        out
    }

    /// 返回整份文字的字符数。
    pub(crate) fn chars(&self) -> usize {
        self.chars
    }

    /// 原文的行和预览滚动位置的对应点：文首、各块的开头、文末（原文共 `total` 行），行号和
    /// 位置都递增。
    ///
    /// 两点之间按比例算。
    pub(crate) fn sync_points(&self, total: usize) -> Vec<(f32, f32)> {
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
    pub(crate) fn y_of_line(&self, line: f32, total: usize) -> f32 {
        interpolate(&self.sync_points(total), line, false)
    }

    /// 预览滚到 `y` 时对应原文的第几行（小数）。
    pub(crate) fn line_of_y(&self, y: f32, total: usize) -> f32 {
        interpolate(&self.sync_points(total), y, true)
    }

    fn blank_at(&self, y: f32) -> Option<usize> {
        let i = self
            .gaps
            .partition_point(|gap| gap.top <= y)
            .checked_sub(1)?;
        let gap = &self.gaps[i];
        self.blank_offsets[gap.offsets.start..gap.offsets.end - 1]
            .get(((y - gap.top) / gap.row).floor() as usize)
            .copied()
    }

    /// 正文一栏的宽度（逻辑像素）。
    pub(crate) fn width(&self) -> f32 {
        self.key.width
    }

    /// 内容总高度（含四周留白）。
    ///
    /// 分栏对齐滚动时判断排版变了没有。
    pub(crate) fn height(&self) -> f32 {
        self.height + self.pad * 2.0
    }
}

/// 在递增的对应点 `(行, 位置)` 之间按比例换算：`inverse` 为假时由行算位置，为真时反过来。
pub(crate) fn interpolate(points: &[(f32, f32)], at: f32, inverse: bool) -> f32 {
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

/// 整份文字（各块的文字连起来），预览里查找用。
pub(crate) fn full_text(app: &App) -> Option<String> {
    let laid = app.preview.markdown.layout.as_ref()?;
    Some(laid.blocks.iter().map(|(_, b)| &*b.text).collect())
}

/// 复制按钮的「链接」地址前缀，后面接代码：点击时把代码放进剪贴板。
///
/// 开头的 `\u{1}` 不是合法的 URL 字符，用来和文档里的真实链接区分。
pub(crate) const COPY: &str = "\u{1}copy:";

/// 图的全屏按钮的「链接」地址：点击后全屏查看 `MarkdownPreview::expand` 中的图。
pub(crate) const EXPAND: &str = "\u{1}expand";

/// 勾选框链接的地址前缀，其后为 `[ ]` 在原文中的位置（字节偏移）；点击时切换勾选状态。
pub(crate) const TASK: &str = "\u{1}task:";
/// 表格工具栏按钮链接的地址前缀，其后为操作名（见 [`table_tools`]）。
pub(crate) const TABLE: &str = "\u{1}table:";

/// 标题锚点 `anchor`（`[跳转](#标题)` 里 `#` 后面的部分）在内容里的高度（逻辑像素）；
/// 找不到该标题时返回 `None`。
///
/// `anchor` 先做百分号解码再转小写；以 `fnref-` 开头的锚点指向引用脚注的编号。
pub(crate) fn anchor_top(app: &App, anchor: &str) -> Option<f32> {
    anchor_box(app, anchor).map(|(top, _)| top)
}

/// 锚点 `anchor` 的范围：上边缘（取法同 [`anchor_top`]）和高度。脚注引用是编号所在那一行，脚注是
/// 整条脚注，标题是标题所在的块。
pub(crate) fn anchor_box(app: &App, anchor: &str) -> Option<(f32, f32)> {
    let laid = app.preview.markdown.layout.as_ref()?;
    let anchor = crate::app::preview::markdown::percent_decode(anchor).to_lowercase();
    // 脚注末尾的 ↩ 跳回正文里第一次引用该脚注的位置，即链到脚注的编号。
    let reference = anchor.strip_prefix("fnref-").map(|n| format!("#fn-{n}"));
    laid.blocks
        .iter()
        .zip(&laid.tops)
        .find_map(|((_, block), top)| {
            let (y, h) = match &reference {
                Some(url) => block.link_box(url),
                None => block
                    .anchors
                    .iter()
                    .find(|(slug, _)| *slug == anchor)
                    .map(|(_, y)| (*y, block.height - y)),
            }?;
            Some((top + y, h))
        })
}
