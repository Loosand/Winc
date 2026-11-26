//! 各类块的排版：标题、引用、独占一行的公式和可折叠的块。

use super::*;

impl Cx<'_, '_> {
    /// 排一个 `level` 级标题；一级标题下方加一道横线，并在 `out` 中记下标题的锚点。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn heading(
        &mut self,
        level: u8,
        inline: &Inline,
        x: f32,
        y: f32,
        width: f32,
        env: Env,
        out: &mut Out,
    ) -> Flow {
        self.heading_with_spaces(level, inline, x, y, width, env, false, out)
    }

    /// 排实时编辑中的 `level` 级标题（保留纯空格文字段以便推动光标）。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn heading_live(
        &mut self,
        level: u8,
        inline: &Inline,
        x: f32,
        y: f32,
        width: f32,
        env: Env,
        out: &mut Out,
    ) -> Flow {
        self.heading_with_spaces(level, inline, x, y, width, env, true, out)
    }

    #[allow(clippy::too_many_arguments)]
    fn heading_with_spaces(
        &mut self,
        level: u8,
        inline: &Inline,
        x: f32,
        y: f32,
        width: f32,
        env: Env,
        keep_spaces: bool,
        out: &mut Out,
    ) -> Flow {
        let base = self.base;
        let scale = [2.0, 1.5, 1.25, 1.0, 0.875, 0.85][(level as usize).clamp(1, 6) - 1];
        let size = base * scale;
        let color = if level == 6 {
            self.colors.muted
        } else {
            env.color
        };
        let para = Para {
            size,
            bold: true,
            color,
            line: 1.25,
            align: Align::None,
            keep_spaces,
        };
        out.anchors.push((slug(&inline.text), y));
        let mut flow = self.flow(inline, x, y, width, para, &mut out.items);
        if level == 1 {
            let line_y = y + flow.height + size * 0.3;
            out.items.push(Item::Fill {
                rect: Rect::new(x, line_y, width, 1.0),
                color: self.colors.border,
                radius: 0.0,
            });
            flow.height += size * 0.3 + 1.0;
        }
        flow
    }

    /// 排一段引用：左侧一道竖线，内容缩进；`alert` 不为 `None` 时是提示块，竖线和标题用其颜色。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn quote(
        &mut self,
        alert: Option<Alert>,
        blocks: &[Block],
        x: f32,
        y: f32,
        width: f32,
        env: Env,
        out: &mut Out,
    ) -> Flow {
        let base = self.base;
        let para = self.body_para(env);
        let bar = (base * 0.25).round();
        let inner = x + bar + base;
        let room = (width - bar - base).max(1.0);
        let (bar_color, env) = match alert {
            Some(alert) => (Colors::alert(self.dark, alert), env),
            None => (
                self.colors.border,
                Env {
                    color: self.colors.muted,
                    ..env
                },
            ),
        };
        let mut top = y;
        if let Some(alert) = alert {
            let title = match alert {
                Alert::Note => "Note",
                Alert::Tip => "Tip",
                Alert::Important => "Important",
                Alert::Warning => "Warning",
                Alert::Caution => "Caution",
            };
            let inline = Inline {
                text: title.to_owned(),
                runs: vec![winc_preview::markdown::Run {
                    end: title.len(),
                    style: Style::default(),
                }],
                ..Inline::default()
            };
            let para = Para {
                bold: true,
                color: bar_color,
                ..para
            };
            let flow = self.flow(&inline, inner, top, room, para, &mut out.items);
            top += flow.height + base * 0.25;
        }
        let flow = self.blocks(blocks, inner, top, room, env, out);
        let height = top + flow.height - y;
        out.items.push(Item::Fill {
            rect: Rect::new(x, y, bar, height),
            color: bar_color,
            radius: 0.0,
        });
        Flow {
            height,
            first_baseline: flow.first_baseline,
        }
    }

    /// 排一个独占一行的公式，水平居中；`source` 是公式在原文中的位置，点击公式时光标移到那里。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn math_block(
        &mut self,
        tex: &str,
        source: usize,
        x: f32,
        y: f32,
        width: f32,
        env: Env,
        out: &mut Out,
    ) -> Flow {
        let base = self.base;
        let mut b = crate::math::layout(self.frame, self.dark, tex, base, true, env.color);
        // 比一行宽的缩小（最多缩到一半），还宽就让它超出去。
        if b.w > width {
            let size = base * (width / b.w).max(0.5);
            b = crate::math::layout(self.frame, self.dark, tex, size, true, env.color);
        }
        let pad = base * 0.5;
        let (dx, baseline) = (x + ((width - b.w) / 2.0).max(0.0), y + pad + b.asc);
        out.items
            .extend(b.items.iter().map(|item| item.moved(dx, baseline, 0)));
        // 点击公式时光标移到公式的原文处，进入编辑。
        let height = pad * 2.0 + b.asc + b.desc;
        out.items.push(Item::Spot {
            rect: Rect::new(x, y, width, height),
            at: (self.chars, u32::try_from(source).unwrap_or(0)),
        });
        // 复制、查找时是公式的原文。
        self.text.push_str(tex);
        self.chars += tex.chars().count();
        self.end_paragraph();
        Flow {
            height,
            first_baseline: Some(baseline),
        }
    }

    /// 排一个可折叠的块：标题一行前加展开或收起的箭头，展开时在其下排 `blocks`。
    ///
    /// `id` 标识这个块，`open` 是原文中的初始状态；用户点击标题切换过时取相反的状态。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn details(
        &mut self,
        id: u64,
        open: bool,
        summary: &Inline,
        blocks: &[Block],
        x: f32,
        y: f32,
        width: f32,
        env: Env,
        out: &mut Out,
    ) -> Flow {
        let base = self.base;
        if summary
            .links
            .first()
            .is_some_and(|url| url == winc_preview::markdown::properties::PANEL)
        {
            return self.property_panel(
                open,
                summary,
                blocks,
                Rect::new(x, y, width, 0.0),
                env,
                out,
            );
        }
        let para = self.body_para(env);
        let open = open != self.toggled.contains(&id);
        let indent = (base * 1.25).round();
        let flow = self.flow(summary, x + indent, y, width - indent, para, &mut out.items);
        let arrow = if open { "▾" } else { "▸" };
        out.items
            .push(self.marker(arrow, x + indent, y, flow.first_baseline, para.color));
        // 整行都能点；排在标题的链接后面，标题里的链接先认。
        out.items.push(Item::Link {
            rect: Rect::new(x, y, width, flow.height),
            url: format!("{DETAILS}{id:x}").into(),
        });
        if !open {
            return flow;
        }
        let top = y + flow.height + base * 0.5;
        let inner = self.blocks(blocks, x, top, width, env, out);
        Flow {
            height: top + inner.height - y,
            first_baseline: flow.first_baseline,
        }
    }

    /// 返回正文段落的样式：正文字号、不加粗、颜色取自 `env`。
    pub(super) fn body_para(&self, env: Env) -> Para {
        Para {
            size: self.base,
            bold: false,
            color: env.color,
            line: BODY_LINE,
            align: Align::None,
            keep_spaces: false,
        }
    }

    /// 排一个顶层块并返回完整的 [`LaidBlock`]；`raw` 为 `Some` 时对光标所在叶子块显示原文，
    /// 若叶子块未命中常规路径则退回整块原文加渲染预览。
    pub fn layout_block(
        &mut self,
        block: &Block,
        width: f32,
        env: Env,
        raw: Option<&Raw>,
    ) -> LaidBlock {
        let mut out = Out::default();
        self.text.clear();
        self.chars = 0;
        self.memo.take_touched();
        self.raw = raw.cloned();
        self.preedit = false;
        self.cell = None;
        let mut flow = self.block(block, 0.0, 0.0, width, env, &mut out);
        // 光标所在的叶子块在块里没有找到（写在列表里的公式这类没有原文对照的块）：整个
        // 顶层块显示原文。
        if let Some(raw) = self.raw.as_mut().filter(|r| !r.done) {
            raw.leaf = 0..raw.block.len();
            raw.parsed_end = raw.block.len();
            raw.kind = raw.guess();
            out = Out::default();
            self.text.clear();
            self.chars = 0;
            flow = self.raw_with_preview(block, 0.0, 0.0, width, env, &mut out);
        }
        self.raw = None;
        LaidBlock::from_layout(
            flow.height,
            out,
            self.text.as_str().into(),
            self.chars,
            self.memo.take_touched(),
            self.preedit,
        )
    }

    /// 排一个仅包含原文的可编辑块（如全空白文档的占位行），返回完整的 [`LaidBlock`]。
    pub fn layout_raw(&mut self, raw: Raw, width: f32) -> LaidBlock {
        self.raw = Some(raw);
        let mut items = Vec::new();
        self.memo.take_touched();
        let flow = self.raw(0.0, 0.0, width, &mut items);
        self.raw = None;
        LaidBlock::new(
            flow.height,
            items,
            Vec::new(),
            "".into(),
            0,
            self.memo.take_touched(),
        )
    }
}

