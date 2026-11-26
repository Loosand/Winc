//! 字体：按需 mmap 字体文件，不扫描系统字体库。
//!
//! 槽位依次是等宽字体、界面字体、等宽字体的斜体、数学字体、终端字体及其斜体，其后是按优先级
//! 排列的后备字体，最后是用户在字体列表中写在主字体之后的字体；斜体和后备字体在首次需要时才
//! 映射文件。候选路径按平台给出，用户配置的字体排在最前面。

use std::ops::Range;
use std::path::PathBuf;

use swash::{FontRef, StringId};

use crate::FontSpec;
use crate::cache::FastMap;
use crate::text::face::{Face, REGULAR, postscript_name};
#[cfg(test)]
use crate::text::face::{
    FontData, GLYPH_TABLES, LOADED, Source, instance_postscript, is_modifiable, load, select,
    weight_range,
};
use crate::text::font_path::FontFile;

/// 字体种类：编辑区、界面、正文、注释斜体、数学字体，以及终端的正体和斜体。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontKind {
    /// 编辑区的等宽字体。
    Mono,
    /// 界面字体（文件树、标签、状态栏）。
    Ui,
    /// 文档正文字体（Markdown 正文与标题）；未单独设置时与 [`FontKind::Ui`] 相同。
    Body,
    /// 等宽字体的斜体，用于注释。
    ///
    /// 字体没有斜体或斜体的字宽与正体不同时，把正体斜切代替。
    MonoItalic,
    /// 数学字体（Markdown 预览里的公式）：macOS 自带的 STIX Two Math 这类，字母、数字、
    /// 运算符风格一致。
    ///
    /// 系统没有时用界面字体。
    Math,
    /// 终端的等宽字体；没有单独设置终端字体时即 [`FontKind::Mono`]。
    Terminal,
    /// 终端字体的斜体；没有单独设置终端字体时即 [`FontKind::MonoItalic`]。
    TerminalItalic,
}

impl FontKind {
    /// 字体种类的个数。
    const COUNT: usize = 7;

    /// 是否为斜体：要斜体的字落到正体字体上时斜切。
    pub(crate) fn italic(self) -> bool {
        matches!(self, FontKind::MonoItalic | FontKind::TerminalItalic)
    }
}

/// 选后备字体时最多尝试的 fontconfig 结果数（经验值）：系统里没有合适的字体时不逐个打开全部字体。
const SORTED_FONTS: usize = 64;

/// macOS 系统自带的苹方所在的文件（macOS 15 起）。
const PINGFANG: &str =
    "/System/Library/PrivateFrameworks/FontServices.framework/Resources/Reserved/PingFangUI.ttc";

/// 可以设置的最小字重。
pub(crate) const WEIGHT_MIN: u16 = 100;
/// 可以设置的最大字重。
pub(crate) const WEIGHT_MAX: u16 = 900;

/// 字体槽位的状态。
enum Slot {
    /// 尚未映射，值是候选字体。
    Pending(Choices),
    /// 已映射，附带打开该槽位的字体文件（用于按需加载同族真粗体）。
    Ready(Face, Option<FontFile>),
    /// 候选文件都无法打开。
    Missing,
}

/// 常规槽位对应的真粗体槽位状态。
#[derive(Clone, Copy)]
enum BoldSlot {
    /// 尚未尝试加载真粗体。
    Pending,
    /// 已加载真粗体到指定槽位。
    Ready(u8),
    /// 该字体无可用真粗体，回退到模拟加粗。
    Unavailable,
}

/// 等宽字体所在的槽位；各槽位的顺序见模块文档。
pub(crate) const MONO: u8 = 0;
/// 界面字体所在的槽位。
const UI: u8 = 1;
/// 文档正文字体所在的槽位；未单独设置时为 `Missing`（使用界面字体）。
const BODY: u8 = 2;
/// 等宽字体的斜体所在的槽位。
const MONO_ITALIC: u8 = 3;
/// 数学字体所在的槽位。
const MATH: u8 = 4;
/// 终端字体所在的槽位；没有单独设置终端字体时为 `Missing`。
const TERMINAL: u8 = 5;
/// 终端字体的斜体所在的槽位。
const TERMINAL_ITALIC: u8 = 6;
/// 第一个后备字体的槽位。
const FALLBACK: u8 = 7;

/// 一项字体设置中最多使用的字体数（个）：槽位编号是 `u8`，三项设置的列表与平台后备字体合计
/// 不能超过 256 个槽位；常见的写法只有两三个字体。
const LISTED_FONTS: usize = 16;

/// 槽位 `slot` 中是否为斜体字体：要斜体的字落到其他槽位时斜切。
pub(crate) fn italic_slot(slot: u8) -> bool {
    matches!(slot, MONO_ITALIC | TERMINAL_ITALIC)
}

