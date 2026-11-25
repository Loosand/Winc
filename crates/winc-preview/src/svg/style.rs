//! 样式：属性的继承、`style` 和 CSS 里的声明、元素自己的属性。

use super::canvas::blend_mode;
use super::*;

/// 字体相关的、可继承的样式。
#[derive(Debug, Clone)]
pub(super) struct Font {
    /// 按优先级排列的字族名。
    pub(super) families: Arc<[String]>,
    /// 字号（用户坐标单位）。
    pub(super) size: f32,
    /// 字重，1 到 1000，400 为正常粗细。
    pub(super) weight: u16,
    /// 是否使用斜体。
    pub(super) italic: bool,
    /// `text-anchor`：整段文字相对给定 `x` 的对齐方式。
    pub(super) anchor: Anchor,
    /// 字符之间的附加间距（用户坐标单位）。
    pub(super) letter_spacing: f32,
    /// 空格之后的附加间距（用户坐标单位）。
    pub(super) word_spacing: f32,
    /// `dominant-baseline`：基线相对给定 `y` 的位置。
    pub(super) baseline: Baseline,
}

impl Default for Font {
    fn default() -> Self {
        Font {
            // 未指定 `font-family` 时使用通用字族 `serif`。
            families: Arc::from(["serif".to_owned()]),
            size: 16.0,
            weight: 400,
            italic: false,
            anchor: Anchor::Start,
            letter_spacing: 0.0,
            word_spacing: 0.0,
            baseline: Baseline::Alphabetic,
        }
    }
}

/// 整段文字相对给定 `x` 的对齐方式，取自 `text-anchor`。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Anchor {
    Start,
    Middle,
    End,
}

/// 基线相对给定 `y` 的位置，取自 `dominant-baseline`，已映射为本模块支持的对齐方式。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Baseline {
    Alphabetic,
    Middle,
    Central,
    Hanging,
    TextTop,
    TextBottom,
}

/// 解析字号，返回用户坐标单位下的值；无法解析时返回 `None`。
///
/// 支持关键字、数、带单位的长度、百分比和 `em`（相对上一级字号 `parent`）。
fn font_size(value: &str, parent: f32) -> Option<f32> {
    // 字号关键字换算成 px：`medium` 取 16 px，其余关键字的 px 值是经验值，不是规范规定的数值。
    // `larger`、`smaller` 为上一级字号的 1.2 倍及其倒数。
    let keyword = match value {
        "xx-small" => Some(9.0),
        "x-small" => Some(10.0),
        "small" => Some(13.0),
        "medium" => Some(16.0),
        "large" => Some(18.0),
        "x-large" => Some(24.0),
        "xx-large" => Some(32.0),
        "larger" => Some(parent * 1.2),
        "smaller" => Some(parent / 1.2),
        _ => None,
    };
    if keyword.is_some() {
        return keyword;
    }
    // 以 `rem` 结尾的值不按 `em` 处理，转交给 `length`。
    if let Some(n) = value.strip_suffix("em").filter(|n| !n.ends_with('r')) {
        return n.trim().parse::<f32>().ok().map(|n| n * parent);
    }
    if let Some(n) = value.strip_suffix('%') {
        return n.trim().parse::<f32>().ok().map(|n| n / 100.0 * parent);
    }
    length(value, parent).filter(|s| *s >= 0.0)
}

/// 解析 `font-weight`，返回 1 到 1000 的数值；无法解析时返回 `None`。
///
/// `bolder`、`lighter` 相对 `parent` 加、减 300，结果分别限制在不超过 900、不低于 100。
fn font_weight(value: &str, parent: u16) -> Option<u16> {
    Some(match value {
        "normal" => 400,
        "bold" => 700,
        "bolder" => (parent + 300).min(900),
        "lighter" => parent.saturating_sub(300).max(100),
        n => n.parse::<f32>().ok()?.clamp(1.0, 1000.0) as u16,
    })
}

/// 解析逗号分隔的字族列表，去掉每个字族名两侧的引号和空白，忽略空项。
fn families(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|f| f.trim().trim_matches(['"', '\'']).trim().to_owned())
        .filter(|f| !f.is_empty())
        .collect()
}

