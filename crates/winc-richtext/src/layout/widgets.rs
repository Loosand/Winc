//! 块级虚拟挂件槽位（批注卡片、AI 修订建议等）的排版分配、命中测试与可见枚举。

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use winc_render::Rect;

use crate::decorations::{
    Decoration, DecorationId, DecorationKind, DecorationLayer, DecorationSet, WidgetAction,
};

/// 块级虚拟挂件卡片与相邻正文块之间的纵向留白（逻辑像素）。
pub const WIDGET_GAP: f32 = 8.0;

/// 在排版流中预留好纵向空间的块级虚拟挂件槽位（内容坐标系，逻辑像素）。
#[derive(Debug, Clone, PartialEq)]
pub struct LaidWidget {
    /// 对应的装饰标识。
    pub id: DecorationId,
    /// 所属语义层。
    pub layer: DecorationLayer,
    /// 宿主侧渲染与交互用的稳定键。
    pub key: Arc<str>,
    /// 可选默认卡片标题/摘要文字。
    pub label: Arc<str>,
    /// 卡片携带的交互按钮动作列表。
    pub actions: Vec<WidgetAction>,
    /// 锚定到的原文 UTF-8 字节偏移。
    pub source: usize,
    /// 锚定到的顶层块序号（空文档时为 `None`）。
    pub block: Option<usize>,
    /// `true` 表示排在锚定块下方，`false` 表示排在上方。
    pub after: bool,
    /// 相对内容左上角的矩形 `(0.0, top, width, height)`。
    pub rect: Rect,
}

/// 排版期间按顶层块分桶的块级挂件集合。
pub(super) struct WidgetBuckets<'a> {
    hash: u64,
    before: HashMap<usize, Vec<&'a Decoration>>,
    after: HashMap<usize, Vec<&'a Decoration>>,
    orphan: Vec<&'a Decoration>,
}

impl<'a> WidgetBuckets<'a> {
    /// 根据装饰层与顶层块字节范围表预先将全部块级挂件分桶。
    pub(super) fn collect(
        decorations: Option<&'a DecorationSet>,
        block_count: usize,
        ranges: &[Range<usize>],
    ) -> Self {
        let hash = decorations.map_or(0, DecorationSet::block_widget_fingerprint);
        let mut before: HashMap<usize, Vec<&'a Decoration>> = HashMap::new();
        let mut after: HashMap<usize, Vec<&'a Decoration>> = HashMap::new();
        let mut orphan: Vec<&'a Decoration> = Vec::new();
        if hash != 0
            && let Some(decorations) = decorations
        {
            for dec in decorations.block_widgets() {
                if let DecorationKind::BlockWidget {
                    after: is_after, ..
                } = &dec.kind
                {
                    if block_count == 0 {
                        orphan.push(dec);
                    } else {
                        let source = dec.range.start;
                        let block_idx = block_index_for_source(ranges, source, block_count);
                        if *is_after {
                            after.entry(block_idx).or_default().push(dec);
                        } else {
                            before.entry(block_idx).or_default().push(dec);
                        }
                    }
                }
            }
        }
        Self {
            hash,
            before,
            after,
            orphan,
        }
    }

    /// 挂件集合的布局指纹。
    pub(super) fn hash(&self) -> u64 {
        self.hash
    }

    /// 在第 `block` 块之前排入挂件槽位，返回更新后的块顶纵坐标。
    pub(super) fn place_before(
        &self,
        block: usize,
        width: f32,
        mut top: f32,
        out: &mut Vec<LaidWidget>,
    ) -> f32 {
        if let Some(list) = self.before.get(&block) {
            for dec in list {
                if let DecorationKind::BlockWidget {
                    key: wkey,
                    label,
                    actions,
                    height,
                    after,
                } = &dec.kind
                {
                    let h = height.max(0.0);
                    out.push(LaidWidget {
                        id: dec.id,
                        layer: dec.layer,
                        key: Arc::clone(wkey),
                        label: Arc::clone(label),
                        actions: actions.clone(),
                        source: dec.range.start,
                        block: Some(block),
                        after: *after,
                        rect: Rect::new(0.0, top, width, h),
                    });
                    top += h + WIDGET_GAP;
                }
            }
        }
        top
    }