/// 字符解析结果缓存的最大项数（项，65536）。
///
/// 缓存项随出现过的字符数增长，满了就整体清空。该值是经验值。
const RESOLVED_ENTRIES: usize = 1 << 16;

/// 全部字体槽位及字符解析缓存。
pub(crate) struct Fonts {
    /// 字体槽位，顺序见模块文档。
    slots: Vec<Slot>,
    /// 各槽位对应的真粗体槽位状态，与 `slots` 一一对应。
    bold_of: Vec<BoldSlot>,
    /// 各字体的字重偏移（见 [`Face::open_at`]）。
    shift: i16,
    /// 字符解析结果缓存：（字体种类，字符）→（槽位，字形 id）。
    resolved: FastMap<(FontKind, char), (u8, u16)>,
    /// 粗体字符解析结果缓存：（字体种类，字符）→（槽位，字形 id，是否需要模拟加粗）。
    resolved_bold: FastMap<(FontKind, char), (u8, u16, bool)>,
    /// ASCII 的快速路径：`[字体种类][码点]` →（槽位，字形 id），字体里没有该字符时为 `None`。
    ///
    /// 用于避免每个字符都查哈希表。
    ascii: [[Option<(u8, u16)>; 128]; FontKind::COUNT],
    /// 粗体 ASCII 的快速路径：`[字体种类][码点]` →（槽位，字形 id，是否需要模拟加粗）。
    ascii_bold: [[Option<(u8, u16, bool)>; 128]; FontKind::COUNT],
    /// 平台后备字体的槽位范围。
    fallbacks: Range<u8>,
    /// 彩色 emoji 字体所在的后备槽位；默认按 emoji 显示的字符先在这里找。
    emoji: u8,
    /// 等宽字体、界面字体、正文字体和终端字体的列表中写在主字体之后的字体所在的槽位范围，依次尝试。
    listed: [Range<u8>; 4],
}

impl Fonts {
    /// 加载字体；`mono`、`ui`、`body` 和 `terminal` 是用户配置里指定的字体列表，为空时等宽字体和界面
    /// 字体用内置候选，正文用界面字体，终端用等宽字体。
    ///
    /// 列表中第一个能打开的字体是主字体，其余的字体在主字体缺字时依次尝试。指定的正文或终端字体都打不
    /// 开时退回界面或等宽字体，不报错。`weight` 是全部文字的字重，超出 100 至 900 时取最接近的值。
    ///
    /// # Errors
    ///
    /// 所有候选中都找不到可用的等宽字体时，返回列出已尝试路径的错误描述。
    ///
    /// # Panics
    ///
    /// 等宽字体成功打开后又立即无法打开时 panic。
    pub(crate) fn load(
        mono: &[FontSpec],
        ui: &[FontSpec],
        body: &[FontSpec],
        terminal: &[FontSpec],
        weight: u16,
    ) -> Result<Fonts, String> {
        let shift = weight.clamp(WEIGHT_MIN, WEIGHT_MAX) as i16 - REGULAR as i16;
        // 指定的字体不可用时退回内置候选而不报错：字体配置有误时程序仍可启动。
        let listed = [mono, ui, body, terminal].map(resolve_list);
        let candidates = Candidates::for_platform(&listed[0], &listed[1]);
        let (mono, mono_file) = first_opened(&candidates.mono, shift).ok_or_else(|| {
            format!(
                "找不到等宽字体，尝试过：{:?}。可以用 WINC_FONT 指定字体文件，\
                 或在配置里设置 editor.font。",
                candidates.mono.iter().map(|f| &f.path).collect::<Vec<_>>()
            )
        })?;

        // 没有界面字体时用等宽字体代替。
        let (ui, ui_file) = first_opened(&candidates.ui, shift)
            .or_else(|| first_opened(&candidates.mono, shift))
            .expect("等宽字体刚刚成功打开过");

        let (body_slot, body_file) = match first_opened(&listed[2], shift) {
            Some((face, file)) => (Slot::Ready(face, Some(file.clone())), Some(file)),
            None => (Slot::Missing, None),
        };

        let italic = italic_candidates(&mono_file, &mono);
        // 数学字体一开始就映射（只是映射文件，很快）：度量要用到它，`metrics` 不能再去加载。
        let math = first_opened(&candidates.math, shift)
            .map_or(Slot::Missing, |(face, file)| Slot::Ready(face, Some(file)));
        // 终端字体同样一开始就映射：度量要用到它。
        let (terminal, terminal_italic, terminal_file) = match first_opened(&listed[3], shift) {
            Some((face, file)) => {
                let italic = italic_candidates(&file, &face);
                (
                    Slot::Ready(face, Some(file.clone())),
                    Slot::Pending(Choices::files(italic)),
                    Some(file),
                )
            }
            None => (Slot::Missing, Slot::Missing, None),
        };
        let mut slots = vec![
            Slot::Ready(mono, Some(mono_file.clone())),
            Slot::Ready(ui, Some(ui_file.clone())),
            body_slot,
            Slot::Pending(Choices::files(italic)),
            math,
            terminal,
            terminal_italic,
        ];
        let emoji = FALLBACK + candidates.emoji as u8;
        slots.extend(candidates.fallbacks.into_iter().map(Slot::Pending));
        let fallbacks = FALLBACK..slots.len() as u8;
        // 列表中主字体之外的字体到第一次用到时才打开。排在主字体之前的字体刚才已打不开，到时
        // 再试一次即成为 `Missing`。没有正文或终端字体时，对应列表不再使用。
        let chosen = [
            Some(&mono_file),
            Some(&ui_file),
            body_file.as_ref(),
            terminal_file.as_ref(),
        ];
        let listed = std::array::from_fn(|i| {
            let start = slots.len() as u8;
            if chosen[i].is_some() {
                let rest = listed[i].iter().filter(|file| Some(*file) != chosen[i]);
                slots.extend(rest.map(|file| Slot::Pending(Choices::files(vec![file.clone()]))));
            }
            start..slots.len() as u8
        });

        let bold_of = vec![BoldSlot::Pending; slots.len()];
        let mut fonts = Fonts {
            slots,
            bold_of,
            shift,
            resolved: FastMap::default(),
            resolved_bold: FastMap::default(),
            ascii: [[None; 128]; FontKind::COUNT],
            ascii_bold: [[None; 128]; FontKind::COUNT],
            fallbacks,
            emoji,
            listed,
        };

        for kind in [FontKind::Mono, FontKind::Ui, FontKind::Math] {
            fonts.fill_ascii(kind);
        }
        if fonts.face(BODY).is_some() {
            fonts.fill_ascii(FontKind::Body);
        }
        if fonts.face(TERMINAL).is_some() {
            fonts.fill_ascii(FontKind::Terminal);
        }

        Ok(fonts)
    }

