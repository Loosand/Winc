//! 声明式节点与链式布局/样式构建器。

use std::borrow::Cow;

use taffy::geometry::{Rect as TaffyRect, Size};
use taffy::style::{
    AlignItems, Dimension, Display, FlexDirection, JustifyContent, LengthPercentage,
    LengthPercentageAuto, Overflow, Style,
};
use winc_render::{Color, Faux, FontKind, Frame, Rect};

/// 文本在自身布局矩形内的水平对齐方式。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TextAlign {
    /// 左对齐。
    #[default]
    Start,
    /// 水平居中。
    Center,
    /// 右对齐。
    End,
}

/// 浮动卡片外框参数：柔和阴影与 1 像素细边框。
#[derive(Debug, Clone, Copy)]
pub struct CardFrame {
    /// 卡片阴影色。
    pub shadow: Color,
    /// 边框下方的不透明底色，防止细边框透出卡片下方内容。
    pub border_base: Color,
    /// 1 像素细边框色。
    pub border: Color,
}

/// 文本叶节点配置。
pub(crate) struct TextSpec<'a> {
    pub(crate) content: Cow<'a, str>,
    pub(crate) font: FontKind,
    pub(crate) size: f32,
    pub(crate) line_height: Option<f32>,
    pub(crate) color: Color,
    pub(crate) wrap: bool,
    pub(crate) align: TextAlign,
    pub(crate) faux: Faux,
}

/// 自定义测量回调：参数依次为 `(已知宽度, 可用宽度, Frame)`，返回 `(宽, 高)`（逻辑像素）。
pub type MeasureFn<'a> =
    Box<dyn FnMut(Option<f32>, Option<f32>, &mut Frame<'_>) -> (f32, f32) + 'a>;

/// 自定义绘制回调：参数依次为 `(Frame, 节点绝对矩形, 当前裁剪框)`。
pub type PaintFn<'a> = Box<dyn FnMut(&mut Frame<'_>, Rect, Rect) + 'a>;

/// 节点的具体内容种类。
pub(crate) enum NodeKind<'a> {
    /// 容器节点（可包含子节点、底色、圆角、边框与裁剪）。
    Container,
    /// 文本叶节点，布局时按 `Frame::measure` 与折行规则自动计算宽高。
    Text(TextSpec<'a>),
    /// 自定义叶节点：用于嵌入已有控件（图标、输入框、开关或自绘区域）。
    Custom {
        measure: MeasureFn<'a>,
        paint: PaintFn<'a>,
    },
}

/// 节点外观样式。
#[derive(Default, Clone, Copy)]
pub(crate) struct PaintStyle {
    pub(crate) bg: Option<Color>,
    pub(crate) radius: f32,
    pub(crate) border: Option<(f32, Color)>,
    pub(crate) card: Option<CardFrame>,
    pub(crate) clip: bool,
    pub(crate) scroll_y: f32,
}

/// 一棵声明式界面树的节点；泛型 `A` 是点击/悬停命中后返回的动作标识类型。
pub struct Node<'a, A> {
    pub(crate) style: Style,
    pub(crate) paint: PaintStyle,
    pub(crate) kind: NodeKind<'a>,
    pub(crate) children: Vec<Node<'a, A>>,
    pub(crate) hit: Option<A>,
}

/// 创建纵向弹性容器（`flex-direction: column`）。
pub fn vstack<'a, A>() -> Node<'a, A> {
    Node::container(FlexDirection::Column)
}

/// 创建横向弹性容器（`flex-direction: row`）。
pub fn hstack<'a, A>() -> Node<'a, A> {
    Node::container(FlexDirection::Row)
}

/// 创建反向横向弹性容器（`flex-direction: row-reverse`：第一个子节点排在最右边，其余向左）。
pub fn hstack_rev<'a, A>() -> Node<'a, A> {
    Node::container(FlexDirection::RowReverse)
}

/// 创建通用空容器节点。
pub fn box_<'a, A>() -> Node<'a, A> {
    Node::container(FlexDirection::Column)
}

/// 创建自动填充剩余空间的弹性占位节点（`flex-grow: 1`）。
pub fn spacer<'a, A>() -> Node<'a, A> {
    box_().flex_1()
}

/// 创建 1 逻辑像素高的横向分割线（宽度占满父容器）。
pub fn divider<'a, A>(color: Color) -> Node<'a, A> {
    box_().w_full().h(1.0).flex_shrink(0.0).bg(color)
}

/// 创建 1 逻辑像素宽的纵向分割线（高度占满父容器）。
pub fn vdivider<'a, A>(color: Color) -> Node<'a, A> {
    box_().w(1.0).h_full().flex_shrink(0.0).bg(color)
}

