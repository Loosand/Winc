//! 声明式界面的通用命中表：按绘制顺序记录各交互区域，查询时逆序匹配（后画的顶层节点优先）。

use winc_render::Rect;

/// 一帧绘制产出的交互命中表。
///
/// 条目按绘制顺序追加：同一点同时落在父节点与子节点、或底层与浮层上时，[`HitMap::at`]
/// 逆序查找，因此后绘制的顶层区域优先命中。
#[derive(Debug, Clone)]
pub struct HitMap<A> {
    entries: Vec<(Rect, A)>,
}

impl<A> Default for HitMap<A> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<A> HitMap<A> {
    /// 创建空命中表。
    pub fn new() -> Self {
        Self::default()
    }

    /// 清空全部命中条目，保留已分配的容量。
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// 记录一块可交互区域；宽或高不大于 0 时忽略。
    pub fn push(&mut self, rect: Rect, action: A) {
        if rect.w > 0.0 && rect.h > 0.0 {
            self.entries.push((rect, action));
        }
    }

    /// 返回落在 `(x, y)` 处的最顶层动作；未命中任何区域时返回 `None`。
    pub fn at(&self, x: f32, y: f32) -> Option<&A> {
        self.entries
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(x, y))
            .map(|(_, action)| action)
    }

    /// 返回动作 `target` 对应的矩形（若重复出现则取最顶层一条），用于测试或弹出菜单锚定。
    pub fn rect_of(&self, target: &A) -> Option<Rect>
    where
        A: PartialEq,
    {
        self.entries
            .iter()
            .rev()
            .find(|(_, action)| action == target)
            .map(|(rect, _)| *rect)
    }

    /// 消费命中表并取出全部 `(矩形, 动作)` 记录，便于与现有命中列表对接。
    pub fn into_entries(self) -> Vec<(Rect, A)> {
        self.entries
    }

    /// 命中表是否为空。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 已记录的命中条目数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}