    /// 返回实际使用的字体种类：没有单独的终端字体时，终端的正体、斜体即等宽字体的正体、斜体；
    /// 没有单独的正文字体时，正文即界面字体。
    fn effective(&self, kind: FontKind) -> FontKind {
        let separate_terminal = self.face(TERMINAL).is_some();
        let separate_body = self.face(BODY).is_some();
        match kind {
            FontKind::Body if !separate_body => FontKind::Ui,
            FontKind::Terminal if !separate_terminal => FontKind::Mono,
            FontKind::TerminalItalic if !separate_terminal => FontKind::MonoItalic,
            kind => kind,
        }
    }

    /// 返回斜体种类 `kind` 对应的（斜体槽位，正体槽位）；不是斜体时返回 `None`。
    fn italic_slots(kind: FontKind) -> Option<(u8, u8)> {
        match kind {
            FontKind::MonoItalic => Some((MONO_ITALIC, MONO)),
            FontKind::TerminalItalic => Some((TERMINAL_ITALIC, TERMINAL)),
            _ => None,
        }
    }

    /// 按 `kind` 主槽位中的字体填写 ASCII 快速路径；槽位中还没有字体时什么也不做。
    ///
    /// 斜体槽位确定没有可用的字体（`Missing`）时，注释里的 ASCII 字符本来就落到正体上（绘制时
    /// 再斜切），按正体填写，同样走快速路径；斜体还没加载（`Pending`）时不填，以免先把正体占上。
    fn fill_ascii(&mut self, kind: FontKind) {
        let primary = self.primary(kind);
        let slot = match (kind, &self.slots[primary as usize]) {
            (FontKind::MonoItalic, Slot::Missing) => MONO,
            (FontKind::TerminalItalic, Slot::Missing) => TERMINAL,
            (FontKind::Math, Slot::Missing) => UI,
            _ => primary,
        };
        let Some(Slot::Ready(face, _)) = self.slots.get(slot as usize) else {
            return;
        };
        let charmap = face.font().charmap();

        for (c, entry) in self.ascii[kind as usize].iter_mut().enumerate() {
            let glyph = charmap.map(c as u32);
            *entry = (glyph != 0).then_some((slot, glyph));
        }
    }

    /// 返回字体种类 `kind` 对应的主槽位。
    pub(crate) fn primary(&self, kind: FontKind) -> u8 {
        match self.effective(kind) {
            FontKind::Mono => MONO,
            FontKind::Ui => UI,
            FontKind::Body => BODY,
            FontKind::MonoItalic => MONO_ITALIC,
            FontKind::Math => MATH,
            FontKind::Terminal => TERMINAL,
            FontKind::TerminalItalic => TERMINAL_ITALIC,
        }
    }

