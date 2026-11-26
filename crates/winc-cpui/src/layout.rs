//! 桥接 `taffy::TaffyTree` 与 [`winc_render::Frame`]：完成测量、Flexbox 布局、递归绘制与命中收集。

use taffy::LayoutOutput;
use taffy::prelude::{AvailableSpace, NodeId, Size, Style, TaffyTree};
use taffy::style::{AlignItems, Dimension, Display, FlexDirection};
use winc_render::{Faux, Frame, Rect};

use crate::hit::HitMap;
use crate::node::{CardFrame, Node, NodeKind, PaintStyle, TextAlign, TextSpec};
use crate::wrap::wrap_text;

/// 界面字体垂直视觉居中的参考度量。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextBasis {
    /// 界面正文字号（逻辑像素）。
    pub ui_size: f32,
    /// 界面字体小写 `x` 高度（逻辑像素）。
    pub x_height: f32,
    /// 界面字体大写字母高度（逻辑像素）。
    pub cap_height: f32,
}

impl TextBasis {
    /// 返回字号为 `size` 的文本在 `row` 内垂直居中时的基线 y 坐标（逻辑像素，取整）。
    pub fn baseline(self, row: Rect, size: f32) -> f32 {
        let k = if self.ui_size > 0.0 {
            size / self.ui_size
        } else {
            1.0
        };
        let optical = (self.x_height + self.cap_height) * k / 4.0;
        (row.y + row.h / 2.0 + optical).round()
    }
}

/// 布局阶段保留在 `TaffyTree` 上下文之外的节点数据。
struct Entry<'a, A> {
    taffy_id: NodeId,
    paint: PaintStyle,
    kind: NodeKind<'a>,
    children: Vec<usize>,
    hit: Option<A>,
}

/// 已完成 Flexbox 布局的界面树。
///
/// 允许调用方先读取 [`LaidOut::root_size`]（例如把弹窗卡片居中），再调用 [`LaidOut::paint_at`]
/// 绘制到指定起点并产出 [`HitMap`]。
pub struct LaidOut<'a, A> {
    tree: TaffyTree<usize>,
    entries: Vec<Entry<'a, A>>,
    root: usize,
    basis: TextBasis,
}

/// 卡片外框阴影与 1px 描边向矩形外扩展的安全边距（逻辑像素），用于裁剪与脏区相交判断。
const CARD_SHADOW_PAD: f32 = 20.0;

/// 矩形是否具有正面积。
fn non_empty(rect: Rect) -> bool {
    rect.w > 0.0 && rect.h > 0.0
}

impl<'a, A> LaidOut<'a, A> {
    /// 返回根节点布局后的 `(宽, 高)`（逻辑像素）。
    pub fn root_size(&self) -> (f32, f32) {
        self.tree
            .layout(self.entries[self.root].taffy_id)
            .map_or((0.0, 0.0), |layout| (layout.size.width, layout.size.height))
    }

    /// 以 `origin` 为左上角把整棵树绘制到 `frame`，超出 `clip` 的部分裁剪，并返回收集到的 [`HitMap`]。
    ///
    /// 完全落在 `clip` 之外的叶节点会跳过文本排版与自定义绘制回调；带裁剪的不可见容器会直接剪枝整棵子树。
    pub fn paint_at(mut self, frame: &mut Frame<'_>, origin: (f32, f32), clip: Rect) -> HitMap<A> {
        let mut hits = HitMap::new();
        self.paint_node(self.root, origin, clip, clip, frame, &mut hits);
        hits
    }

    /// 以 `origin` 为左上角仅重绘与局部脏区 `dirty` 相交的节点，并仍返回 `clip` 范围内的完整 [`HitMap`]。
    ///
    /// 适合先恢复 [`crate::SurfaceCache`] 静态快照，再仅局部重绘悬停或状态变化的控件区域。
    pub fn paint_dirty(
        mut self,
        frame: &mut Frame<'_>,
        origin: (f32, f32),
        clip: Rect,
        dirty: Rect,
    ) -> HitMap<A> {
        let mut hits = HitMap::new();
        let paint_clip = clip.intersect(dirty);
        self.paint_node(self.root, origin, clip, paint_clip, frame, &mut hits);
        hits
    }