    /// 在第 `block` 块之后排入挂件槽位，返回更新后的流尾纵坐标。
    pub(super) fn place_after(
        &self,
        block: usize,
        width: f32,
        mut y: f32,
        out: &mut Vec<LaidWidget>,
    ) -> f32 {
        if let Some(list) = self.after.get(&block) {
            for dec in list {
                if let DecorationKind::BlockWidget {
                    key: wkey,
                    label,
                    actions,
                    height,
                    after,
                } = &dec.kind
                {
                    let h = height.max(0.0);
                    let wy = y + WIDGET_GAP;
                    out.push(LaidWidget {
                        id: dec.id,
                        layer: dec.layer,
                        key: Arc::clone(wkey),
                        label: Arc::clone(label),
                        actions: actions.clone(),
                        source: dec.range.start,
                        block: Some(block),
                        after: *after,
                        rect: Rect::new(0.0, wy, width, h),
                    });
                    y = wy + h;
                }
            }
        }
        y
    }

    /// 空文档时排入无归属块的挂件槽位，返回更新后的流尾纵坐标。
    pub(super) fn place_orphans(&self, width: f32, mut y: f32, out: &mut Vec<LaidWidget>) -> f32 {
        for dec in &self.orphan {
            if let DecorationKind::BlockWidget {
                key: wkey,
                label,
                actions,
                height,
                after,
            } = &dec.kind
            {
                let h = height.max(0.0);
                let wy = if y > 0.0 { y + WIDGET_GAP } else { 0.0 };
                out.push(LaidWidget {
                    id: dec.id,
                    layer: dec.layer,
                    key: Arc::clone(wkey),
                    label: Arc::clone(label),
                    actions: actions.clone(),
                    source: dec.range.start,
                    block: None,
                    after: *after,
                    rect: Rect::new(0.0, wy, width, h),
                });
                y = wy + h;
            }
        }
        y
    }
}

/// 在按起点升序排列的顶层块范围切片 `ranges` 中以 $O(\log B)$ 定位 `source` 所属的块序号。
fn block_index_for_source(ranges: &[Range<usize>], source: usize, block_count: usize) -> usize {
    if ranges.is_empty() || block_count == 0 {
        return 0;
    }
    let hi = ranges.partition_point(|r| r.start <= source);
    if hi == 0 {
        return 0;
    }
    let candidate = (hi - 1).min(block_count - 1);
    if ranges[candidate].contains(&source) {
        return candidate;
    }
    if candidate > 0 && ranges[candidate - 1].contains(&source) {
        return candidate - 1;
    }
    if ranges[candidate].end == source {
        return candidate;
    }
    if candidate > 0 && ranges[candidate - 1].end == source {
        return candidate - 1;
    }
    candidate
}

/// 排版结果持有的块级挂件槽位集合与查询接口。
pub(super) struct WidgetSlots {
    items: Vec<LaidWidget>,
    hash: u64,
}

impl WidgetSlots {
    /// 由已完成排版的槽位列表与布局指纹构造。
    pub(super) fn new(items: Vec<LaidWidget>, hash: u64) -> Self {
        Self { items, hash }
    }

    /// 是否包含至少一个块级挂件槽位。
    pub(super) fn has_any(&self) -> bool {
        !self.items.is_empty()
    }

    /// 块级挂件槽位总数。
    pub(super) fn len(&self) -> usize {
        self.items.len()
    }

    /// 布局指纹（无挂件时恒为 `0`）。
    pub(super) fn fingerprint(&self) -> u64 {
        self.hash
    }