    /// 字体种类 `kind` 的主字体；数学字体没有时是界面字体。
    pub(crate) fn primary_face(&self, kind: FontKind) -> Option<&Face> {
        self.face(self.primary(kind)).or_else(|| self.face(UI))
    }

    /// 首次用到斜体种类 `kind` 时映射它的斜体字体。
    ///
    /// 斜体的字宽与正体不同时不使用斜体（字符必须落在等宽的格子里），改为把正体斜切代替。
    fn load_italic(&mut self, kind: FontKind) {
        let Some((italic, upright)) = Self::italic_slots(kind) else {
            return;
        };
        if !matches!(self.slots[italic as usize], Slot::Pending(_)) {
            return;
        }
        self.ensure_loaded(italic);

        let width = |face: &Face| {
            let font = face.font();
            font.glyph_metrics(&face.coords)
                .advance_width(font.charmap().map('0'))
                / f32::from(font.metrics(&face.coords).units_per_em.max(1))
        };
        if let (Some(regular), Some(slanted)) = (self.face(upright), self.face(italic))
            && (width(regular) - width(slanted)).abs() > 0.005
        {
            self.slots[italic as usize] = Slot::Missing;
        }
        self.fill_ascii(kind);
    }

    /// 返回槽位 `slot` 中已映射的字体；尚未映射或不存在时返回 `None`。
    pub(crate) fn face(&self, slot: u8) -> Option<&Face> {
        match self.slots.get(slot as usize) {
            Some(Slot::Ready(face, _)) => Some(face),
            _ => None,
        }
    }

    /// 槽位 `slot` 是否是彩色 emoji 字体的槽位。
    pub(crate) fn is_emoji_slot(&self, slot: u8) -> bool {
        slot == self.emoji
    }

    /// 返回彩色 emoji 字体的槽位和字体，必要时才映射文件；系统中没有时返回 `None`。
    pub(crate) fn emoji_face(&mut self) -> Option<(u8, &Face)> {
        let slot = self.emoji;
        self.ensure_loaded(slot);
        Some((slot, self.face(slot)?))
    }

    /// 返回能显示字符 `c` 的（槽位，字形 id）。
    ///
    /// 依次尝试 `kind` 对应的主字体、另一种主字体和各后备字体，必要时才映射文件。所有字体都没有
    /// 该字符时返回（主槽位，0），即该字体的 `.notdef` 字形。
    pub(crate) fn resolve(&mut self, kind: FontKind, c: char) -> (u8, u16) {
        let kind = self.effective(kind);
        if kind.italic() {
            self.load_italic(kind);
        }
        let primary = self.primary(kind);

        if (c as u32) < 128
            && let Some(hit) = self.ascii[kind as usize][c as usize]
        {
            return hit;
        }

        if let Some(&hit) = self.resolved.get(&(kind, c)) {
            return hit;
        }

        // 先找主字体及其列表中的字体，再找其他主字体及其列表（斜体没有的字符用正体，终端字体
        // 没有的用等宽字体，正文没有的用界面字体），最后是后备字体。斜体、终端字体不作为其他字体的后备。
        let [mono, ui, body, terminal] = self.listed.clone();
        let with = |slot: u8, listed: Range<u8>| std::iter::once(slot).chain(listed);
        let mono = with(MONO, mono);
        let ui = with(UI, ui);
        let mut order: Vec<u8> = match kind {
            FontKind::Mono => mono.chain(ui).collect(),
            FontKind::Ui => ui.chain(mono).collect(),
            FontKind::Body => with(BODY, body).chain(ui).chain(mono).collect(),
            FontKind::MonoItalic => [MONO_ITALIC].into_iter().chain(mono).chain(ui).collect(),
            FontKind::Math => [MATH].into_iter().chain(ui).chain(mono).collect(),
            FontKind::Terminal => with(TERMINAL, terminal).chain(mono).chain(ui).collect(),
            FontKind::TerminalItalic => [TERMINAL_ITALIC]
                .into_iter()
                .chain(with(TERMINAL, terminal))
                .chain(mono)
                .chain(ui)
                .collect(),
        };
        order.extend(self.fallbacks.clone());
        // 默认按 emoji 显示的字符优先使用彩色 emoji 字体：顺序在前的符号字体、
        // 中文字体和主字体常带有同一字符的单色字形。
        if winc_emoji::presentation(c) {
            order.insert(0, self.emoji);
        }

        let mut result = (primary, 0);
        // 缓存项数随出现过的字符数增长（例如打开一个几乎每个字符都不同的文件），满了就整体清空；
        // 之后重新解析只需再查几次字体的字符映射表。
        if self.resolved.len() >= RESOLVED_ENTRIES {
            self.resolved = FastMap::default();
        }

        for slot in order {
            self.ensure_loaded(slot);

            if let Some(face) = self.face(slot) {
                let glyph = face.font().charmap().map(c);

                if glyph != 0 {
                    result = (slot, glyph);
                    break;
                }
            }
        }

        self.resolved.insert((kind, c), result);
        result
    }