/// 创建文本叶节点；默认使用界面字体、13 逻辑像素、白色不透明，调用链式方法配置实际字号与颜色。
pub fn text<'a, A>(content: impl Into<Cow<'a, str>>) -> Node<'a, A> {
    Node {
        style: Style {
            flex_shrink: 1.0,
            ..Style::default()
        },
        paint: PaintStyle::default(),
        kind: NodeKind::Text(TextSpec {
            content: content.into(),
            font: FontKind::Ui,
            size: 13.0,
            line_height: None,
            color: Color::hex(0xffffff),
            wrap: false,
            align: TextAlign::Start,
            faux: Faux::default(),
        }),
        children: Vec::new(),
        hit: None,
    }
}

/// 创建固定尺寸的自定义绘制叶节点。
pub fn custom<'a, A>(
    (width, height): (f32, f32),
    paint: impl FnMut(&mut Frame<'_>, Rect, Rect) + 'a,
) -> Node<'a, A> {
    Node {
        style: Style {
            size: Size {
                width: Dimension::length(width),
                height: Dimension::length(height),
            },
            flex_shrink: 0.0,
            ..Style::default()
        },
        paint: PaintStyle::default(),
        kind: NodeKind::Custom {
            measure: Box::new(move |_, _, _| (width, height)),
            paint: Box::new(paint),
        },
        children: Vec::new(),
        hit: None,
    }
}

/// 创建按可用空间动态测量的自定义绘制叶节点。
pub fn custom_measured<'a, A>(
    measure: impl FnMut(Option<f32>, Option<f32>, &mut Frame<'_>) -> (f32, f32) + 'a,
    paint: impl FnMut(&mut Frame<'_>, Rect, Rect) + 'a,
) -> Node<'a, A> {
    Node {
        style: Style::default(),
        paint: PaintStyle::default(),
        kind: NodeKind::Custom {
            measure: Box::new(measure),
            paint: Box::new(paint),
        },
        children: Vec::new(),
        hit: None,
    }
}

impl<'a, A> Node<'a, A> {
    fn container(direction: FlexDirection) -> Self {
        Self {
            style: Style {
                display: Display::Flex,
                flex_direction: direction,
                ..Style::default()
            },
            paint: PaintStyle::default(),
            kind: NodeKind::Container,
            children: Vec::new(),
            hit: None,
        }
    }

    // ---- 尺寸 ----

    /// 设置固定宽度（逻辑像素）。
    pub fn w(mut self, width: f32) -> Self {
        self.style.size.width = Dimension::length(width);
        self
    }

    /// 设置固定高度（逻辑像素）。
    pub fn h(mut self, height: f32) -> Self {
        self.style.size.height = Dimension::length(height);
        self
    }

    /// 同时设置宽高（逻辑像素）。
    pub fn size(self, width: f32, height: f32) -> Self {
        self.w(width).h(height)
    }

    /// 宽度占满父容器（`100%`）。
    pub fn w_full(mut self) -> Self {
        self.style.size.width = Dimension::percent(1.0);
        self
    }

    /// 高度占满父容器（`100%`）。
    pub fn h_full(mut self) -> Self {
        self.style.size.height = Dimension::percent(1.0);
        self
    }

    /// 设置最小宽度（逻辑像素）。
    pub fn min_w(mut self, width: f32) -> Self {
        self.style.min_size.width = LengthPercentageAuto::length(width);
        self
    }

    /// 设置最小高度（逻辑像素）。
    pub fn min_h(mut self, height: f32) -> Self {
        self.style.min_size.height = LengthPercentageAuto::length(height);
        self
    }

    /// 设置最大宽度（逻辑像素）。
    pub fn max_w(mut self, width: f32) -> Self {
        self.style.max_size.width = LengthPercentageAuto::length(width);
        self
    }

    /// 设置最大高度（逻辑像素）。
    pub fn max_h(mut self, height: f32) -> Self {
        self.style.max_size.height = LengthPercentageAuto::length(height);
        self
    }

    // ---- 弹性 ----

    /// 等价于 CSS `flex: 1 1 0%`：占满主轴剩余空间并允许收缩。
    pub fn flex_1(mut self) -> Self {
        self.style.flex_grow = 1.0;
        self.style.flex_shrink = 1.0;
        self.style.flex_basis = Dimension::length(0.0);
        self
    }

    /// 按权重 `weight` 分配主轴空间（`flex-grow: weight; flex-shrink: 1; flex-basis: 0`），
    /// 适合按固定列比切分的网格行（如 4:1:1:1 或 7 等分日历）。
    pub fn flex_weight(mut self, weight: f32) -> Self {
        self.style.flex_grow = weight;
        self.style.flex_shrink = 1.0;
        self.style.flex_basis = Dimension::length(0.0);
        self
    }