    /// 按装饰标识查找对应的已排版块级挂件槽位。
    pub(super) fn find_by_id(&self, id: DecorationId) -> Option<&LaidWidget> {
        self.items.iter().find(|w| w.id == id)
    }

    /// 返回内容坐标 `(cx, cy)` 命中的块级虚拟挂件槽位（利用纵向单调性做 $O(\log W)$ 检索）。
    pub(super) fn hit(&self, cx: f32, cy: f32) -> Option<&LaidWidget> {
        let start = self.items.partition_point(|w| w.rect.bottom() <= cy);
        for w in &self.items[start..] {
            if w.rect.y > cy {
                break;
            }
            if w.rect.contains(cx, cy) {
                return Some(w);
            }
        }
        None
    }

    /// 若第 `block` 块下方存在 `after` 块级虚拟挂件，返回首个挂件的上边缘（相对内容顶部）。
    pub(super) fn top_after(&self, block: usize) -> Option<f32> {
        self.items
            .iter()
            .filter(|w| w.block == Some(block) && w.after)
            .map(|w| w.rect.y)
            .min_by(f32::total_cmp)
    }

    /// 枚举与视口裁剪区 `clip` 相交的块级挂件及其窗口坐标系矩形（$O(\log W + K)$）。
    pub(super) fn visible(
        &self,
        left: f32,
        origin_y: f32,
        clip: Rect,
    ) -> impl Iterator<Item = (&LaidWidget, Rect)> {
        let top = clip.y - origin_y;
        let bottom = clip.bottom() - origin_y;
        let start = self.items.partition_point(|w| w.rect.bottom() <= top);
        let end = self.items.partition_point(|w| w.rect.y < bottom).max(start);
        self.items[start..end].iter().filter_map(move |widget| {
            let r = Rect::new(
                left + widget.rect.x,
                origin_y + widget.rect.y,
                widget.rect.w,
                widget.rect.h,
            );
            (r.bottom() > clip.y && r.y < clip.bottom() && r.h > 0.0).then_some((widget, r))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_search_anchors_and_queries_many_block_widgets() {
        let block_count = 2_000usize;
        let ranges: Vec<Range<usize>> = (0..block_count).map(|i| (i * 20)..(i * 20 + 18)).collect();
        let mut decs = DecorationSet::new();
        for i in 0..200usize {
            let b = i * 10;
            decs.upsert(
                Decoration::block_widget(
                    DecorationId(i as u64 + 1),
                    DecorationLayer::Annotation,
                    b * 20 + 5,
                    format!("note-{i}"),
                    40.0,
                    true,
                )
                .with_label(format!("批注 {i}"))
                .with_action("resolve", "解决", true),
            );
        }

        let buckets = WidgetBuckets::collect(Some(&decs), block_count, &ranges);
        let mut laid = Vec::new();
        let mut y = 0.0_f32;
        for b in 0..block_count {
            y = buckets.place_before(b, 600.0, y, &mut laid);
            y += 24.0;
            y = buckets.place_after(b, 600.0, y, &mut laid);
        }

        let slots = WidgetSlots::new(laid, buckets.hash());
        assert_eq!(slots.len(), 200);
        let target = slots.find_by_id(DecorationId(101)).unwrap();
        assert_eq!(target.block, Some(1000));
        assert_eq!(target.actions.len(), 1);
        assert_eq!(&*target.actions[0].id, "resolve");

        // 命中测试与可见窗口枚举应精确命中目标挂件。
        let hit = slots
            .hit(100.0, target.rect.y + 10.0)
            .expect("should hit widget");
        assert_eq!(hit.id, DecorationId(101));

        let clip = Rect::new(0.0, 0.0, 800.0, 200.0);
        let origin_y = -target.rect.y + 40.0;
        let visible: Vec<_> = slots.visible(20.0, origin_y, clip).collect();
        assert!(visible.iter().any(|(w, _)| w.id == DecorationId(101)));
        assert!(visible.len() <= 3, "只应枚举视口附近的少量挂件");
    }
}
