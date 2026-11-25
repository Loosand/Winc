//! 勾选式的选择：勾选记在界面中，提交时才把勾上的内容写入暂存区。
//!
//! 已跟踪的文件默认勾上，取消勾选的文件、差异中取消勾选的各处改动和各行记在这里。暂存区视图
//! （[`CommitState::staging`]）不使用这些记录，各项的状态直接取自暂存区。

use winc_core::diff::{LineSet, Pick};
use winc_editor::diff_view::LEFT;

use super::*;

impl CommitState {
    /// 文件 `entry` 是否勾上：已跟踪的文件默认勾上，未跟踪的文件勾上后才算。
    pub(super) fn picked(&self, entry: &Entry) -> bool {
        if entry.untracked() {
            self.included.contains(&entry.path)
        } else {
            !self.excluded.contains(&entry.path)
        }
    }

    /// 文件的勾选框：`Some(true)` 整个提交，`Some(false)` 只提交一部分改动，`None` 不提交。
    ///
    /// 提交的是工作区里的内容，与暂存区无关：在终端里暂存过的文件与其他文件的处理相同。
    pub(super) fn pick_state(&self, entry: &Entry) -> Option<bool> {
        if !self.picked(entry) {
            None
        } else if self.skips_hunks(entry) {
            Some(false)
        } else {
            Some(true)
        }
    }

    /// 文件 `entry` 是否在差异里勾掉了其中几处改动。
    pub(super) fn skips_hunks(&self, entry: &Entry) -> bool {
        self.skipped.get(&entry.path).is_some_and(|s| !s.is_empty())
    }

    /// 返回文件 `entry` 现在各段改动的提交方式，下标同 `all`（各段改动的键和在 `HEAD` 中的行
    /// 区间）。
    pub(in crate::app) fn pick_inclusions(
        &self,
        entry: &Entry,
        all: &[(u64, Range<usize>)],
    ) -> Vec<Pick> {
        if !self.picked(entry) {
            return vec![Pick::Skip; all.len()];
        }
        let skips = self.skipped.get(&entry.path).map_or(&[][..], Vec::as_slice);
        picks(skips, all)
    }

    /// 勾上或勾掉文件 `entry` 的第 `hunk` 处改动；`all` 是该文件现在所有改动的键和在 `HEAD` 中的
    /// 行区间。
    ///
    /// 文件没勾时勾上一处：文件勾上，只提交这一处；一处都不剩时文件也勾掉。
    pub(in crate::app) fn toggle_hunk(
        &mut self,
        entry: &Entry,
        hunk: usize,
        all: &[(u64, Range<usize>)],
    ) {
        if hunk >= all.len() {
            return;
        }
        if !self.picked(entry) {
            self.set_checked(entry, true);
            let others = (all.iter().enumerate())
                .filter(|(h, _)| *h != hunk)
                .map(|(_, change)| Skip::whole(change))
                .collect();
            self.skipped.insert(entry.path.clone(), others);
            return;
        }
        let mut fresh = self.refreshed_skips(entry, all);
        fresh[hunk] = match fresh[hunk] {
            Some(_) => None,
            None => Some(Skip::whole(&all[hunk])),
        };
        self.store_skips(entry, fresh);
    }

    /// 勾上或勾掉文件 `entry` 的第 `hunk` 处改动中的一行。
    ///
    /// `new_len` 是这处改动的新行数，`line` 是（侧，相对这处改动开头的行号），`all` 同
    /// [`CommitState::toggle_hunk`]。文件没勾时勾上一行：文件勾上，只提交这一行。
    pub(in crate::app) fn toggle_line(
        &mut self,
        entry: &Entry,
        (hunk, new_len): (usize, usize),
        (side, line): (usize, usize),
        all: &[(u64, Range<usize>)],
    ) {
        let Some((key, old)) = all.get(hunk).cloned() else {
            return;
        };
        let total = old.len() + new_len;
        let except = || {
            let mut set = full_set(old.len(), new_len);
            toggle_in(&mut set, side, line);
            set
        };
        if !self.picked(entry) {
            self.set_checked(entry, true);
            let mut fresh: Vec<Skip> = (all.iter().enumerate())
                .filter(|(h, _)| *h != hunk)
                .map(|(_, change)| Skip::whole(change))
                .collect();
            fresh.push(Skip::with_lines(key, &old, except(), total));
            fresh.retain(|s| !s.is_empty());
            self.skipped.insert(entry.path.clone(), fresh);
            return;
        }
        let mut fresh = self.refreshed_skips(entry, all);
        let lines = match fresh[hunk].take() {
            Some(Skip {
                lines: Some(mut set),
                ..
            }) => {
                toggle_in(&mut set, side, line);
                set
            }
            Some(_) => except(),
            None => {
                let mut set = LineSet::default();
                toggle_in(&mut set, side, line);
                set
            }
        };
        fresh[hunk] = Some(Skip::with_lines(key, &old, lines, total)).filter(|s| !s.is_empty());
        self.store_skips(entry, fresh);
    }