    /// 设置 `flex-grow`。
    pub fn flex_grow(mut self, grow: f32) -> Self {
        self.style.flex_grow = grow;
        self
    }

    /// 设置 `flex-shrink`。
    pub fn flex_shrink(mut self, shrink: f32) -> Self {
        self.style.flex_shrink = shrink;
        self
    }

    // ---- 边距与间距 ----

    /// 设置四边统一内边距（逻辑像素）。
    pub fn p(mut self, value: f32) -> Self {
        self.style.padding = TaffyRect::length(value);
        self
    }

    /// 设置左右内边距（逻辑像素）。
    pub fn px(mut self, value: f32) -> Self {
        let lp = LengthPercentage::length(value);
        self.style.padding.left = lp;
        self.style.padding.right = lp;
        self
    }

    /// 设置上下内边距（逻辑像素）。
    pub fn py(mut self, value: f32) -> Self {
        let lp = LengthPercentage::length(value);
        self.style.padding.top = lp;
        self.style.padding.bottom = lp;
        self
    }

    /// 设置左内边距（逻辑像素）。
    pub fn pl(mut self, value: f32) -> Self {
        self.style.padding.left = LengthPercentage::length(value);
        self
    }

    /// 设置右内边距（逻辑像素）。
    pub fn pr(mut self, value: f32) -> Self {
        self.style.padding.right = LengthPercentage::length(value);
        self
    }

    /// 设置上内边距（逻辑像素）。
    pub fn pt(mut self, value: f32) -> Self {
        self.style.padding.top = LengthPercentage::length(value);
        self
    }

    /// 设置下内边距（逻辑像素）。
    pub fn pb(mut self, value: f32) -> Self {
        self.style.padding.bottom = LengthPercentage::length(value);
        self
    }

    /// 设置四边统一外边距（逻辑像素）。
    pub fn m(mut self, value: f32) -> Self {
        self.style.margin = TaffyRect::length(value);
        self
    }

    /// 设置左右外边距（逻辑像素）。
    pub fn mx(mut self, value: f32) -> Self {
        let lp = LengthPercentageAuto::length(value);
        self.style.margin.left = lp;
        self.style.margin.right = lp;
        self
    }

    /// 设置上下外边距（逻辑像素）。
    pub fn my(mut self, value: f32) -> Self {
        let lp = LengthPercentageAuto::length(value);
        self.style.margin.top = lp;
        self.style.margin.bottom = lp;
        self
    }

    /// 设置左外边距（逻辑像素）。
    pub fn ml(mut self, value: f32) -> Self {
        self.style.margin.left = LengthPercentageAuto::length(value);
        self
    }

    /// 设置右外边距（逻辑像素）。
    pub fn mr(mut self, value: f32) -> Self {
        self.style.margin.right = LengthPercentageAuto::length(value);
        self
    }

    /// 设置上外边距（逻辑像素）。
    pub fn mt(mut self, value: f32) -> Self {
        self.style.margin.top = LengthPercentageAuto::length(value);
        self
    }

    /// 设置下外边距（逻辑像素）。
    pub fn mb(mut self, value: f32) -> Self {
        self.style.margin.bottom = LengthPercentageAuto::length(value);
        self
    }

    /// 设置子项之间水平与垂直间距（逻辑像素）。
    pub fn gap(mut self, value: f32) -> Self {
        self.style.gap = Size::length(value);
        self
    }

    /// 设置子项之间的水平间距（逻辑像素）。
    pub fn gap_x(mut self, value: f32) -> Self {
        self.style.gap.width = LengthPercentage::length(value);
        self
    }

    /// 设置子项之间的垂直间距（逻辑像素）。
    pub fn gap_y(mut self, value: f32) -> Self {
        self.style.gap.height = LengthPercentage::length(value);
        self
    }

    // ---- 对齐 ----

    /// 交叉轴起始对齐。
    pub fn items_start(mut self) -> Self {
        self.style.align_items = AlignItems::START;
        self
    }

    /// 交叉轴居中对齐。
    pub fn items_center(mut self) -> Self {
        self.style.align_items = AlignItems::CENTER;
        self
    }

    /// 交叉轴末端对齐。
    pub fn items_end(mut self) -> Self {
        self.style.align_items = AlignItems::END;
        self
    }

    /// 交叉轴拉伸填满。
    pub fn items_stretch(mut self) -> Self {
        self.style.align_items = AlignItems::STRETCH;
        self
    }

    /// 主轴起始对齐。
    pub fn justify_start(mut self) -> Self {
        self.style.justify_content = JustifyContent::START;
        self
    }