    /// 返回粗体字符 `c` 对应的（槽位，字形 id，是否需要模拟加粗）。
    ///
    /// 优先为常规命中的槽位按需加载真粗体（可变字体 `wght` 轴、同族粗体文件或系统粗体命名实例）；
    /// 当真粗体不可用或缺少该字符时，回退到常规槽位并置 `faux_bold = true`。
    pub(crate) fn resolve_bold(&mut self, kind: FontKind, c: char) -> (u8, u16, bool) {
        let kind = self.effective(kind);
        if (c as u32) < 128
            && let Some(hit) = self.ascii_bold[kind as usize][c as usize]
        {
            return hit;
        }
        if let Some(&hit) = self.resolved_bold.get(&(kind, c)) {
            return hit;
        }
        let (slot, glyph) = self.resolve(kind, c);
        let result = if glyph == 0 || self.is_emoji_slot(slot) {
            (slot, glyph, false)
        } else if let Some(bold_slot) = self.ensure_bold_slot(slot)
            && let Some(face) = self.face(bold_slot)
            && let bold_glyph = face.font().charmap().map(c)
            && bold_glyph != 0
        {
            (bold_slot, bold_glyph, false)
        } else {
            (slot, glyph, true)
        };
        if (c as u32) < 128 {
            self.ascii_bold[kind as usize][c as usize] = Some(result);
        } else {
            if self.resolved_bold.len() >= RESOLVED_ENTRIES {
                self.resolved_bold = FastMap::default();
            }
            self.resolved_bold.insert((kind, c), result);
        }
        result
    }

    /// 按需为常规槽位 `slot` 加载对应的真粗体槽位；不存在或无法加载时记住 `Unavailable`。
    fn ensure_bold_slot(&mut self, slot: u8) -> Option<u8> {
        let idx = slot as usize;
        match *self.bold_of.get(idx)? {
            BoldSlot::Ready(bold) => return Some(bold),
            BoldSlot::Unavailable => return None,
            BoldSlot::Pending => {}
        }
        let file = match self.slots.get(idx) {
            Some(Slot::Ready(_, Some(file))) => file.clone(),
            _ => {
                self.bold_of[idx] = BoldSlot::Unavailable;
                return None;
            }
        };
        if self.slots.len() >= u8::MAX as usize {
            self.bold_of[idx] = BoldSlot::Unavailable;
            return None;
        }
        match Face::open_bold(&file, self.shift) {
            Some(face) => {
                let bold_slot = self.slots.len() as u8;
                self.slots.push(Slot::Ready(face, None));
                self.bold_of.push(BoldSlot::Unavailable);
                self.bold_of[idx] = BoldSlot::Ready(bold_slot);
                Some(bold_slot)
            }
            None => {
                self.bold_of[idx] = BoldSlot::Unavailable;
                None
            }
        }
    }

    /// 槽位 `slot` 仍是 `Pending` 时，尝试映射其候选文件，并把槽位更新为 `Ready` 或 `Missing`。
    fn ensure_loaded(&mut self, slot: u8) {
        let Some(entry) = self.slots.get_mut(slot as usize) else {
            return;
        };

        if let Slot::Pending(choices) = entry {
            *entry = match choices.open(self.shift) {
                Some((face, file)) => Slot::Ready(face, Some(file)),
                None => Slot::Missing,
            };
        }
    }
}

/// 返回用户配置的字体列表解析出的候选，按列表的顺序；解析不出来的字体略去，超过
/// [`LISTED_FONTS`] 个的部分不用。都解析不出来时为空列表，调用方会按平台默认字体处理。
fn resolve_list(specs: &[FontSpec]) -> Vec<FontFile> {
    let mut files: Vec<FontFile> = Vec::new();
    for file in specs
        .iter()
        .take(LISTED_FONTS)
        .filter_map(FontSpec::resolve)
    {
        if !files.contains(&file) {
            files.push(file);
        }
    }
    files
}
/// 返回 `files` 中第一个能打开的字体及其文件；都打不开时返回 `None`。
fn first_opened(files: &[FontFile], shift: i16) -> Option<(Face, FontFile)> {
    files
        .iter()
        .find_map(|file| Face::open_at(file, shift).map(|face| (face, file.clone())))
}

