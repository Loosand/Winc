//! 非侵入式装饰层（Decorations & Virtual Widgets）。
//!
//! 对标 `CodeMirror` 6 `Decoration` / `RangeSet` 与蓝图 2.4 / 4.3 / 4.4 节：
//! 搜索高亮、断链提示、批注、AI 原位预览与修订建议均作为独立装饰叠加在视图之上，
//! 不写入 Markdown 原文，也不污染撤销栈；文档持续编辑时通过 [`ChangeMap`] / [`ChangeSeq`]
//! 自动维持与原文位置的稳定锚定。

use std::ops::Range;
use std::sync::Arc;

use winc_render::Color;

use crate::mapping::{Assoc, ChangeMap, ChangeSeq};

/// 装饰条目的稳定标识，用于增量更新或移除单条批注/AI 建议。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DecorationId(pub u64);

/// 装饰所属语义层，支持按层整体刷新或清空。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DecorationLayer {
    /// 查找匹配与当前匹配高亮。
    Search,
    /// 断链或语法校验提示。
    Diagnostic,
    /// 批注与审阅高亮。
    Annotation,
    /// AI 原位补全与修订预览。
    AiPreview,
}

/// 区间装饰的视觉样式。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MarkStyle {
    /// 实色/半透明底色高亮。
    Background(Color),
    /// 底部虚线下划线（如断链提示）。
    DashedUnderline(Color),
    /// 底部实线下划线（如批注/建议锚点）。
    SolidUnderline(Color),
    /// 中部删除线（如 AI 修订建议中的待删片段）。
    Strikethrough(Color),
}

/// 行内虚文字（ghost text / inline widget）的绘制样式。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WidgetStyle {
    /// 文字前景色。
    pub color: Color,
    /// 可选底色（如 AI 插入预览的淡绿底）。
    pub background: Option<Color>,
    /// 是否斜体显示。
    pub italic: bool,
}

/// 块级虚拟挂件卡片上的交互动作描述符（纯数据，不依赖任何界面框架）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WidgetAction {
    /// 动作稳定标识（如 `"accept"`、`"dismiss"`）。
    pub id: Arc<str>,
    /// 按钮展示文案。
    pub label: Arc<str>,
    /// 是否为主操作按钮（如「采纳」「应用」）。
    pub primary: bool,
}

impl WidgetAction {
    /// 构造一条块级挂件动作描述符。
    pub fn new(id: impl Into<Arc<str>>, label: impl Into<Arc<str>>, primary: bool) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            primary,
        }
    }
}

/// 装饰形态：区间标记、行内虚文字或块级虚拟区块。
#[derive(Debug, Clone, PartialEq)]
pub enum DecorationKind {
    /// 覆盖原文一段字节范围的视觉标记。
    Mark {
        /// 标记绘制样式。
        style: MarkStyle,
    },
    /// 锚定在原文某字节偏移处的行内虚拟文字（如 AI 幽灵补全）。
    InlineWidget {
        /// 展示文字（不写入原文）。
        text: Arc<str>,
        /// 绘制样式。
        style: WidgetStyle,
    },
    /// 附着在原文某字节偏移上方或下方的虚拟交互区块（如批注卡片、AI 修订建议条）。
    BlockWidget {
        /// 宿主侧渲染/交互用的稳定键。
        key: Arc<str>,
        /// 可选默认卡片标题/摘要文字（宿主无自定义渲染器时作为可读卡片文案显示）。
        label: Arc<str>,
        /// 卡片携带的交互按钮动作列表（由宿主界面层如 CPUI 渲染并路由命中）。
        actions: Vec<WidgetAction>,
        /// 预留逻辑像素高度。
        height: f32,
        /// `true` 表示排在锚点所在块下方，`false` 表示排在上方。
        after: bool,
    },
}

/// 单条装饰记录。
#[derive(Debug, Clone, PartialEq)]
pub struct Decoration {
    /// 稳定标识。
    pub id: DecorationId,
    /// 所属语义层。
    pub layer: DecorationLayer,
    /// 关联的原文 UTF-8 字节范围（行内/块级挂件可为空范围 `at..at`）。
    pub range: Range<usize>,
    /// 装饰形态与样式。
    pub kind: DecorationKind,
    /// 点锚点遇到同位置插入时的附着方向。
    pub assoc: Assoc,
}