    /// 主轴居中对齐。
    pub fn justify_center(mut self) -> Self {
        self.style.justify_content = JustifyContent::CENTER;
        self
    }

    /// 主轴末端对齐。
    pub fn justify_end(mut self) -> Self {
        self.style.justify_content = JustifyContent::END;
        self
    }

    /// 主轴两端对齐。
    pub fn justify_between(mut self) -> Self {
        self.style.justify_content = JustifyContent::SPACE_BETWEEN;
        self
    }

    // ---- 裁剪与滚动 ----

    /// 裁剪超出本节点边界的子内容，同时收窄子节点的命中区域。
    pub fn overflow_hidden(mut self) -> Self {
        self.paint.clip = true;
        self.style.overflow.x = Overflow::Hidden;
        self.style.overflow.y = Overflow::Hidden;
        self
    }

    /// 设置纵向滚动偏移量（逻辑像素，正数表示内容向上滚动），并自动启用裁剪。
    pub fn scroll_y(mut self, offset: f32) -> Self {
        self.paint.clip = true;
        self.paint.scroll_y = offset;
        self.style.overflow.y = Overflow::Scroll;
        self
    }

    // ---- 外观 ----

    /// 设置填充底色。
    pub fn bg(mut self, color: Color) -> Self {
        self.paint.bg = Some(color);
        self
    }

    /// 设置圆角半径（逻辑像素）。
    pub fn rounded(mut self, radius: f32) -> Self {
        self.paint.radius = radius;
        self
    }

    /// 设置内侧纯色边框（宽 `width` 逻辑像素）。
    pub fn border(mut self, width: f32, color: Color) -> Self {
        self.paint.border = Some((width, color));
        self
    }

    /// 为节点添加浮动卡片的柔和阴影与 1 像素外边框（底色须不透明）。
    pub fn card_frame(mut self, card: CardFrame) -> Self {
        self.paint.card = Some(card);
        self
    }

    // ---- 文本专属 ----

    /// 设置文本字体种类。
    pub fn font(mut self, font: FontKind) -> Self {
        if let NodeKind::Text(spec) = &mut self.kind {
            spec.font = font;
        }
        self
    }

    /// 设置文本字号（逻辑像素）。
    pub fn text_size(mut self, size: f32) -> Self {
        if let NodeKind::Text(spec) = &mut self.kind {
            spec.size = size;
        }
        self
    }

    /// 设置文本行高（逻辑像素）。
    pub fn line_height(mut self, height: f32) -> Self {
        if let NodeKind::Text(spec) = &mut self.kind {
            spec.line_height = Some(height);
        }
        self
    }

    /// 设置文本颜色。
    pub fn color(mut self, color: Color) -> Self {
        if let NodeKind::Text(spec) = &mut self.kind {
            spec.color = color;
        }
        self
    }

    /// 启用按可用宽度自动折行。
    pub fn wrap(mut self) -> Self {
        if let NodeKind::Text(spec) = &mut self.kind {
            spec.wrap = true;
        }
        self
    }

    /// 设置文本水平对齐方式。
    pub fn text_align(mut self, align: TextAlign) -> Self {
        if let NodeKind::Text(spec) = &mut self.kind {
            spec.align = align;
        }
        self
    }

    /// 设置模拟粗体/斜体。
    pub fn faux(mut self, faux: Faux) -> Self {
        if let NodeKind::Text(spec) = &mut self.kind {
            spec.faux = faux;
        }
        self
    }

    // ---- 组合与条件 ----

    /// 追加一个子节点。
    pub fn child(mut self, child: Node<'a, A>) -> Self {
        self.children.push(child);
        self
    }

    /// 存在时追加一个子节点。
    pub fn child_opt(mut self, child: Option<Node<'a, A>>) -> Self {
        if let Some(child) = child {
            self.children.push(child);
        }
        self
    }

    /// 追加一组子节点。
    pub fn children(mut self, children: impl IntoIterator<Item = Node<'a, A>>) -> Self {
        self.children.extend(children);
        self
    }

    /// 当 `cond` 为真时对自身应用变换 `f`。
    pub fn when(self, cond: bool, f: impl FnOnce(Self) -> Self) -> Self {
        if cond { f(self) } else { self }
    }

    /// 当 `option` 为 `Some(value)` 时对自身应用变换 `f`。
    pub fn when_some<T>(self, option: Option<T>, f: impl FnOnce(Self, T) -> Self) -> Self {
        if let Some(value) = option {
            f(self, value)
        } else {
            self
        }
    }

    // ---- 交互 ----

    /// 把本节点的可交互区域以动作 `action` 注册到绘制产出的 [`crate::HitMap`]。
    pub fn hit(mut self, action: A) -> Self {
        self.hit = Some(action);
        self
    }
}
