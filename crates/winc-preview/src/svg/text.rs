//! `<text>` 的排版。
//!
//! 字形轮廓由调用方通过 [`Glyphs`] 提供，本模块按 SVG 的规则排版：`x`、`y`、`dx`、`dy`（每个
//! 字符可以有各自的值）、`<tspan>`、空白合并、`text-anchor`、`dominant-baseline`、字间距和词间距。
//! 排好的字形转换为普通的形状，填充、描边、渐变和裁剪的处理与其他形状相同。不支持竖排、
//! `textPath`、`rotate`、`textLength` 和 `text-decoration`。

use std::sync::Arc;

use roxmltree::Node;
use zeno::{Command, Fill};

use super::style::{Anchor, Baseline};
use super::syntax::*;
use super::{Item, Own, Parser, Props, Shape};

mod foreign;

/// 一个字形。
///
/// 轮廓和前进宽度均以 em 为单位；原点在基线左端，y 轴向下。
#[derive(Debug, Clone)]
pub struct Glyph {
    /// 字形轮廓（em 为单位）。
    ///
    /// 由各字符共用：字形来源可以把缓存里的轮廓直接交出来，不必每个字符复制一遍。
    pub outline: Arc<[Command]>,
    /// 前进宽度（em）。
    pub advance: f32,
}

/// 字体的竖向度量（em，向上为正），用于计算 `dominant-baseline` 的基线位移。
#[derive(Debug, Clone, Copy)]
pub struct FontMetrics {
    /// 基线以上的高度（em）。
    pub ascent: f32,
    /// 基线以下的高度（em）。
    pub descent: f32,
    /// 小写 `x` 的高度（em）。
    pub x_height: f32,
}

/// 按字族、字重和斜体提供字形的来源。
///
/// `families` 按顺序尝试；都没有目标字符时使用系统的后备字体，仍然没有则返回 `None`，该字符不绘制。
/// 通用字族名（`sans-serif`、`serif`、`monospace` 等）由实现方映射为系统字体。
pub trait Glyphs: Send + Sync {
    /// 返回字符 `c` 在给定字族、字重和斜体下的字形；找不到时返回 `None`，该字符跳过不画。
    fn glyph(&self, families: &[String], weight: u16, italic: bool, c: char) -> Option<Glyph>;
    /// 返回给定字族、字重和斜体下的竖向度量。
    fn metrics(&self, families: &[String], weight: u16, italic: bool) -> FontMetrics;
}

/// 排版时的一个字符及其位置信息。
struct Char {
    c: char,
    /// 所用样式在 `styles` 中的下标。
    style: usize,
    /// 显式给出的绝对 x 坐标（用户坐标单位）；`None` 表示接在上一个字符之后。
    x: Option<f32>,
    /// 显式给出的绝对 y 坐标（用户坐标单位）；`None` 表示沿用当前排版位置。
    y: Option<f32>,
    /// x 方向的相对偏移（用户坐标单位）。
    dx: f32,
    /// y 方向的相对偏移（用户坐标单位）。
    dy: f32,
}

/// 一个元素上的 `x`、`y`、`dx`、`dy` 列表（依此顺序），以及已经消耗的字符数。
struct Positions {
    lists: [Vec<f32>; 4],
    used: usize,
}

impl<'a, 'input> Parser<'a, 'input> {
    /// 排版 `<text>` 元素，把字形轮廓按样式分组后作为形状追加到 `out`。
    ///
    /// 没有字形来源时记为不支持的特性并直接返回。`(props, own)` 是元素的样式，`ctm` 是当前变换，
    /// `depth` 是元素嵌套深度。
    pub(super) fn text(
        &mut self,
        node: Node<'a, 'input>,
        (props, own): (&Props, &Own),
        ctm: Affine,
        out: &mut Vec<Item>,
        depth: usize,
    ) {
        let Some(glyphs) = self.glyphs.clone() else {
            self.miss(winc_i18n::t!("preview-svg-missing-text"));
            return;
        };
        let mut chars = Vec::new();
        let mut styles = vec![(props.clone(), own.clone())];
        let mut stack = Vec::new();
        let mut last_space = true;
        self.collect(
            node,
            0,
            &mut chars,
            &mut styles,
            &mut stack,
            &mut last_space,
            depth,
        );
        // 去掉末尾的空白（`xml:space="preserve"` 时保留）。
        while chars.last().is_some_and(|c| c.c == ' ' && !preserve(node)) {
            chars.pop();
        }
        self.place(glyphs.as_ref(), &chars, &styles, ctm, out);
    }