    /// 返回文件 `entry` 已勾掉的改动在现在的 `all` 上的记录，下标同 `all`（见 [`assign`]）。
    ///
    /// 内容相同的改动沿用原来的记录；内容变了的改动行号不再对应，按整处勾掉记录，不会被算回要
    /// 提交的改动；认不出的记录被丢弃。
    fn refreshed_skips(&self, entry: &Entry, all: &[(u64, Range<usize>)]) -> Vec<Option<Skip>> {
        let skips = self.skipped.get(&entry.path).map_or(&[][..], Vec::as_slice);
        (assign(skips, all).into_iter().zip(all))
            .map(|(found, (key, old))| {
                found.map(|found| {
                    if found.key == *key {
                        Skip {
                            old: old.clone(),
                            ..found.clone()
                        }
                    } else {
                        Skip::whole(&(*key, old.clone()))
                    }
                })
            })
            .collect()
    }

    /// 记下文件 `entry` 勾掉的改动（下标同各段改动）；所有改动都整处勾掉时文件也勾掉。
    fn store_skips(&mut self, entry: &Entry, skips: Vec<Option<Skip>>) {
        let none_left = skips
            .iter()
            .all(|s| s.as_ref().is_some_and(|s| s.lines.is_none()));
        self.skipped
            .insert(entry.path.clone(), skips.into_iter().flatten().collect());
        if none_left {
            self.set_checked(entry, false);
        }
    }

    /// `HEAD` 现在是 `oid`：变了时按块的勾选全部作废。
    pub(in crate::app) fn head_is(&mut self, oid: Option<&str>) {
        if self.head.as_deref() != oid {
            self.head = oid.map(str::to_owned);
            self.skipped.clear();
        }
    }

    /// 勾上或勾掉文件 `entry`：`on` 为 `false` 时勾掉；之前按块勾掉的一处改动随之作废。
    pub(super) fn set_checked(&mut self, entry: &Entry, on: bool) {
        self.checks += 1;
        let path = entry.path.clone();
        // 整个文件勾上或勾掉，之前按块勾掉的作废。
        self.skipped.remove(&path);
        if on {
            self.excluded.remove(&path);
            self.included.insert(path);
        } else {
            self.included.remove(&path);
            self.excluded.insert(path);
        }
    }

    /// 状态里当前勾上的各个文件。
    pub(super) fn picked_entries<'a>(
        &'a self,
        status: &'a Status,
    ) -> impl Iterator<Item = &'a Entry> {
        status.entries.iter().filter(|e| self.picked(e))
    }
}

/// 勾掉的一处改动：按内容（[`winc_core::diff::hunk_key`]）认，内容改过了再按它在 `HEAD` 里的
/// 行认。
///
/// `HEAD` 一侧不会因为编辑工作区而变，改动的内容变了也还认得出来，不会因为内容又改动就被
/// 重新算回要提交的改动。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skip {
    key: u64,
    /// 在 `HEAD` 里的行；纯新增时是插入位置上的空区间。
    old: Range<usize>,
    /// 只勾掉其中几行时的这些行；`None` 表示整处勾掉。行号只在内容等于 `key` 时有效。
    lines: Option<LineSet>,
}