/// 返回等宽字体 `file`（已打开为 `face`）的斜体候选，按优先级排列。
///
/// 先在同一目录下按常见的命名找文件，只做几次 `stat`：`X-Regular.ttf` → `X-Italic.ttf`，
/// `X[wght].ttf` → `X-Italic[wght].ttf`，`consola.ttf` → `consolai.ttf`，`.ttc` 集合中
/// `Menlo-Regular` → `Menlo-Italic`。都没有时才按字族名向系统查询（macOS 上一次约几毫秒）。
fn italic_candidates(file: &FontFile, face: &Face) -> Vec<FontFile> {
    let mut out = Vec::new();
    let path = &file.path;
    let (Some(stem), Some(ext)) = (
        path.file_stem().and_then(|s| s.to_str()),
        path.extension().and_then(|s| s.to_str()),
    ) else {
        return out;
    };

    if ext.eq_ignore_ascii_case("ttc") {
        let postscript = file
            .postscript
            .clone()
            .or_else(|| postscript_name(&face.font()));
        if let Some(name) = postscript {
            let base = name.strip_suffix("-Regular").unwrap_or(&name);
            out.push(FontFile {
                path: path.clone(),
                postscript: Some(format!("{base}-Italic")),
            });
        }
    } else {
        let mut names = Vec::new();
        if stem.contains("Regular") {
            names.push(stem.replacen("Regular", "Italic", 1));
        }
        if let Some((name, axes)) = stem.split_once('[') {
            names.push(format!("{name}-Italic[{axes}"));
        }
        for suffix in ["Italic", "-Italic", "-Oblique", "i"] {
            names.push(format!("{stem}{suffix}"));
        }
        out.extend(
            names
                .into_iter()
                .map(|name| path.with_file_name(format!("{name}.{ext}")))
                .filter(|p| p.is_file())
                .map(FontFile::first),
        );
    }

    if out.is_empty()
        && let Some(family) = family_name(&face.font())
        && let Some(found) = crate::text::font_path::resolve_face(&family, false, true)
        && found.italic
    {
        out.push(found.file);
    }
    out
}

/// 返回字体的字族名（如 `Menlo`）；字体里没有时返回 `None`。
fn family_name(font: &FontRef) -> Option<String> {
    let name = font
        .localized_strings()
        .find_by_id(StringId::Family, None)?;
    Some(name.to_string())
}

/// 返回 fontconfig 中与 `pattern`（如 `monospace`、`sans-serif:lang=zh-cn`）最匹配的字体。
///
/// 通过 `fc-match` 命令查询，不链接 fontconfig 库。没有 `fc-match`、查询失败或给出的文件不存在
/// 时返回 `None`。
fn fontconfig(pattern: &str) -> Option<FontFile> {
    let output = std::process::Command::new("fc-match")
        .args(["--format", "%{file}\n%{postscriptname}", pattern])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let (file, postscript) = text.split_once('\n').unwrap_or((&text, ""));
    let path = PathBuf::from(file);
    path.is_file().then(|| FontFile {
        path,
        postscript: (!postscript.is_empty()).then(|| postscript.to_owned()),
    })
}

/// 返回 fontconfig 为 `pattern` 排序的字体，最匹配的在前，最多 [`SORTED_FONTS`] 个。
///
/// `fc-match -s` 的结果按用户的偏好排序，排在最前的不一定含有查询的语言的文字：用户把不含
/// 汉字的字体强制绑定到 `sans-serif` 时，它在 `lang=zh-cn` 的查询中同样排第一。没有 `fc-match`
/// 或查询失败时返回空列表。
fn fontconfig_sorted(pattern: &str) -> Vec<FontFile> {
    let output = std::process::Command::new("fc-match")
        .args([
            "--sort",
            "--format",
            "%{file}\n%{postscriptname}\n",
            pattern,
        ])
        .stderr(std::process::Stdio::null())
        .output();
    match output {
        Ok(output) if output.status.success() => {
            parse_sorted_fonts(&String::from_utf8_lossy(&output.stdout))
        }
        _ => Vec::new(),
    }
}

/// 解析 [`fontconfig_sorted`] 的输出：每个字体两行，依次是文件路径和 PostScript 名；只保留存在的
/// 文件。
fn parse_sorted_fonts(text: &str) -> Vec<FontFile> {
    let lines: Vec<&str> = text.split('\n').collect();
    lines
        .as_chunks::<2>()
        .0
        .iter()
        .map(|[path, name]| FontFile {
            path: PathBuf::from(path),
            postscript: (!name.is_empty()).then(|| (*name).to_owned()),
        })
        .filter(|font| font.path.is_file())
        .take(SORTED_FONTS)
        .collect()
}

/// 各字体槽位的候选字体。
struct Candidates {
    /// 等宽字体的候选，按优先级排列。
    mono: Vec<FontFile>,
    /// 界面字体的候选，按优先级排列。
    ui: Vec<FontFile>,
    /// 数学字体的候选，按优先级排列。
    math: Vec<FontFile>,
    /// 每一项是一个后备槽位的候选。
    fallbacks: Vec<Choices>,
    /// 彩色 emoji 字体在 `fallbacks` 中的下标（从 0 起）。
    emoji: usize,
}