/// 高项与矮项的分界（逻辑像素，经验值）：高度超过该值的项算「高的」，每次绘制都检查一遍；
/// 矮的项按上边缘二分查找。
const TALL: f32 = 120.0;

/// 一个排好版的顶层块：要画的项，坐标相对块的左上角。
#[derive(Default)]
pub struct LaidBlock {
    /// 块的高度（逻辑像素）。
    pub height: f32,
    /// Height already added for the editable rows below the content.
    editing_blank_height: f32,
    /// 要画的项，按绘制先后排列。
    pub(crate) items: Vec<Item>,
    /// 标题的锚点（小写、空格换成 `-`、去掉标点）和所在高度，`[跳转](#标题)` 用。
    anchors: Vec<(String, f32)>,
    /// 块里能选中的文字连起来：段落、单元格、代码行之间是换行，折行处不加。
    ///
    /// 选中、复制、查找都按它，位置是字符偏移。
    pub(crate) text: Box<str>,
    /// `text` 有多少个字符。
    pub chars: usize,
    /// 按高度找看得见的项用（见 [`LaidBlock::visible`]）。
    ///
    /// `tall` 是高的项（代码块的底色、引用的竖条、表格的竖线、大图）的序号，`short` 是矮的项
    /// 按上边缘排好的（上边缘，序号）。
    tall: Vec<u32>,
    short: Vec<(f32, u32)>,
    /// 排版这一块时使用的排版缓存条目，见 [`Memo::sweep`]。
    pub memo: Box<[u64]>,
    /// 输入法组字是否排在这一块中，见 [`Cx::preedit`]。
    pub(crate) preedit: bool,
    /// 比正文宽、可以横向滚动的表格，按项的先后排列。
    pub(crate) scrolls: Vec<Scroller>,
    /// 显示原文的块中渲染结果的第一项和上边缘（见 [`Out::preview`]）。
    preview: Option<(u32, f32)>,
    /// 要画的项已经释放时留下的内容（见 [`LaidBlock::shed`]）；项都在时为 `None`。
    shed: Option<Box<Shed>>,
    /// 超大单块的原位修补索引（小块与已卸载块为 `None`）。
    pub(crate) patch: Option<Box<crate::patch::PatchIndex>>,
}

