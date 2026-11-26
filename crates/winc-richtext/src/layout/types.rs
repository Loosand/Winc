//! 排版层公共常量、缓存键、外部输入上下文与辅助几何函数。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use winc_render::{Color, Frame};
use winc_syntax::TokenKind;
use winc_typeset::{Colors, Cx, ImageSlot, LaidBlock, Memo, Raw};

use crate::decorations::DecorationSet;
use crate::mapping::LiveLeaf;

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

/// 顶层块超过该数量（块）的文档只保留 [`Near`] 范围内的块的排版结果，其余的块释放要画的项
/// （见 [`LaidBlock::shed`]），排版缓存也只保留这些块用到的条目。
///
/// 取自调参：512 块以内的文档全部保留时，排版结果与缓存合计不超过几 MB。在 Apple M 系列芯片上
/// 实测，2 万段、4 MB 的文档全部保留时二者占用 91 MB 堆内存。
pub const SHED_BLOCKS: usize = 512;

/// 光标所在的块前后各保留排版结果的块数。取自调参：32 块超过一屏能显示的段数，光标上下移动、
/// 翻页时落到的块都已排好。
pub const NEAR_BLOCKS: usize = 32;

/// 一帧中排版 [`Later::window`] 之外的块的时限：4 毫秒，约为 60 Hz 下一帧间隔的四分之一。
pub const LAYOUT_BUDGET: std::time::Duration = std::time::Duration::from_millis(4);

/// 内容一栏两侧仍按点在文字上处理的宽度（逻辑像素，经验值）。
pub(super) const CONTENT_SLACK: f32 = 12.0;

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

/// 预览上的一个位置：左边缘（相对内容左边）、所在那一行铺底色的上边缘（和滚动位置同一个
/// 量法）和高度，均为逻辑像素；见 [`super::Laid::point_of`]。
pub type Point = (f32, f32, f32);

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

/// 需要保留排版结果的块：内容坐标在 `rows` 范围内的块，以及序号在 `blocks` 范围内的块（光标和
/// 选区所在的块及其前后）。
#[derive(Debug, Clone, PartialEq)]
pub struct Near {
    /// 需要保留的内容纵坐标范围 `(top, bottom)`（逻辑像素）。
    pub rows: (f32, f32),
    /// 需要保留的顶层块序号区间列表。
    pub blocks: Vec<std::ops::Range<usize>>,
}

impl Near {
    /// 按本范围保留的排版结果是否够 `wanted` 使用：`wanted` 的各块都在其中，视口（高 `view`）
    /// 前后各留有一定余量。
    pub fn covers(&self, wanted: &Near, view: f32) -> bool {
        let rows = (wanted.rows.0 + view * 0.75, wanted.rows.1 - view * 0.75);
        let inside = |want: &std::ops::Range<usize>| {
            (self.blocks.iter()).any(|kept| kept.start <= want.start && want.end <= kept.end)
        };
        self.rows.0 <= rows.0 && rows.1 <= self.rows.1 && wanted.blocks.iter().all(inside)
    }

    /// 上边缘在 `top`、高 `height` 的第 `i` 块是否在范围内。
    pub(super) fn holds(&self, i: usize, top: f32, height: f32) -> bool {
        (top < self.rows.1 && top + height > self.rows.0)
            || self.blocks.iter().any(|blocks| blocks.contains(&i))
    }
}

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

/// [`super::Laid::build`] 所需的外部上下文：只传入排版真正依赖的原文、配色、图片池与缓存，
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
    /// Markdown 排版配色表。
    pub colors: Colors,
    /// 当前是否为深色主题。
    pub dark: bool,
    /// 代码块语法高亮配色回调。
    pub token: &'a dyn Fn(TokenKind) -> Color,
    /// 图片池（只读引用，按规范化路径查询已加载图片）。
    pub images: &'a HashMap<PathBuf, ImageSlot>,
    /// 将文档中的图片相对/绝对地址解析为本地路径的函数。
    pub image_path: &'a dyn Fn(&str) -> Option<PathBuf>,
    /// 跨块复用的排版测字缓存。
    pub memo: &'a mut Memo,
    /// 已切换展开/折叠状态的 `<details>` 或高图集合。
    pub toggled: &'a HashSet<u64>,
    /// 当前是否处于可编辑的实时预览模式。
    pub editable: bool,
    /// 解析结果是否与当前文档版本号一致。
    pub checked: bool,
    /// 若文档带 YAML 属性面板，替换第 0 块参与排版。
    pub properties: Option<winc_preview::markdown::Block>,
    /// 可选非侵入式装饰层（仅读取其中的块级虚拟挂件，在排版流中为其预留纵向槽位）。
    pub decorations: Option<&'a DecorationSet>,
}

/// 基线为 `baseline`、字号为 `size` 的一行文字铺底色时的上边缘和高度（逻辑像素）。
///
/// 选中底色、光标和行框共用这一套数，避免各算各的。
pub fn line_box(baseline: f32, size: f32) -> (f32, f32) {
    (baseline - size * 1.05, size * 1.4)
}

/// 判断块或其子块中是否包含带展开/收起状态的块（`<details>` 折叠块或可能折叠的 Mermaid 图）。
pub(super) fn block_has_toggle(block: &winc_preview::markdown::Block) -> bool {
    use winc_preview::markdown::Block;
    match block {
        Block::Details { .. } => true,
        Block::Code { lang, .. } => lang == "mermaid",
        Block::Quote(_, inner) | Block::Footnote(_, inner) => inner.iter().any(block_has_toggle),
        Block::List(_, items) => items
            .iter()
            .any(|item| item.blocks.iter().any(block_has_toggle)),
        _ => false,
    }
}

/// `c` 是否可以出现在一行原文开头、正文之前的标记中。
pub(super) fn is_markup(c: char) -> bool {
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

/// 根据当前光标所在叶子块与文档原文构造即时原文排版输入。
pub(super) fn build_live_raw(
    rope: Option<&ropey::Rope>,
    preedit: &str,
    live: Option<&LiveLeaf>,
) -> Option<(usize, Raw)> {
    let live = live?;
    let rope = rope?;
    let snap = |b: usize| rope.char_to_byte(rope.byte_to_char(b.min(rope.len_bytes())));
    let (start, end) = (snap(live.range.start), snap(live.range.end));
    let text: Arc<str> = rope.byte_slice(start..end).to_string().into();
    let leaf =
        snap(live.leaf.start).saturating_sub(start)..snap(live.leaf.end).saturating_sub(start);
    let caret = snap(live.at).saturating_sub(start);
    let preedit = ((live.prose || live.table) && !preedit.is_empty()).then(|| Arc::from(preedit));
    let parsed_end = live.parsed_end.saturating_sub(start);
    Some((
        live.block,
        Raw::new(text, leaf, parsed_end, caret, live.prose, preedit),
    ))
}

/// 用外部排版条件键刷新测字缓存环境。
pub(super) fn prepare_memo(memo: &mut Memo, key: Key) {
    memo.prepare(winc_typeset::memo_key(|h| {
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
}

/// 由外部输入上下文与绘制帧构造 `winc_typeset::Cx`。
pub(super) fn build_cx<'a, 'f>(
    input: LayoutInput<'a>,
    frame: &'a mut Frame<'f>,
    key: Key,
) -> (Cx<'a, 'f>, Option<winc_preview::markdown::Block>) {
    let properties = input.properties;
    let cx = Cx {
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
    (cx, properties)
}