    /// 按 `chars` 中各字符的位置和 `styles` 中的样式排版，把字形轮廓按样式分组后追加到 `out`。
    fn place(
        &mut self,
        glyphs: &dyn Glyphs,
        chars: &[Char],
        styles: &[(Props, Own)],
        ctm: Affine,
        out: &mut Vec<Item>,
    ) {
        if chars.is_empty() {
            return;
        }

        // 逐字符排版：带有 `x` 或 `y` 的字符开始一个新的文本块（`text-anchor` 按块对齐）。
        let mut placed: Vec<(usize, Vec<Command>)> = Vec::new();
        let mut chunks: Vec<(usize, usize, Anchor)> = Vec::new();
        let (mut pen_x, mut pen_y) = (0.0f32, 0.0f32);
        // 每个字符排完后的结束位置（x，用户坐标单位），块对齐时用来计算块宽度。
        let mut ends: Vec<f32> = Vec::with_capacity(chars.len());
        for (i, ch) in chars.iter().enumerate() {
            let font = &styles[ch.style].0.font;
            if ch.x.is_some() || ch.y.is_some() || i == 0 {
                if let Some(last) = chunks.last_mut() {
                    last.1 = i;
                }
                chunks.push((i, chars.len(), font.anchor));
            }
            if let Some(x) = ch.x {
                pen_x = x;
            }
            if let Some(y) = ch.y {
                pen_y = y;
            }
            pen_x += ch.dx;
            pen_y += ch.dy;
            // 默认的 `alphabetic` 基线不需要字体的竖向度量：跳过查询，避免每个字符都查一次字体。
            let shift = if font.baseline == Baseline::Alphabetic {
                0.0
            } else {
                let metrics = glyphs.metrics(&font.families, font.weight, font.italic);
                let em = match font.baseline {
                    Baseline::Alphabetic => 0.0,
                    Baseline::Middle => metrics.x_height / 2.0,
                    Baseline::Central => (metrics.ascent - metrics.descent) / 2.0,
                    Baseline::Hanging => metrics.ascent * 0.8,
                    Baseline::TextTop => metrics.ascent,
                    Baseline::TextBottom => -metrics.descent,
                };
                em * font.size
            };
            let glyph = glyphs.glyph(&font.families, font.weight, font.italic, ch.c);
            let advance = glyph
                .as_ref()
                .map_or(font.size * 0.5, |g| g.advance * font.size);
            if let Some(glyph) = glyph {
                let (ox, oy, s) = (pen_x, pen_y + shift, font.size);
                let outline = map_points(&glyph.outline, |p| {
                    zeno::Point::new(ox + p.x * s, oy + p.y * s)
                });
                placed.push((i, outline));
            }
            pen_x +=
                advance + font.letter_spacing + if ch.c == ' ' { font.word_spacing } else { 0.0 };
            ends.push(pen_x);
        }

        // 块对齐：按块宽度把整块向左移动。`start_x(i)` 是第 i 个字符的起点：自己的 `x`，
        // 没有时取上一个字符排完的位置，再加 `dx`。
        let start_x =
            |i: usize| chars[i].x.unwrap_or(if i == 0 { 0.0 } else { ends[i - 1] }) + chars[i].dx;
        let mut offset = vec![0.0f32; chars.len()];
        for &(from, to, anchor) in &chunks {
            if anchor == Anchor::Start || from >= to {
                continue;
            }
            // 块宽度不含末尾字符之后的字间距。
            let last_font = &styles[chars[to - 1].style].0.font;
            let width = ends[to - 1] - last_font.letter_spacing - start_x(from);
            let shift = if anchor == Anchor::Middle {
                width / 2.0
            } else {
                width
            };
            for o in &mut offset[from..to] {
                *o = -shift;
            }
        }

        // 样式相同且相邻的字符合并为一个形状。
        let mut groups: Vec<(usize, Vec<Command>)> = Vec::new();
        for (i, mut outline) in placed {
            if offset[i] != 0.0 {
                let dx = offset[i];
                map_points_in_place(&mut outline, |p| zeno::Point::new(p.x + dx, p.y));
            }
            let style = chars[i].style;
            match groups.last_mut() {
                Some((s, path)) if *s == style => path.extend(outline),
                _ => groups.push((style, outline)),
            }
        }
        // 渐变使用的外框 `bounds` 按整段文字计算：各组外框的并集，没有轮廓的组
        // （例如只有空格）不计入。
        let groups: Vec<(usize, Vec<Command>, [f32; 4])> = groups
            .into_iter()
            .map(|(style, path)| {
                let b = super::path_bounds(&path);
                (style, path, b)
            })
            .collect();
        let Some(bounds) = groups
            .iter()
            .filter(|g| !g.1.is_empty())
            .map(|g| g.2)
            .reduce(|[ax, ay, aw, ah], [bx, by, bw, bh]| {
                let (x0, y0) = (ax.min(bx), ay.min(by));
                let (x1, y1) = ((ax + aw).max(bx + bw), (ay + ah).max(by + bh));
                [x0, y0, x1 - x0, y1 - y0]
            })
        else {
            return;
        };
        for (style, path, path_bounds) in groups {
            let (props, own) = &styles[style];
            if !props.visible {
                continue;
            }
            let fill = self
                .paint(&props.fill, props.fill_opacity, props, bounds)
                .map(|p| (p, Fill::NonZero));
            let stroke = self.stroke(props, own, bounds);
            if fill.is_none() && stroke.is_none() {
                continue;
            }
            self.shapes += 1;
            out.push(Item::Shape(Shape {
                bounds: path_bounds,
                path,
                transform: ctm,
                fill,
                stroke,
                stroke_first: props.stroke_first,
            }));
        }
    }