/// 一个后备槽位的候选：先按序尝试 fontconfig 的查询结果（有查询时），再按优先级尝试固定的字体
/// 文件。
///
/// 后备槽位在第一次用到时才打开，fontconfig 也在那时查询。
struct Choices {
    /// 交给 fontconfig 的查询，如 `sans-serif:lang=zh-cn`。
    query: Option<String>,
    /// 固定的候选字体文件，按优先级排列。
    files: Vec<FontFile>,
    /// 槽位要显示的代表字符；不为 `None` 时选用第一个含有该字符的字体。
    probe: Option<char>,
}

impl Choices {
    /// 只有固定字体文件的候选。
    fn files(files: Vec<FontFile>) -> Choices {
        Choices {
            query: None,
            files,
            probe: None,
        }
    }

    /// 打开第一个含有 [`Self::probe`] 的候选字体及其文件；没有 `probe` 时打开第一个能打开的。
    ///
    /// 都不含 `probe` 时返回第一个能打开的字体，都打不开时返回 `None`。`shift` 是字重的偏移。
    fn open(&self, shift: i16) -> Option<(Face, FontFile)> {
        let asked = self
            .query
            .as_deref()
            .map_or_else(Vec::new, fontconfig_sorted);
        let mut first = None;
        let faces = asked.iter().chain(&self.files);
        for (face, file) in
            faces.filter_map(|file| Face::open_at(file, shift).map(|f| (f, file.clone())))
        {
            let Some(probe) = self.probe else {
                return Some((face, file));
            };
            if face.font().charmap().map(probe) != 0 {
                return Some((face, file));
            }
            first.get_or_insert((face, file));
        }
        first
    }
}

impl Candidates {
    /// 返回各槽位的候选字体：用户配置的字体排在最前面，其后是平台默认字体。
    fn for_platform(mono: &[FontFile], ui: &[FontFile]) -> Candidates {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();

        // `WINC_FONT` 环境变量指定的字体排在最前面，其次是用户配置的字体，便于临时试用其他字体。
        let mut mono = mono.to_vec();
        if let Some(font) = std::env::var_os("WINC_FONT") {
            mono.insert(0, FontFile::first(font));
        }
        let mut ui_paths = ui.to_vec();

        let user_fonts = |name: &str| -> Vec<FontFile> {
            vec![
                FontFile::first(home.join("Library/Fonts").join(name)),
                FontFile::first(PathBuf::from("/Library/Fonts").join(name)),
            ]
        };
        let paths = |list: &[&str]| -> Vec<FontFile> {
            list.iter().copied().map(FontFile::first).collect()
        };
        if cfg!(target_os = "macos") {
            mono.extend(user_fonts("JetBrainsMono-Regular.ttf"));
            mono.extend(user_fonts("JetBrainsMono[wght].ttf"));
            mono.extend(paths(&[
                "/System/Library/Fonts/SFNSMono.ttf",
                "/System/Library/Fonts/Menlo.ttc",
                "/System/Library/Fonts/Monaco.ttf",
            ]));

            Candidates {
                mono,
                ui: {
                    ui_paths.extend(paths(&[
                        "/System/Library/Fonts/SFNS.ttf",
                        "/System/Library/Fonts/Helvetica.ttc",
                    ]));
                    ui_paths
                },
                math: paths(&["/System/Library/Fonts/Supplemental/STIXTwoMath.otf"]),
                fallbacks: vec![
                    // 中文先用苹方（系统界面的中文字体）。macOS 15 起它的字形只在 `hvgl` 表中，
                    // 由系统提供轮廓；文件不存在时（更早的系统）用冬青黑体。
                    std::iter::once(FontFile {
                        path: PINGFANG.into(),
                        postscript: Some("PingFangSC-Regular".to_owned()),
                    })
                    .chain(paths(&[
                        "/System/Library/Fonts/Hiragino Sans GB.ttc",
                        "/System/Library/Fonts/STHeiti Medium.ttc",
                        "/System/Library/Fonts/STHeiti Light.ttc",
                    ]))
                    .collect(),
                    paths(&["/System/Library/Fonts/Menlo.ttc"]),
                    paths(&["/System/Library/Fonts/Apple Symbols.ttf"]),
                    paths(&["/System/Library/Fonts/Apple Color Emoji.ttc"]),
                    paths(&["/System/Library/Fonts/AppleSDGothicNeo.ttc"]),
                ]
                .into_iter()
                .map(Choices::files)
                .collect(),
                emoji: 3,
            }
        } else if cfg!(target_os = "windows") {
            let fonts = crate::text::font_path::windows_fonts();
            let system = |list: &[&str]| -> Vec<FontFile> {
                list.iter()
                    .map(|name| FontFile::first(fonts.join(name)))
                    .collect()
            };
            mono.extend(system(&["CascadiaMono.ttf", "consola.ttf"]));

            Candidates {
                mono,
                ui: {
                    ui_paths.extend(system(&["segoeui.ttf"]));
                    ui_paths
                },
                math: vec![FontFile {
                    path: fonts.join("cambria.ttc"),
                    postscript: Some("CambriaMath".to_owned()),
                }],
                fallbacks: vec![
                    system(&["msyh.ttc", "simsun.ttc"]),
                    system(&["seguisym.ttf"]),
                    system(&["seguiemj.ttf"]),
                ]
                .into_iter()
                .map(Choices::files)
                .collect(),
                emoji: 2,
            }
        } else {
            // Linux 先查询 fontconfig：它记录了发行版安装字体的位置，也按用户设置的偏好选择；没有
            // fontconfig 的精简系统再试后面这些常见的位置。等宽字体和界面字体启动时就要用到，因此
            // 同时查询（各约几毫秒），中文和 emoji 的后备字体到第一次用到时再查。应用指定了语言时，
            // 等宽字体和界面字体也按该语言选，因为 fontconfig 默认按系统语言选。
            let language = app_language().map_or_else(String::new, |lang| format!(":lang={lang}"));
            let (asked_mono, asked_ui) = std::thread::scope(|scope| {
                let ui = scope.spawn(|| fontconfig(&format!("sans-serif{language}")));
                (
                    fontconfig(&format!("monospace{language}")),
                    ui.join().ok().flatten(),
                )
            });
            mono.extend(asked_mono);
            mono.extend(paths(&[
                "/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-Regular.ttf",
                "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
                "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
                "/usr/share/fonts/liberation/LiberationMono-Regular.ttf",
            ]));
            ui_paths.extend(asked_ui);
            ui_paths.extend(paths(&[
                "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
                "/usr/share/fonts/TTF/DejaVuSans.ttf",
                "/usr/share/fonts/noto/NotoSans-Regular.ttf",
            ]));

            Candidates {
                mono,
                ui: ui_paths,
                math: paths(&[
                    "/usr/share/fonts/opentype/stix/STIXTwoMath-Regular.otf",
                    "/usr/share/fonts/opentype/lmodern/latinmodern-math.otf",
                    "/usr/share/fonts/texlive-lm-math/latinmodern-math.otf",
                ]),
                fallbacks: vec![
                    Choices {
                        query: Some(format!("sans-serif:lang={}", cjk_language())),
                        files: paths(&[
                            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
                            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
                            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
                        ]),
                        probe: Some('中'),
                    },
                    Choices {
                        query: Some("emoji".to_owned()),
                        files: paths(&["/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf"]),
                        probe: Some('😀'),
                    },
                ],
                emoji: 1,
            }
        }
    }
}