/// 释放了要画的项的块留下的内容：重排时判断是否受影响，以及按链接定位。
#[derive(Default)]
struct Shed {
    /// 块里用到的图片。
    images: Box<[PathBuf]>,
    /// 块里各个链接的地址和范围的上边缘、高度（块里的坐标，逻辑像素）。
    links: Box<[(Arc<str>, f32, f32)]>,
}

/// 比正文宽的表格：表格的各项按自然宽度排版，绘制时横向平移、裁剪到正文一栏内。
#[derive(Debug, Clone)]
pub struct Scroller {
    /// 属于该表格的项（[`LaidBlock::items`] 的序号）。
    pub items: std::ops::Range<u32>,
    /// 显示表格的区域（块里的坐标）：正文一栏宽，高度含表格下方的滚动条。
    pub rect: Rect,
    /// 表格的总宽度（逻辑像素）。
    pub content: f32,
}

impl Scroller {
    /// 最大横向滚动距离（逻辑像素）。
    pub fn max(&self) -> f32 {
        (self.content - self.rect.w).max(0.0)
    }
}

impl LaidBlock {
    /// 创建一个排好的顶层块，同时按高度建好查找可见项的索引。
    pub fn new(
        height: f32,
        items: Vec<Item>,
        anchors: Vec<(String, f32)>,
        text: Box<str>,
        chars: usize,
        memo: Box<[u64]>,
    ) -> LaidBlock {
        let (mut tall, mut short) = (Vec::new(), Vec::new());
        for (i, item) in items.iter().enumerate() {
            let (top, bottom) = item.span_y();
            if bottom - top > TALL {
                tall.push(i as u32);
            } else {
                short.push((top, i as u32));
            }
        }
        short.sort_by(|a, b| a.0.total_cmp(&b.0));
        LaidBlock {
            height,
            editing_blank_height: 0.0,
            items,
            anchors,
            text,
            chars,
            tall,
            short,
            memo,
            preedit: false,
            scrolls: Vec::new(),
            preview: None,
            shed: None,
            patch: None,
        }
    }