impl Decoration {
    /// 构造一条区间标记装饰。
    pub fn mark(
        id: DecorationId,
        layer: DecorationLayer,
        range: Range<usize>,
        style: MarkStyle,
    ) -> Self {
        Self {
            id,
            layer,
            range,
            kind: DecorationKind::Mark { style },
            assoc: Assoc::After,
        }
    }

    /// 构造一条行内虚拟文字装饰（如 AI 幽灵补全预览）。
    pub fn inline_widget(
        id: DecorationId,
        layer: DecorationLayer,
        at: usize,
        text: impl Into<Arc<str>>,
        style: WidgetStyle,
    ) -> Self {
        Self {
            id,
            layer,
            range: at..at,
            kind: DecorationKind::InlineWidget {
                text: text.into(),
                style,
            },
            assoc: Assoc::After,
        }
    }

    /// 构造一条块级虚拟区块装饰（如批注卡片或 AI 建议卡片）。
    pub fn block_widget(
        id: DecorationId,
        layer: DecorationLayer,
        at: usize,
        key: impl Into<Arc<str>>,
        height: f32,
        after: bool,
    ) -> Self {
        Self {
            id,
            layer,
            range: at..at,
            kind: DecorationKind::BlockWidget {
                key: key.into(),
                label: Arc::from(""),
                actions: Vec::new(),
                height,
                after,
            },
            assoc: Assoc::After,
        }
    }

    /// 为块级虚拟区块设置展示摘要/标题文字。
    pub fn with_label(mut self, text: impl Into<Arc<str>>) -> Self {
        if let DecorationKind::BlockWidget { label, .. } = &mut self.kind {
            *label = text.into();
        }
        self
    }

    /// 为块级虚拟区块追加一个交互操作按钮（如「采纳」「忽略」）。
    pub fn with_action(
        mut self,
        id: impl Into<Arc<str>>,
        label: impl Into<Arc<str>>,
        primary: bool,
    ) -> Self {
        if let DecorationKind::BlockWidget { actions, .. } = &mut self.kind {
            actions.push(WidgetAction::new(id, label, primary));
        }
        self
    }

    fn is_point(&self) -> bool {
        !matches!(self.kind, DecorationKind::Mark { .. })
    }

    fn is_block_widget(&self) -> bool {
        matches!(self.kind, DecorationKind::BlockWidget { .. })
    }
}

/// 有序装饰集合：按 `(range.start, range.end, id)` 维护，支持按区间快速查询与随编辑自动映射。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct DecorationSet {
    items: Vec<Decoration>,
    /// 所有装饰中最长的范围跨度（字节），用于二分检索起点前移，避免漏查跨块长装饰。
    max_span: usize,
    /// 当前集合中块级虚拟挂件的数量，用于零开销判断排版是否受装饰影响。
    block_widget_count: usize,
}

impl DecorationSet {
    /// 创建空的装饰集合。
    pub fn new() -> Self {
        Self::default()
    }

    /// 装饰条目总数。
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// 指定语义层的装饰条目数量。
    pub fn layer_len(&self, layer: DecorationLayer) -> usize {
        self.items.iter().filter(|d| d.layer == layer).count()
    }

    /// 是否没有任何装饰。
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 是否包含任意需要参与排版的块级虚拟挂件。
    pub fn has_block_widgets(&self) -> bool {
        self.block_widget_count > 0
    }

    /// 枚举全部块级虚拟挂件（已按锚点原文位置排序）。
    pub fn block_widgets(&self) -> impl Iterator<Item = &Decoration> {
        self.items.iter().filter(|d| d.is_block_widget())
    }

    /// 计算块级虚拟挂件集合的布局指纹：无块级挂件时恒为 `0`，用于 `Laid` 帧缓存命中判断。
    pub fn block_widget_fingerprint(&self) -> u64 {
        if self.block_widget_count == 0 {
            return 0;
        }
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        for d in self.items.iter().filter(|d| d.is_block_widget()) {
            if let DecorationKind::BlockWidget {
                key,
                label,
                actions,
                height,
                after,
            } = &d.kind
            {
                d.id.hash(&mut h);
                d.layer.hash(&mut h);
                d.range.start.hash(&mut h);
                key.hash(&mut h);
                label.hash(&mut h);
                actions.hash(&mut h);
                height.to_bits().hash(&mut h);
                after.hash(&mut h);
            }
        }
        h.finish().max(1)
    }

