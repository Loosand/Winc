//! 重排前后的块位置快照与过渡动画计算（滑动、标记淡入、预览切换淡出）。

use std::collections::HashMap;
use std::sync::Arc;

use winc_render::Color;
use winc_typeset::LaidBlock;

/// 淡入的项：所在顶层块的序号及块内各项的序号，按块的序号排序。
pub type Fading = Vec<(usize, Box<[usize]>)>;

/// 重排后各块从原位置到新位置的过渡。
pub struct Moves {
    /// 各顶层块起始时相对新位置向下的偏移（逻辑像素）。
    pub from: Vec<f32>,
    /// 需要淡入的新增标记项。
    pub fading: Fading,
    /// 刚显示原文的块中渲染结果的首项序号及纵向偏移。
    pub inner: Vec<(usize, u32, f32)>,
    /// 刚恢复排版的块原来的原文残留项。
    pub leaving: Vec<Leaving>,
}

/// 重排之前各顶层块的排版和绘制位置，重排后据此计算过渡动画（见 `BlockMotion`）。
pub struct Before {
    blocks: Vec<(u64, Arc<LaidBlock>, f32)>,
    revision: Option<u64>,
}

/// 刚恢复排版的块原来显示的原文。
pub struct Leaving {
    /// 所属顶层块序号。
    pub block: usize,
    /// 恢复排版前的旧排版块。
    pub old: Arc<LaidBlock>,
    /// 旧排版块中需要淡出的项数。
    pub items: u32,
    /// 淡出时的纵向位置偏移。
    pub offset: f32,
}

impl Before {
    /// 记录各块在滚动位置 `scroll` 下的当前绘制位置，供重排后计算过渡动画。
    pub(super) fn capture(
        blocks: &[(u64, Arc<LaidBlock>)],
        tops: &[f32],
        scroll: f32,
        offset: impl Fn(usize) -> f32,
        revision: Option<u64>,
    ) -> Self {
        let captured = blocks
            .iter()
            .zip(tops)
            .enumerate()
            .map(|(i, ((hash, block), top))| (*hash, Arc::clone(block), top + offset(i) - scroll))
            .collect();
        Self {
            blocks: captured,
            revision,
        }
    }

    /// 计算从本快照过渡到新排版结果所需的各块偏移与淡入项。
    pub(super) fn motion_to(
        &self,
        new: &[(u64, Arc<LaidBlock>)],
        tops: &[f32],
        live_revision: Option<u64>,
        scroll: f32,
        muted: Color,
        slide: bool,
    ) -> Moves {
        let old = &self.blocks;
        let matched: Vec<Option<usize>> = if old.len() == new.len() {
            (0..new.len()).map(Some).collect()
        } else {
            let same = |i: usize, j: usize| new[i].0 == old[j].0;
            let short = old.len().min(new.len());
            let head = (0..short).take_while(|&i| same(i, i)).count();
            let tail = (0..short - head)
                .take_while(|&k| same(new.len() - 1 - k, old.len() - 1 - k))
                .count();
            (0..new.len())
                .map(|i| {
                    if i < head {
                        Some(i)
                    } else if i >= new.len() - tail {
                        Some(i + old.len() - new.len())
                    } else {
                        None
                    }
                })
                .collect()
        };
        let mut from = Vec::with_capacity(new.len());
        let mut last = 0.0;
        for (i, j) in matched.iter().enumerate() {
            if let Some(j) = j {
                last = old[*j].2 - (tops[i] - scroll);
            }
            from.push(last);
        }
        let moved = live_revision == self.revision && self.revision.is_some();
        if !moved && !slide {
            from.fill(0.0);
        }
        let (mut inner, mut leaving) = (Vec::new(), Vec::new());
        for (i, j) in matched.iter().enumerate() {
            let Some(j) = *j else { continue };
            if !moved && !slide {
                break;
            }
            match (old[j].1.preview_split(), new[i].1.preview_split()) {
                (None, Some((first, top))) => inner.push((i, first, -top)),
                (Some((first, top)), None) => {
                    leaving.push(Leaving {
                        block: i,
                        old: Arc::clone(&old[j].1),
                        items: first,
                        offset: from[i],
                    });
                    from[i] += top;
                }
                _ => {}
            }
        }
        let mut fading = Vec::new();
        for (i, j) in matched.iter().enumerate() {
            let Some(j) = *j else { continue };
            if !moved || new[i].0 == old[j].0 {
                continue;
            }
            let mut seen: HashMap<Arc<str>, usize> = HashMap::new();
            for (_, text) in old[j].1.muted_text_marks(muted) {
                *seen.entry(text).or_default() += 1;
            }
            let fresh: Box<[usize]> = new[i]
                .1
                .muted_text_marks(muted)
                .into_iter()
                .filter(|(_, text)| match seen.get_mut(text) {
                    Some(n) if *n > 0 => {
                        *n -= 1;
                        false
                    }
                    _ => true,
                })
                .map(|(k, _)| k)
                .collect();
            if !fresh.is_empty() {
                fading.push((i, fresh));
            }
        }
        Moves {
            from,
            fading,
            inner,
            leaving,
        }
    }
}