    /// 由单块排版输出 `out` 直接构造完整的 [`LaidBlock`]，包含预编辑、空行、滚动区与原文分界。
    pub(crate) fn from_layout(
        height: f32,
        out: Out,
        text: Box<str>,
        chars: usize,
        memo: Box<[u64]>,
        preedit: bool,
    ) -> LaidBlock {
        let mut laid = LaidBlock::new(height, out.items, out.anchors, text, chars, memo);
        laid.preedit = preedit;
        laid.editing_blank_height = out.editing_blank_height;
        laid.scrolls = out.scrolls;
        laid.preview = out.preview;
        laid.patch = out.patch;
        laid
    }

    /// 返回释放了要画的项的同一块：高度、锚点、文字不变，另记下用到的图片和链接。
    ///
    /// 很长的文档中离视口远的块以这种形式保留，占用的内存只与文字的长度有关；回到视口附近时
    /// 重新排版。
    pub fn shed(&self) -> LaidBlock {
        let images = self.items.iter().filter_map(|item| match item {
            Item::Image { path, .. } => Some(path.clone()),
            _ => None,
        });
        let links = self.items.iter().filter_map(|item| match item {
            Item::Link { rect, url } => Some((Arc::clone(url), rect.y, rect.h)),
            _ => None,
        });
        LaidBlock {
            height: self.height,
            editing_blank_height: self.editing_blank_height,
            anchors: self.anchors.clone(),
            text: self.text.clone(),
            chars: self.chars,
            shed: Some(Box::new(Shed {
                images: images.collect(),
                links: links.collect(),
            })),
            ..LaidBlock::default()
        }
    }

    /// 要画的项是否已经释放。
    pub fn is_shed(&self) -> bool {
        self.shed.is_some()
    }

    /// 块内的绘制项，按绘制先后排列。
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// 块里能选中的完整文字。
    pub fn text(&self) -> &str {
        &self.text
    }

    /// 把块文字里第 `from` 到 `to` 个字符追加到 `out`。
    pub fn append_text_range(&self, from: usize, to: usize, out: &mut String) {
        let to = to.min(self.chars);
        out.extend(self.text.chars().skip(from).take(to.saturating_sub(from)));
    }

    /// 返回标题锚点 `slug` 在块内的上边缘和高度（逻辑像素）；找不到时返回 `None`。
    pub fn anchor_box(&self, slug: &str) -> Option<(f32, f32)> {
        self.anchors
            .iter()
            .find(|(key, _)| key == slug)
            .map(|(_, y)| (*y, self.height - y))
    }

    /// 块内所有标题锚点及其在块内的上边缘（逻辑像素）。
    pub fn anchors(&self) -> &[(String, f32)] {
        &self.anchors
    }