    /// 按标识查找单条装饰。
    pub fn get(&self, id: DecorationId) -> Option<&Decoration> {
        self.items.iter().find(|d| d.id == id)
    }

    /// 枚举全部区间标记装饰（`DecorationKind::Mark`）及其原文范围与样式。
    pub fn marks(&self) -> impl Iterator<Item = (&Range<usize>, MarkStyle)> {
        self.items.iter().filter_map(|d| match d.kind {
            DecorationKind::Mark { style } => Some((&d.range, style)),
            _ => None,
        })
    }

    /// 枚举全部行内虚拟挂件（`DecorationKind::InlineWidget`）的锚点、文字与样式。
    pub fn inline_widgets(&self) -> impl Iterator<Item = (usize, &str, WidgetStyle)> {
        self.items.iter().filter_map(|d| match &d.kind {
            DecorationKind::InlineWidget { text, style } => Some((d.range.start, &**text, *style)),
            _ => None,
        })
    }

    /// 枚举指定语义层 `layer` 下的全部装饰。
    pub fn in_layer(&self, layer: DecorationLayer) -> impl Iterator<Item = &Decoration> {
        self.items.iter().filter(move |d| d.layer == layer)
    }

    /// 插入或替换同 [`DecorationId`] 的装饰。
    pub fn upsert(&mut self, decoration: Decoration) {
        if let Some(pos) = self.items.iter().position(|d| d.id == decoration.id) {
            let removed = self.items.remove(pos);
            if removed.is_block_widget() {
                self.block_widget_count = self.block_widget_count.saturating_sub(1);
            }
        }
        if decoration.is_block_widget() {
            self.block_widget_count += 1;
        }
        let span = decoration.range.len();
        self.max_span = self.max_span.max(span);
        let key = (decoration.range.start, decoration.range.end, decoration.id);
        let idx = self
            .items
            .partition_point(|d| (d.range.start, d.range.end, d.id) < key);
        self.items.insert(idx, decoration);
    }

    /// 批量替换指定语义层的全部装饰。
    pub fn replace_layer(
        &mut self,
        layer: DecorationLayer,
        decorations: impl IntoIterator<Item = Decoration>,
    ) {
        self.items.retain(|d| d.layer != layer);
        for d in decorations {
            self.items.push(d);
        }
        self.rebuild_index();
    }

    /// 按标识移除单条装饰，返回是否命中。
    pub fn remove(&mut self, id: DecorationId) -> bool {
        let before = self.items.len();
        self.items.retain(|d| d.id != id);
        if self.items.len() != before {
            self.recompute_meta();
            true
        } else {
            false
        }
    }

    /// 清空指定语义层的全部装饰。
    pub fn clear_layer(&mut self, layer: DecorationLayer) {
        let before = self.items.len();
        self.items.retain(|d| d.layer != layer);
        if self.items.len() != before {
            self.recompute_meta();
        }
    }

    /// 清空全部装饰。
    pub fn clear(&mut self) {
        self.items.clear();
        self.max_span = 0;
        self.block_widget_count = 0;
    }

    /// 文档发生一次编辑后，把全部装饰的原文范围映射到新文档坐标系。
    ///
    /// - 区间标记（`Mark`）若整个范围被删除导致坍缩为空，自动移除；
    /// - 点挂件（`InlineWidget` / `BlockWidget`）按各自的 [`Assoc`] 保持稳定锚定。
    pub fn map(&mut self, change: &ChangeMap) {
        if self.items.is_empty() {
            return;
        }
        self.items.retain_mut(|d| {
            if d.is_point() {
                let at = change.map_pos(d.range.start, d.assoc);
                d.range = at..at;
                true
            } else {
                let mapped = change.map_range(d.range.clone());
                if mapped.is_empty() {
                    false
                } else {
                    d.range = mapped;
                    true
                }
            }
        });
        self.rebuild_index();
    }