/// 把样式属性 `name: value` 应用到 `font`；非字体属性被忽略。
///
/// `Parser::apply` 在其他属性都不匹配时转到这里。`parent` 是上一级的字体样式，`diagonal`
/// 是百分比长度的参照值（视口的归一化对角线，用户坐标单位）。
pub(super) fn apply_font(font: &mut Font, parent: &Font, name: &str, value: &str, diagonal: f32) {
    match name {
        "font-family" => {
            let list = families(value);
            if !list.is_empty() {
                font.families = list.into();
            }
        }
        "font-size" => {
            if let Some(size) = font_size(value, parent.size) {
                font.size = size;
            }
        }
        "font-weight" => {
            if let Some(weight) = font_weight(value, parent.weight) {
                font.weight = weight;
            }
        }
        "font-style" => font.italic = matches!(value, "italic" | "oblique"),
        "font" => shorthand(font, parent, value),
        "text-anchor" => {
            font.anchor = match value {
                "middle" => Anchor::Middle,
                "end" => Anchor::End,
                _ => Anchor::Start,
            }
        }
        "letter-spacing" => {
            font.letter_spacing = if value == "normal" {
                0.0
            } else {
                font_size(value, font.size)
                    .or_else(|| length(value, diagonal))
                    .unwrap_or(0.0)
            }
        }
        "word-spacing" => {
            font.word_spacing = if value == "normal" {
                0.0
            } else {
                length(value, diagonal).unwrap_or(0.0)
            }
        }
        "dominant-baseline" | "alignment-baseline" => {
            font.baseline = match value {
                "middle" => Baseline::Middle,
                "central" => Baseline::Central,
                "hanging" => Baseline::Hanging,
                "text-before-edge" | "text-top" | "before-edge" => Baseline::TextTop,
                "text-after-edge" | "text-bottom" | "after-edge" | "ideographic" => {
                    Baseline::TextBottom
                }
                _ => Baseline::Alphabetic,
            }
        }
        _ => {}
    }
}

/// 应用 `font` 简写属性，如 `font: italic bold 12px/1.5 Helvetica, sans-serif`。
fn shorthand(font: &mut Font, parent: &Font, value: &str) {
    let mut words = value.split_whitespace().peekable();
    while let Some(word) = words.peek().copied() {
        match word {
            "italic" | "oblique" => font.italic = true,
            "normal" | "small-caps" => {}
            w if font_weight(w, parent.weight).is_some() && !w.ends_with("px") => {
                font.weight = font_weight(w, parent.weight).unwrap_or(400);
            }
            _ => break,
        }
        words.next();
    }
    let Some(size) = words.next() else {
        return;
    };
    let size = size.split('/').next().unwrap_or(size);
    if let Some(size) = font_size(size, parent.size) {
        font.size = size;
    }
    let rest: Vec<&str> = words.collect();
    let list = families(&rest.join(" "));
    if !list.is_empty() {
        font.families = list.into();
    }
}

impl<'a, 'input> Parser<'a, 'input> {
    /// 计算元素 `node` 的样式。
    ///
    /// 先继承父元素 `parent` 的样式，再依次应用属性、`<style>` 里的规则和 `style` 属性，
    /// 后两者覆盖前面的同名声明。
    pub(super) fn props(&mut self, node: Node<'a, 'input>, parent: &Props) -> Props {
        let declared = self.declared(node);
        self.props_from(&declared, parent)
    }

    /// 与 [`Parser::props`] 相同，但声明 `declared` 已经取好。
    ///
    /// [`Parser::declared`] 需要匹配 CSS 规则，同一个元素的继承样式和自有属性共用同一份声明。
    pub(super) fn props_from(&mut self, declared: &[Declaration], parent: &Props) -> Props {
        let mut props = parent.clone();
        for (name, value) in declared {
            self.apply(&mut props, parent, name, value);
        }
        props
    }

    /// 返回 `node` 从根元素一路继承下来的样式。
    ///
    /// 裁剪、蒙版、滤镜的内容使用内容所在位置的样式，不使用引用这些内容的元素的样式。
    pub(super) fn inherited(&mut self, node: Node<'a, 'input>) -> Props {
        let mut chain: Vec<Node> = node.ancestors().filter(Node::is_element).collect();
        chain.reverse();
        let mut props = Props::default();
        for n in chain {
            props = self.props(n, &props);
        }
        props
    }