    /// 块末尾已为可编辑空行计入的高度（逻辑像素）。
    pub fn editing_blank_height(&self) -> f32 {
        self.editing_blank_height
    }

    /// 输入法组字是否已排入本块。
    pub fn has_preedit(&self) -> bool {
        self.preedit
    }

    /// 显示原文的块中渲染结果的第一项序号和上边缘（逻辑像素）。
    pub fn preview_split(&self) -> Option<(u32, f32)> {
        self.preview
    }

    /// 块内所有可横向滚动的表格。
    pub fn scrollers(&self) -> &[Scroller] {
        &self.scrolls
    }

    /// 返回第 `index` 个可横向滚动的表格。
    pub fn scroller(&self, index: usize) -> Option<&Scroller> {
        self.scrolls.get(index)
    }

    /// 返回块内坐标 `(x, y)` 处可横向滚动的表格序号及最大滚动距离。
    pub fn scroller_at(&self, x: f32, y: f32) -> Option<(usize, f32)> {
        self.scrolls
            .iter()
            .position(|s| s.rect.contains(x, y))
            .map(|s| (s, self.scrolls[s].max()))
    }

    /// 返回纵坐标 `y`（块内坐标）落入的可横向滚动表格序号及描述。
    pub fn scroller_at_y(&self, y: f32) -> Option<(usize, &Scroller)> {
        self.scrolls
            .iter()
            .enumerate()
            .find(|(_, s)| (s.rect.y..s.rect.bottom()).contains(&y))
    }

    /// 返回块中使用次要色 `muted` 的非空文字项的序号和内容，供标记淡入过渡比对。
    pub fn muted_text_marks(&self, muted: Color) -> Vec<(usize, Arc<str>)> {
        self.items
            .iter()
            .enumerate()
            .filter_map(|(k, item)| match item {
                Item::Text { text, color, .. } if *color == muted && !text.trim().is_empty() => {
                    Some((k, Arc::clone(text)))
                }
                _ => None,
            })
            .collect()
    }

    /// 返回块里第一个指向 `url` 的链接的上边缘和高度（块里的坐标，逻辑像素）。
    pub fn link_box(&self, url: &str) -> Option<(f32, f32)> {
        let kept = self.shed.iter().flat_map(|shed| shed.links.iter());
        let kept = kept.map(|(u, y, h)| (&**u, *y, *h));
        let drawn = self.items.iter().filter_map(|item| match item {
            Item::Link { rect, url } => Some((&**url, rect.y, rect.h)),
            _ => None,
        });
        let mut links = kept.chain(drawn);
        links.find(|(u, ..)| *u == url).map(|(_, y, h)| (y, h))
    }

    /// 第 `item` 项所在的可横向滚动的表格的序号（[`LaidBlock::scrollers`]）；不在其中时为 `None`。
    pub fn scroller_of(&self, item: usize) -> Option<usize> {
        let item = u32::try_from(item).ok()?;
        self.scrolls.iter().position(|s| s.items.contains(&item))
    }