    fn paint_node(
        &mut self,
        index: usize,
        parent_origin: (f32, f32),
        clip: Rect,
        paint_clip: Rect,
        frame: &mut Frame<'_>,
        hits: &mut HitMap<A>,
    ) {
        let taffy_id = self.entries[index].taffy_id;
        let Ok(&layout) = self.tree.layout(taffy_id) else {
            return;
        };
        let rect = Rect::new(
            parent_origin.0 + layout.location.x,
            parent_origin.1 + layout.location.y,
            layout.size.width,
            layout.size.height,
        );
        let style = self.entries[index].paint;
        let visible = rect.intersect(clip);
        let is_visible = non_empty(visible);
        let paint_visible = rect.intersect(paint_clip);
        let card_touches = style.card.is_some()
            && non_empty(
                rect.inset(-CARD_SHADOW_PAD, -CARD_SHADOW_PAD)
                    .intersect(paint_clip),
            );
        let should_paint = non_empty(paint_visible) || card_touches;

        // 不可见的裁剪容器既不可能命中、也不可能露出子节点，整棵子树直接剪枝。
        if style.clip && !is_visible && !card_touches {
            return;
        }

        if should_paint {
            // 1. 卡片外框阴影与 1px 细边（需在底色之前绘制，覆盖区域跳过内部阴影）。
            if let Some(CardFrame {
                shadow,
                border_base,
                border,
            }) = style.card
            {
                let covered = frame.covered(rect, style.radius);
                frame.shadow(rect, style.radius, 16.0, 4.0, shadow, Some(covered));
                let edge = rect.inset(-1.0, -1.0);
                frame.rect_except(edge, border_base, style.radius + 1.0, covered);
                frame.rect_except(edge, border, style.radius + 1.0, covered);
            }

            // 2. 背景底色。
            if let Some(bg) = style.bg
                && non_empty(paint_visible)
            {
                if style.radius > 0.0 {
                    frame.rect_clipped(rect, bg, style.radius, paint_clip);
                } else {
                    frame.rect(paint_visible, bg);
                }
            }

            // 3. 普通内侧描边边框。
            if let Some((width, color)) = style.border
                && width > 0.0
                && non_empty(paint_visible)
            {
                let inset = rect.inset(width / 2.0, width / 2.0);
                let shapes =
                    winc_render::rounded_outline(inset.x, inset.y, inset.w, inset.h, style.radius);
                frame.stroke(&shapes, width, color, paint_clip);
            }

            // 4. 叶节点自身内容绘制。
            let leaf_clip = if style.clip {
                paint_visible
            } else {
                paint_clip
            };
            if non_empty(leaf_clip) {
                let basis = self.basis;
                match &mut self.entries[index].kind {
                    NodeKind::Container => {}
                    NodeKind::Text(spec) => {
                        paint_text(frame, basis, spec, rect, leaf_clip);
                    }
                    NodeKind::Custom { paint, .. } => {
                        paint(frame, rect, leaf_clip);
                    }
                }
            }
        }

        // 5. 注册本节点的命中区域（在子节点之前入表，因此子节点若也带 hit 会在逆序查找中优先命中）。
        if is_visible && let Some(action) = self.entries[index].hit.take() {
            hits.push(visible, action);
        }

        // 6. 递归绘制子节点。
        let children = std::mem::take(&mut self.entries[index].children);
        if children.is_empty() {
            return;
        }
        let child_clip = if style.clip { visible } else { clip };
        let child_paint_clip = if style.clip {
            paint_visible
        } else {
            paint_clip
        };
        let child_origin = (rect.x, rect.y - style.scroll_y);
        if style.clip && non_empty(child_paint_clip) {
            frame.within(child_paint_clip, |frame| {
                for child in children {
                    self.paint_node(
                        child,
                        child_origin,
                        child_clip,
                        child_paint_clip,
                        frame,
                        hits,
                    );
                }
            });
        } else {
            for child in children {
                self.paint_node(
                    child,
                    child_origin,
                    child_clip,
                    child_paint_clip,
                    frame,
                    hits,
                );
            }
        }
    }
}