/// 应用指定的、选日韩文字体用的语言；为 `None` 时按系统语言选。见 [`set_cjk_language`]。
static CJK_LANGUAGE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// 设置选日韩文字体用的语言（如 `zh-cn`），使汉字按该语言的字形显示；`None` 表示按系统语言选。
///
/// 应用的界面语言是中文时传入对应的语言，使界面文字与字形一致。只有之后加载的字体（新建
/// 渲染器、[`crate::Renderer::set_fonts`]）按新语言选。返回语言是否改变，改变时调用方应重新
/// 加载字体。目前只有 Linux 按语言选字体，其他平台的中文字体是固定的。
pub fn set_cjk_language(language: Option<&str>) -> bool {
    let mut current = CJK_LANGUAGE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let changed = current.as_deref() != language;
    *current = language.map(str::to_owned);
    changed
}

/// 返回应用指定的、选日韩文字体用的语言；没有指定时返回 `None`。
fn app_language() -> Option<String> {
    CJK_LANGUAGE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// 返回选日韩文字体用的语言：应用指定了就用它，否则按系统语言（`LC_ALL`、`LC_CTYPE`、
/// `LANG`）选，见 [`cjk_language_of`]。
fn cjk_language() -> String {
    if let Some(language) = app_language() {
        return language;
    }
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
        .unwrap_or_default();
    cjk_language_of(&locale)
}

/// 返回选日韩文字体用的语言：系统语言 `locale`（如 `zh_TW.UTF-8`）是中文、日文或韩文时用它
/// （`zh-tw`），汉字按该地区的字形显示；其他语言用简体中文 `zh-cn`。
fn cjk_language_of(locale: &str) -> String {
    let tag = locale
        .split(['.', '@'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .replace('_', "-");
    let language = tag.split('-').next().unwrap_or_default();
    if matches!(language, "ja" | "ko") || (language == "zh" && tag != "zh") {
        tag
    } else {
        "zh-cn".to_owned()
    }
}

#[cfg(test)]
pub(crate) mod tests;