    /// 块内各表格的绘制项序号、矩形、列宽和行高，按绘制顺序返回。
    pub fn tables(&self) -> impl Iterator<Item = (usize, Rect, &[f32], &[f32])> {
        self.items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| match item {
                Item::Table {
                    rect, cols, rows, ..
                } => Some((i, *rect, &**cols, &**rows)),
                _ => None,
            })
    }

    /// 图片和公式的命中区域及源码坐标，按绘制顺序返回。坐标均相对顶层块。
    pub fn source_spots(&self) -> impl Iterator<Item = (usize, Rect, (usize, u32))> + '_ {
        self.items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| match item {
                Item::Image {
                    rect, at: Some(at), ..
                }
                | Item::Spot { rect, at } => Some((i, *rect, *at)),
                _ => None,
            })
    }

    /// 返回第 `item` 项的图片在原文中的字节偏移，相对顶层块起点。
    pub fn image_source(&self, item: usize) -> Option<u32> {
        match self.items.get(item)? {
            Item::Image {
                at: Some((_, source)),
                ..
            } => Some(*source),
            _ => None,
        }
    }

    /// 返回源码位置所在文字段的字体、模拟字形和字号，用于绘制输入法组字。
    /// `source` 是相对顶层块的字节偏移，取起点不超过它的最后一段。
    pub fn font_at_source(&self, source: usize) -> Option<(FontKind, Faux, f32)> {
        self.items
            .iter()
            .filter_map(|item| match item {
                Item::Text {
                    font,
                    faux,
                    size,
                    span: Some(span),
                    ..
                } => span
                    .source
                    .filter(|&at| at as usize <= source)
                    .map(|at| (at, (*font, *faux, *size))),
                _ => None,
            })
            .max_by_key(|(at, _)| *at)
            .map(|(_, font)| font)
    }

    /// 块里有没有用到 `paths` 里的图片（排版时还在读的也算）。
    pub fn uses_any(&self, paths: &[PathBuf]) -> bool {
        let kept = self.shed.iter().flat_map(|shed| shed.images.iter());
        let drawn = self.items.iter().filter_map(|item| match item {
            Item::Image { path, .. } => Some(path),
            _ => None,
        });
        !paths.is_empty() && kept.chain(drawn).any(|path| paths.contains(path))
    }

    /// 块里有没有指向 `url` 的链接。
    pub fn links_to(&self, url: &str) -> bool {
        self.link_box(url).is_some()
    }

    /// 原位更新可见项空间索引（`tall` 与 `short`），供超大单块局部修补使用。
    pub(crate) fn patch_spatial_index(
        &mut self,
        old_items: std::ops::Range<usize>,
        new_len: usize,
        dy: f32,
    ) {
        let di = new_len as isize - old_items.len() as isize;
        let new_end = old_items.start + new_len;

        self.tall.retain_mut(|idx| {
            let i = *idx as usize;
            if old_items.contains(&i) {
                false
            } else {
                if i >= old_items.end {
                    *idx = (i as isize + di) as u32;
                }
                true
            }
        });

        let mut sorted = true;
        let mut prev_top = f32::NEG_INFINITY;
        self.short.retain_mut(|(top, idx)| {
            let i = *idx as usize;
            if old_items.contains(&i) {
                return false;
            }
            if i >= old_items.end {
                let shifted = (i as isize + di) as u32;
                *idx = shifted;
                if dy != 0.0 {
                    *top = self.items[shifted as usize].span_y().0;
                }
            }
            if *top < prev_top {
                sorted = false;
            }
            prev_top = *top;
            true
        });
        if !sorted {
            self.short.sort_by(|a, b| a.0.total_cmp(&b.0));
        }

        for i in old_items.start..new_end {
            let (top, bottom) = self.items[i].span_y();
            if bottom - top > TALL {
                self.tall.push(i as u32);
            } else {
                let pos = self.short.partition_point(|&(t, _)| t <= top);
                self.short.insert(pos, (top, i as u32));
            }
        }
    }

    /// 若存在超大块修补索引，按原文偏移（相对顶层块）返回可能命中的局部项窗口，避免全块扫描。
    pub fn source_item_window(&self, source: usize) -> Option<std::ops::Range<usize>> {
        self.patch.as_ref()?.source_item_window(source)
    }

    /// 和 `y0..y1`（块里的坐标）有交集的项的序号，按原来的先后（先画的底色在下面）。
    ///
    /// 长列表、长表格、长代码块是一个顶层块，里面成千上万项，每帧只看屏上那些。
    pub fn visible(&self, y0: f32, y1: f32, out: &mut Vec<u32>) {
        out.clear();
        let hit = |i: u32| {
            let (top, bottom) = self.items[i as usize].span_y();
            bottom >= y0 && top <= y1
        };
        out.extend(self.tall.iter().copied().filter(|&i| hit(i)));
        let from = self.short.partition_point(|&(top, _)| top < y0 - TALL);
        let to = self.short.partition_point(|&(top, _)| top <= y1);
        out.extend(
            self.short[from..to.max(from)]
                .iter()
                .map(|&(_, i)| i)
                .filter(|&i| hit(i)),
        );
        out.sort_unstable();
    }
}