    /// 依次经过一组编辑步骤映射全部装饰。
    pub fn map_seq(&mut self, seq: &ChangeSeq) {
        for step in seq.steps() {
            self.map(step);
        }
    }

    /// 枚举与字节区间 `query` 相交（含点挂件落入 `query.start..=query.end`）的全部装饰。
    pub fn in_range(&self, query: Range<usize>) -> impl Iterator<Item = &Decoration> {
        let floor = query.start.saturating_sub(self.max_span);
        let start = self.items.partition_point(|d| d.range.start < floor);
        let end = self.items.partition_point(|d| d.range.start <= query.end);
        self.items[start..end].iter().filter(move |d| {
            if d.is_point() {
                query.start <= d.range.start && d.range.start <= query.end
            } else {
                d.range.start < query.end && d.range.end > query.start
            }
        })
    }

    fn rebuild_index(&mut self) {
        self.items
            .sort_by_key(|d| (d.range.start, d.range.end, d.id));
        self.recompute_meta();
    }

    fn recompute_meta(&mut self) {
        self.max_span = self.items.iter().map(|d| d.range.len()).max().unwrap_or(0);
        self.block_widget_count = self.items.iter().filter(|d| d.is_block_widget()).count();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decorations_remap_across_edits_and_drop_deleted_marks() {
        let mut set = DecorationSet::new();
        assert_eq!(set.block_widget_fingerprint(), 0);
        let bg = MarkStyle::Background(Color::hex(0xffee88));
        let ghost = WidgetStyle {
            color: Color::hex(0x888888),
            background: None,
            italic: true,
        };
        set.upsert(Decoration::mark(
            DecorationId(1),
            DecorationLayer::Annotation,
            10..20,
            bg,
        ));
        set.upsert(Decoration::inline_widget(
            DecorationId(2),
            DecorationLayer::AiPreview,
            25,
            " suggested text",
            ghost,
        ));
        assert_eq!(set.block_widget_fingerprint(), 0);
        set.upsert(
            Decoration::block_widget(
                DecorationId(3),
                DecorationLayer::AiPreview,
                30,
                "ai-card-1",
                48.0,
                true,
            )
            .with_label("AI 修订建议"),
        );
        let fp_initial = set.block_widget_fingerprint();
        assert_ne!(fp_initial, 0);

        // 在装饰之前插入 5 字节：全部向右平移 5 字节。
        set.map(&ChangeMap::single(0, 0, 5));
        assert_eq!(set.get(DecorationId(1)).unwrap().range, 15..25);
        assert_eq!(set.get(DecorationId(2)).unwrap().range, 30..30);
        assert_eq!(set.get(DecorationId(3)).unwrap().range, 35..35);
        assert_ne!(set.block_widget_fingerprint(), fp_initial);

        // 在第一条批注内部插入 3 字节：批注区间自动扩张。
        set.map(&ChangeMap::single(18, 0, 3));
        assert_eq!(set.get(DecorationId(1)).unwrap().range, 15..28);
        assert_eq!(set.get(DecorationId(2)).unwrap().range, 33..33);

        // 完整删除第一条批注覆盖的文本：该批注自动清理，点挂件向左平移。
        set.map(&ChangeMap::single(14, 15, 0));
        assert_eq!(set.len(), 2);
        assert!(set.get(DecorationId(1)).is_none());
        assert_eq!(set.get(DecorationId(2)).unwrap().range, 18..18);
        assert_eq!(set.get(DecorationId(3)).unwrap().range, 23..23);

        // 按区间查询命中正确子集。
        let hit: Vec<_> = set.in_range(15..20).map(|d| d.id).collect();
        assert_eq!(hit, vec![DecorationId(2)]);

        // 按层清空只清该层。
        set.upsert(Decoration::mark(
            DecorationId(4),
            DecorationLayer::Diagnostic,
            2..6,
            MarkStyle::DashedUnderline(Color::hex(0xff3333)),
        ));
        set.clear_layer(DecorationLayer::AiPreview);
        assert_eq!(set.len(), 1);
        assert_eq!(
            set.in_layer(DecorationLayer::Diagnostic)
                .map(|d| d.id)
                .collect::<Vec<_>>(),
            vec![DecorationId(4)]
        );
        assert_eq!(set.block_widget_fingerprint(), 0);
    }
}