    /// 返回一个元素上写的所有样式声明，按优先级从低到高排列：后面的覆盖前面的。
    ///
    /// 每访问一次元素都要取一遍（`<use>` 每展开一次、渐变每被引用一次也是），而 `d`、
    /// 内嵌图片的 `href` 这类属性值可以达到数 MB，因此只引用原文，不复制。
    pub(super) fn declared(&self, node: Node<'a, 'input>) -> Vec<Declaration<'a>> {
        let mut out: Vec<Declaration<'a>> = node
            .attributes()
            .filter(|a| a.namespace().is_none())
            .map(|a| (Cow::Borrowed(a.name()), a.value()))
            .collect();
        out.extend(
            self.sheet
                .matching(node)
                .map(|(name, value)| (Cow::Borrowed(name), value)),
        );
        if let Some(style) = node.attribute("style") {
            out.extend(css::declarations(style));
        }
        out
    }

    /// 把一条声明 `name: value` 应用到 `props`；字体相关的属性交给 `text::apply_font` 处理。
    fn apply(&mut self, props: &mut Props, parent: &Props, name: &str, value: &str) {
        let value = value.trim();
        if value == "inherit" {
            return;
        }
        let diagonal = self.diagonal();
        match name {
            // 无法解析的声明不生效，保留优先级较低的声明给出的值。
            "fill" => props.fill = paint(value).unwrap_or_else(|| props.fill.clone()),
            "stroke" => props.stroke = paint(value).unwrap_or_else(|| props.stroke.clone()),
            "fill-opacity" => props.fill_opacity = opacity(value),
            "stroke-opacity" => props.stroke_opacity = opacity(value),
            "fill-rule" | "clip-rule" => {
                let rule = if value == "evenodd" {
                    Fill::EvenOdd
                } else {
                    Fill::NonZero
                };
                if name == "fill-rule" {
                    props.fill_rule = rule;
                } else {
                    props.clip_rule = rule;
                }
            }
            "stroke-width" => {
                if let Some(w) = length(value, diagonal).filter(|w| *w >= 0.0) {
                    props.stroke_width = w;
                }
            }
            "stroke-linecap" => {
                props.cap = match value {
                    "round" => Cap::Round,
                    "square" => Cap::Square,
                    _ => Cap::Butt,
                }
            }
            "stroke-linejoin" => {
                props.join = match value {
                    "round" => Join::Round,
                    "bevel" => Join::Bevel,
                    _ => Join::Miter,
                }
            }
            "stroke-miterlimit" => {
                if let Ok(m) = value.parse::<f32>() {
                    props.miter = m.max(1.0);
                }
            }
            "stroke-dasharray" => {
                let mut dashes: Vec<f32> = value
                    .split([',', ' '])
                    .filter(|s| !s.is_empty())
                    .filter_map(|s| length(s, diagonal))
                    .collect();
                if dashes.iter().any(|d| *d < 0.0) || dashes.iter().sum::<f32>() <= 0.0 {
                    dashes.clear();
                } else if dashes.len() % 2 == 1 {
                    dashes.extend_from_within(..);
                }
                props.dashes = dashes.into();
            }
            "stroke-dashoffset" => props.dash_offset = length(value, diagonal).unwrap_or(0.0),
            "color" => {
                if let Some(c) = color(value) {
                    props.color = c;
                }
            }
            "visibility" => props.visible = value == "visible",
            "paint-order" => {
                let words: Vec<&str> = value.split_whitespace().collect();
                let at = |w: &str| words.iter().position(|x| *x == w);
                props.stroke_first = match (at("stroke"), at("fill")) {
                    (Some(s), Some(f)) => s < f,
                    (Some(_), None) => true,
                    _ => false,
                };
            }
            "color-interpolation-filters" => props.filters_linear = value != "sRGB",
            "image-rendering" => {
                props.pixelated = matches!(value, "pixelated" | "crisp-edges" | "optimizeSpeed")
            }
            "marker-start" | "marker-mid" | "marker-end" | "marker" if value != "none" => {
                self.miss(winc_i18n::t!("preview-svg-missing-markers"))
            }
            "animation" | "animation-name" if value != "none" => {
                self.miss(winc_i18n::t!("preview-svg-missing-animation"))
            }
            _ => apply_font(&mut props.font, &parent.font, name, value, diagonal),
        }
    }

    /// 返回当前元素自己的（不继承的）属性。
    pub(super) fn own(&self, node: Node<'a, 'input>) -> Own {
        Self::own_from(&self.declared(node))
    }

    /// 与 [`Parser::own`] 相同，但声明 `declared` 已经取好。
    pub(super) fn own_from(declared: &[Declaration]) -> Own {
        let mut own = Own {
            opacity: 1.0,
            shown: true,
            transform: Affine::IDENTITY,
            clip_path: None,
            mask: None,
            filter: None,
            overflow_visible: false,
            non_scaling: false,
            mask_alpha: false,
            blend: Blend::Normal,
        };
        let url = |v: &str| {
            let v = v.trim();
            let inside = v.strip_prefix("url(")?.split_once(')')?.0;
            let id = inside.trim().trim_matches(['"', '\'']);
            Some(id.strip_prefix('#').unwrap_or(id).to_owned())
        };
        for &(ref name, value) in declared {
            match name.as_ref() {
                "opacity" => own.opacity = opacity(value),
                "display" => own.shown = value.trim() != "none",
                "transform" => own.transform = transform(value),
                "clip-path" => own.clip_path = url(value),
                "mask" => own.mask = url(value),
                "filter" => {
                    own.filter = match value.trim() {
                        "none" | "" => None,
                        v if v.starts_with("url(") => url(v).map(FilterRef::Url),
                        v => Some(FilterRef::Functions(v.to_owned())),
                    }
                }
                "overflow" => own.overflow_visible = matches!(value.trim(), "visible" | "auto"),
                "vector-effect" => own.non_scaling = value.trim() == "non-scaling-stroke",
                "mask-type" => own.mask_alpha = value.trim() == "alpha",
                "mix-blend-mode" => own.blend = blend_mode(value),
                _ => {}
            }
        }
        own
    }
}