    /// 收集 `node` 内的字符，文本节点按空白规则合并空白；`<tspan>` 和 `<a>` 使用自己的样式和
    /// 位置列表。
    #[allow(clippy::too_many_arguments)]
    fn collect(
        &mut self,
        node: Node<'a, 'input>,
        style: usize,
        chars: &mut Vec<Char>,
        styles: &mut Vec<(Props, Own)>,
        stack: &mut Vec<Positions>,
        last_space: &mut bool,
        depth: usize,
    ) {
        if depth > super::MAX_DEPTH {
            return;
        }
        let (vw, vh) = self.viewport;
        let list = |name: &str, reference: f32| -> Vec<f32> {
            node.attribute(name)
                .map(|v| {
                    v.split([',', ' ', '\t', '\n'])
                        .filter(|s| !s.is_empty())
                        .filter_map(|s| length(s, reference))
                        .collect()
                })
                .unwrap_or_default()
        };
        stack.push(Positions {
            lists: [list("x", vw), list("y", vh), list("dx", vw), list("dy", vh)],
            used: 0,
        });
        let preserve = preserve(node);
        for child in node.children() {
            if let Some(text) = child.text().filter(|_| child.is_text()) {
                // 默认的空白规则：删除换行符，制表符视为空格，连续空格合并为一个；
                // `xml:space="preserve"` 时换行符和制表符都转为空格，且不合并。
                for c in text.chars() {
                    let c = match c {
                        '\n' | '\r' if !preserve => continue,
                        '\t' | '\n' | '\r' => ' ',
                        c => c,
                    };
                    if c == ' ' && *last_space && !preserve {
                        continue;
                    }
                    *last_space = c == ' ';
                    // `x`、`y`、`dx`、`dy` 各取自最内层、且列表中还有未使用值的元素。
                    let mut picked = [None; 4];
                    for (k, slot) in picked.iter_mut().enumerate() {
                        *slot = stack
                            .iter()
                            .rev()
                            .find_map(|p| p.lists[k].get(p.used).copied());
                    }
                    for p in stack.iter_mut() {
                        p.used += 1;
                    }
                    chars.push(Char {
                        c,
                        style,
                        x: picked[0],
                        y: picked[1],
                        dx: picked[2].unwrap_or(0.0),
                        dy: picked[3].unwrap_or(0.0),
                    });
                }
            } else if child.is_element() {
                match child.tag_name().name() {
                    "tspan" | "a" => {
                        let declared = self.declared(child);
                        let own = Self::own_from(&declared);
                        if !own.shown {
                            continue;
                        }
                        let parent = styles[style].0.clone();
                        let props = self.props_from(&declared, &parent);
                        styles.push((props, own));
                        let index = styles.len() - 1;
                        self.collect(child, index, chars, styles, stack, last_space, depth + 1);
                    }
                    "textPath" => self.miss(winc_i18n::t!("preview-svg-missing-text-path")),
                    _ => {}
                }
            }
        }
        stack.pop();
    }
}

/// `node` 或其祖先是否带有 `xml:space="preserve"`。
fn preserve(node: Node) -> bool {
    node.ancestors()
        .find_map(|n| n.attribute(("http://www.w3.org/XML/1998/namespace", "space")))
        == Some("preserve")
}
