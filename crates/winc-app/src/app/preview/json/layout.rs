//! 从左到右布置卡片：同一层共用一列，相邻子树各占不相交的纵向区域。

use std::collections::HashSet;
use winc_preview::json::{Graph, Row};
use winc_render::Rect;

pub(crate) const ROW_H: f32 = 26.0;
const COLUMN_GAP: f32 = 60.0;
const SIBLING_GAP: f32 = 22.0;

pub(crate) struct Layout {
    pub(crate) cards: Vec<Option<Rect>>,
    pub(crate) size: (f32, f32),
}

impl Layout {
    pub(crate) fn new(
        graph: &Graph,
        folded: &HashSet<usize>,
        measure: impl FnMut(&[Row]) -> f32,
    ) -> Self {
        let mut tree = Tree {
            graph,
            folded,
            measure,
            columns: Vec::new(),
            metrics: vec![(0.0, 0.0); graph.len()],
        };
        let height = tree.measure(0, 0);
        let mut width = 0.0;
        for col in &mut tree.columns {
            let next = width + *col + COLUMN_GAP;
            *col = width;
            width = next;
        }
        let mut cards = vec![None; graph.len()];
        tree.place(&mut cards, 0, 0, 0.0);
        Self {
            cards,
            size: (width - COLUMN_GAP, height),
        }
    }
}

struct Tree<'a, M> {
    graph: &'a Graph,
    folded: &'a HashSet<usize>,
    measure: M,
    columns: Vec<f32>,
    // 每张卡片的宽度、其整棵可见子树所需的高度。
    metrics: Vec<(f32, f32)>,
}

impl<M: FnMut(&[Row]) -> f32> Tree<'_, M> {
    fn children(&self, id: usize) -> impl Iterator<Item = usize> + use<'_, M> {
        self.graph[id]
            .iter()
            .filter_map(|row| row.child)
            .filter(|id| !self.folded.contains(id))
    }

    fn measure(&mut self, id: usize, level: usize) -> f32 {
        let rows = &self.graph[id];
        let width = (self.measure)(rows);
        if level == self.columns.len() {
            self.columns.push(width);
        } else {
            self.columns[level] = self.columns[level].max(width);
        }
        let mut children_height = 0.0;
        for child in rows.iter().filter_map(|row| row.child) {
            if !self.folded.contains(&child) {
                children_height += self.measure(child, level + 1) + SIBLING_GAP;
            }
        }
        let height = (children_height - SIBLING_GAP).max(rows.len() as f32 * ROW_H);
        self.metrics[id] = (width, height);
        height
    }

    fn place(&self, cards: &mut [Option<Rect>], id: usize, level: usize, top: f32) {
        let (width, span) = self.metrics[id];
        let height = self.graph[id].len() as f32 * ROW_H;
        cards[id] = Some(Rect::new(
            self.columns[level],
            top + (span - height) / 2.0,
            width,
            height,
        ));
        let children_height = self
            .children(id)
            .map(|child| self.metrics[child].1 + SIBLING_GAP)
            .sum::<f32>()
            - SIBLING_GAP;
        let mut y = top + (span - children_height) / 2.0;
        for child in self.children(id) {
            self.place(cards, child, level + 1, y);
            y += self.metrics[child].1 + SIBLING_GAP;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sibling_subtrees_do_not_overlap_and_folded_descendants_are_hidden() {
        let graph =
            winc_preview::json::parse(r#"[{"child":{"a":1,"b":2}},{"child":{"c":3}}]"#).unwrap();
        let layout = |folded| Layout::new(&graph, &folded, |_| 100.0);
        let all = layout(HashSet::new());
        let rect = |id: usize| all.cards[id].unwrap();
        assert_eq!(rect(1).x, rect(3).x);
        assert!(rect(0).right() < rect(1).x && rect(1).right() < rect(2).x);
        assert!(rect(2).bottom() < rect(4).y);
        assert!((0..5).all(|id| rect(id).y >= 0.0 && rect(id).bottom() <= all.size.1));
        let folded = layout(HashSet::from([1]));
        assert!(folded.cards[1].is_none() && folded.cards[2].is_none());
        assert!(folded.cards[3].is_some() && folded.cards[4].is_some());
        assert!(folded.size.1 < all.size.1);
    }
}