/// 把 `side` 侧第 `line` 行（相对改动开头）加入或移出 `set`。
fn toggle_in(set: &mut LineSet, side: usize, line: usize) {
    let lines = if side == LEFT {
        &mut set.old
    } else {
        &mut set.new
    };
    if !lines.remove(&line) {
        lines.insert(line);
    }
}

/// 返回包含一处改动全部行的集合。
fn full_set(old: usize, new: usize) -> LineSet {
    LineSet {
        old: (0..old).collect(),
        new: (0..new).collect(),
    }
}

impl Skip {
    /// 创建整处勾掉的记录。
    fn whole((key, old): &(u64, Range<usize>)) -> Skip {
        Skip {
            key: *key,
            old: old.clone(),
            lines: None,
        }
    }

    /// 创建勾掉 `lines` 的记录：`total` 是这处改动的总行数，全部勾掉时按整处记录。
    fn with_lines(key: u64, old: &Range<usize>, lines: LineSet, total: usize) -> Skip {
        let whole = lines.old.len() + lines.new.len() == total;
        Skip {
            key,
            old: old.clone(),
            lines: (!whole).then_some(lines),
        }
    }

    /// 是否没有勾掉任何行。
    fn is_empty(&self) -> bool {
        self.lines
            .as_ref()
            .is_some_and(|set| set.old.is_empty() && set.new.is_empty())
    }

    /// 返回内容为 `key` 的改动在这条记录下的提交方式。
    pub(super) fn pick(&self, key: u64) -> Pick {
        match &self.lines {
            Some(lines) if self.key == key => Pick::Lines(lines.clone()),
            _ => Pick::Skip,
        }
    }

    /// 内容变了的一处改动（在 `HEAD` 里的行 `old`）和这条记录的位置是否重叠：重叠时视为同一处。
    fn overlaps(&self, old: &Range<usize>) -> bool {
        if self.old.is_empty() || old.is_empty() {
            // 纯新增：插入位置挨着或落在另一段里。
            self.old.start <= old.end && old.start <= self.old.end
        } else {
            self.old.start < old.end && old.start < self.old.end
        }
    }
}

/// 把勾掉的记录 `skips` 对应到现在的各段改动 `all`（键、在 `HEAD` 中的行区间）上，返回每段
/// 改动对应的记录，下标同 `all`；一条记录最多对应一段改动。
///
/// 内容相同的改动可能有几处（例如两处加入了同一行），键相同，只按键对应会把一条记录同时算到这
/// 几处上：勾掉一处时几处一起显示为勾掉，再勾上时只去掉其中一份，无法勾回。因此依次按三种条件
/// 对应，每次只用还没对应上的记录：键和位置都相同；键相同、位置最近（上方增删了行时位置会变）；
/// 内容变了、位置重叠（见 [`Skip::overlaps`]）。
pub(super) fn assign<'a>(skips: &'a [Skip], all: &[(u64, Range<usize>)]) -> Vec<Option<&'a Skip>> {
    let mut taken = vec![false; skips.len()];
    let mut found: Vec<Option<usize>> = vec![None; all.len()];
    for round in 0..3 {
        for (h, (key, old)) in all.iter().enumerate() {
            if found[h].is_some() {
                continue;
            }
            let mut free = (0..skips.len()).filter(|&i| !taken[i]);
            let pick = match round {
                0 => free.find(|&i| skips[i].key == *key && skips[i].old == *old),
                1 => free
                    .filter(|&i| skips[i].key == *key)
                    .min_by_key(|&i| skips[i].old.start.abs_diff(old.start)),
                _ => free.find(|&i| skips[i].key != *key && skips[i].overlaps(old)),
            };
            if let Some(i) = pick {
                taken[i] = true;
                found[h] = Some(i);
            }
        }
    }
    found.into_iter().map(|i| i.map(|i| &skips[i])).collect()
}

/// 按勾掉的记录 `skips` 返回各段改动 `all` 的提交方式，下标同 `all`（见 [`assign`]）。
pub(super) fn picks(skips: &[Skip], all: &[(u64, Range<usize>)]) -> Vec<Pick> {
    (assign(skips, all).into_iter().zip(all))
        .map(|(skip, (key, _))| skip.map_or(Pick::Apply, |skip| skip.pick(*key)))
        .collect()
}