/// 在可用空间 `available`（逻辑像素，`None` 表示按内容自适应）下对声明式树执行 Flexbox 布局。
///
/// 当节点需要按内容自适应宽度（shrink-to-fit）同时受自身 `max_w` 约束并触发内部文本折行时，
/// 通过一层不拉伸子项的外壳把最大可用宽度以 `Definite` 传入，确保折行文本得到准确高度。
pub fn layout<'a, A>(
    frame: &mut Frame<'_>,
    basis: TextBasis,
    available: (Option<f32>, Option<f32>),
    root: Node<'a, A>,
) -> LaidOut<'a, A> {
    let mut tree: TaffyTree<usize> = TaffyTree::new();
    let mut entries: Vec<Entry<'a, A>> = Vec::new();
    let root_max_w = match root.style.max_size.width.expand() {
        taffy::style::ExpandedLengthPercentageAuto::Length(v) => Some(v),
        _ => None,
    };
    let root_idx = build_tree(&mut tree, &mut entries, root);
    let root_taffy = entries[root_idx].taffy_id;

    let wrap_w = match (available.0, root_max_w) {
        (Some(a), Some(m)) => Some(a.min(m)),
        (Some(a), None) => Some(a),
        (None, Some(m)) => Some(m),
        (None, None) => None,
    };

    let (compute_root, space) = if let Some(max_w) = wrap_w
        && available.0.is_none()
        && let Ok(wrapper) = tree.new_with_children(
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::START,
                size: Size {
                    width: Dimension::length(max_w),
                    height: Dimension::auto(),
                },
                ..Style::default()
            },
            &[root_taffy],
        ) {
        // 用 `align-items: start` 的外壳把 `max_w` 作为确定上限传给子树：
        // 短内容按自身宽度收缩，长内容在 `max_w` 处折行并重新计算高度。
        (
            wrapper,
            Size {
                width: AvailableSpace::Definite(max_w),
                height: available
                    .1
                    .map_or(AvailableSpace::MaxContent, AvailableSpace::Definite),
            },
        )
    } else {
        (
            root_taffy,
            Size {
                width: available
                    .0
                    .map_or(AvailableSpace::MaxContent, AvailableSpace::Definite),
                height: available
                    .1
                    .map_or(AvailableSpace::MaxContent, AvailableSpace::Definite),
            },
        )
    };

    let _ = tree.compute_layout_with_measure(
        compute_root,
        space,
        |inputs, _node_id, context, _style| {
            let Some(&entry_idx) = context.as_deref() else {
                return LayoutOutput::HIDDEN;
            };
            let size = measure_leaf(
                &mut entries[entry_idx].kind,
                inputs.known_dimensions,
                inputs.available_space,
                frame,
            );
            LayoutOutput::from_outer_size(size)
        },
    );

    LaidOut {
        tree,
        entries,
        root: root_idx,
        basis,
    }
}

/// 在固定矩形 `bounds` 内完成布局并直接绘制整棵声明式树，返回收集到的 [`HitMap`]。
pub fn render<'a, A>(
    frame: &mut Frame<'_>,
    basis: TextBasis,
    bounds: Rect,
    root: Node<'a, A>,
) -> HitMap<A> {
    let laid = layout(frame, basis, (Some(bounds.w), Some(bounds.h)), root);
    laid.paint_at(frame, (bounds.x, bounds.y), bounds)
}

/// 在固定矩形 `bounds` 内完成布局，但仅重绘与脏区 `dirty` 相交的节点，返回完整 [`HitMap`]。
pub fn render_dirty<'a, A>(
    frame: &mut Frame<'_>,
    basis: TextBasis,
    bounds: Rect,
    dirty: Rect,
    root: Node<'a, A>,
) -> HitMap<A> {
    let laid = layout(frame, basis, (Some(bounds.w), Some(bounds.h)), root);
    laid.paint_dirty(frame, (bounds.x, bounds.y), bounds, dirty)
}

fn build_tree<'a, A>(
    tree: &mut TaffyTree<usize>,
    entries: &mut Vec<Entry<'a, A>>,
    node: Node<'a, A>,
) -> usize {
    let mut child_indices = Vec::with_capacity(node.children.len());
    let mut child_taffy_ids = Vec::with_capacity(node.children.len());
    for child in node.children {
        let idx = build_tree(tree, entries, child);
        child_taffy_ids.push(entries[idx].taffy_id);
        child_indices.push(idx);
    }

    let entry_idx = entries.len();
    let taffy_id = if child_taffy_ids.is_empty() {
        tree.new_leaf_with_context(node.style, entry_idx)
    } else {
        tree.new_with_children(node.style, &child_taffy_ids)
    }
    .unwrap_or_else(|_| {
        tree.new_leaf(Style::default())
            .expect("empty taffy leaf creation succeeds")
    });

    entries.push(Entry {
        taffy_id,
        paint: node.paint,
        kind: node.kind,
        children: child_indices,
        hit: node.hit,
    });
    entry_idx
}

fn measure_leaf(
    kind: &mut NodeKind<'_>,
    known: Size<Option<f32>>,
    avail: Size<AvailableSpace>,
    frame: &mut Frame<'_>,
) -> Size<f32> {
    let avail_w = match avail.width {
        AvailableSpace::Definite(w) => Some(w.max(0.0)),
        AvailableSpace::MinContent => Some(0.0),
        AvailableSpace::MaxContent => None,
    };

    match kind {
        NodeKind::Container => Size {
            width: known.width.unwrap_or(0.0),
            height: known.height.unwrap_or(0.0),
        },
        NodeKind::Text(spec) => {
            let line_h = spec.line_height.unwrap_or(spec.size + 7.0);
            let limit = known.width.or(avail_w);
            let (measured_w, line_count) = measure_text(frame, spec, limit);
            Size {
                width: known.width.unwrap_or(measured_w),
                height: known.height.unwrap_or(line_count as f32 * line_h),
            }
        }
        NodeKind::Custom { measure, .. } => {
            let (w, h) = measure(known.width, avail_w, frame);
            Size {
                width: known.width.unwrap_or(w),
                height: known.height.unwrap_or(h),
            }
        }
    }
}

fn measure_text(
    frame: &mut Frame<'_>,
    spec: &TextSpec<'_>,
    width_limit: Option<f32>,
) -> (f32, usize) {
    let text = spec.content.as_ref();
    if text.is_empty() {
        return (0.0, 0);
    }
    let measure = |s: &str, frame: &mut Frame<'_>| -> f32 {
        if spec.faux == Faux::default() {
            frame.measure(spec.font, spec.size, s)
        } else {
            frame.measure_faux(spec.font, spec.faux, spec.size, s)
        }
    };

    if !spec.wrap || width_limit.is_none() {
        let mut widest = 0.0_f32;
        let mut count = 0usize;
        for line in text.split('\n') {
            widest = widest.max(measure(line, frame));
            count += 1;
        }
        // 向上取整避免浮点舍入导致绘制时右边缘裁掉 1 像素或误折行。
        return (widest.ceil(), count);
    }

    let max_w = width_limit.unwrap_or(f32::INFINITY);
    let mut widest = 0.0_f32;
    let mut count = 0usize;
    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            count += 1;
            continue;
        }
        let wrapped = wrap_with_spec(frame, spec, paragraph, max_w);
        for line in wrapped {
            widest = widest.max(measure(line, frame));
            count += 1;
        }
    }
    (widest.ceil().min(max_w.ceil()), count)
}

fn wrap_with_spec<'t>(
    frame: &mut Frame<'_>,
    spec: &TextSpec<'_>,
    paragraph: &'t str,
    width: f32,
) -> Vec<&'t str> {
    let (font, faux, size) = (spec.font, spec.faux, spec.size);
    wrap_text(paragraph, width, |s| {
        if faux == Faux::default() {
            frame.measure(font, size, s)
        } else {
            frame.measure_faux(font, faux, size, s)
        }
    })
}

fn paint_text(
    frame: &mut Frame<'_>,
    basis: TextBasis,
    spec: &TextSpec<'_>,
    rect: Rect,
    clip: Rect,
) {
    let text = spec.content.as_ref();
    if text.is_empty() || rect.w <= 0.0 || rect.h <= 0.0 {
        return;
    }
    let line_h = spec.line_height.unwrap_or(spec.size + 7.0);
    // 略微放宽 0.5 像素抵消浮点误差，避免单行测量后在绘制阶段意外折行。
    let wrap_w = rect.w + 0.5;
    let mut y = rect.y;

    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            y += line_h;
            continue;
        }
        let lines: Vec<&str> = if spec.wrap {
            wrap_with_spec(frame, spec, paragraph, wrap_w)
        } else {
            vec![paragraph]
        };
        for line in lines {
            let row = Rect::new(rect.x, y, rect.w, line_h);
            let x = match spec.align {
                TextAlign::Start => row.x,
                TextAlign::Center => {
                    let w = measure_line(frame, spec, line);
                    (row.x + (row.w - w) / 2.0).round()
                }
                TextAlign::End => {
                    let w = measure_line(frame, spec, line);
                    (row.right() - w).round()
                }
            };
            let baseline = basis.baseline(row, spec.size);
            if spec.faux == Faux::default() {
                frame.text(spec.font, spec.size, x, baseline, line, spec.color, clip);
            } else {
                frame.text_faux(
                    spec.font, spec.faux, spec.size, x, baseline, line, spec.color, clip,
                );
            }
            y += line_h;
        }
    }
}

fn measure_line(frame: &mut Frame<'_>, spec: &TextSpec<'_>, line: &str) -> f32 {
    if spec.faux == Faux::default() {
        frame.measure(spec.font, spec.size, line)
    } else {
        frame.measure_faux(spec.font, spec.faux, spec.size, line)
    }
}
