//! 勾选和折叠的状态，以及按目录分层排好的改动列表。

use winc_core::diff::Pick;
use winc_editor::diff_view::DiffView;
use winc_editor::git::Change;

use super::*;

/// 排好的改动列表：各行，和每组、每个目录下面有哪些文件（画勾选框、点勾选框时用，不必每行
/// 把所有改动翻一遍）。
///
/// 按状态和收起、平铺的样子排一次，缓存到它们变了为止（见 [`App::commit_rows`]）；解引用成
/// 各行。
#[derive(Debug, Default)]
pub(crate) struct CommitRows {
    rows: Vec<CommitRow>,
    /// 各组、各目录下面的文件（`Status::entries` 的下标）：一组、一个目录（收起的也算）下面的
    /// 文件在这里连成一段，它里面的目录是其中的一小段。
    members: Vec<usize>,
    /// 每组的文件在 `members` 里的那一段（下标是 `CommitGroup as usize`）。
    groups: [Range<usize>; CommitGroup::COUNT],
    /// 列表里每个目录行的文件在 `members` 里的那一段：下标同上，键是相对仓库根目录的目录。
    dirs: [HashMap<PathBuf, Range<usize>>; CommitGroup::COUNT],
    /// 组、目录行的勾选框（见 [`CommitRows::members_state`]）：（勾选的版本，按那一段在
    /// `members` 里的位置记下的结果）。
    states: RefCell<MemberStates>,
}

/// [`CommitRows::members_state`] 的缓存：勾选的版本，以及按那一段在 `members` 里的
/// （起点，长度）记下的勾选状态。
type MemberStates = (u64, HashMap<(usize, usize), Option<bool>>);

/// 排列表时读取的折叠与排序设置，使 [`CommitRows`] 不依赖 [`CommitState`]。
struct RowsLayout<'a> {
    staging: bool,
    flat: bool,
    compact: bool,
    sort: CommitSort,
    collapsed: &'a [bool; CommitGroup::COUNT],
    collapsed_dirs: &'a [HashSet<PathBuf>; CommitGroup::COUNT],
}

impl CommitRows {
    /// 返回第 `row` 行所在的组：该行或它之前最近的组标题行；没有这一行时为 `None`。
    pub fn group_of(&self, row: usize) -> Option<CommitGroup> {
        self.rows.get(..=row)?.iter().rev().find_map(|r| match r {
            CommitRow::Group(group) => Some(*group),
            _ => None,
        })
    }

    /// 组 `group` 里的文件（`Status::entries` 的下标）。
    pub fn group(&self, group: CommitGroup) -> &[usize] {
        &self.members[self.groups[group.index()].clone()]
    }

    /// 组 `group` 里目录 `dir`（相对仓库根目录）下面的文件；列表里没有这个目录行时为空。
    pub fn dir(&self, group: CommitGroup, dir: &Path) -> &[usize] {
        self.dirs[group.index()]
            .get(dir)
            .map_or(&[], |r| &self.members[r.clone()])
    }

    /// `members`（本表的 [`Self::group`]、[`Self::dir`] 给出的一段）的勾选状态，含义同
    /// [`CommitState::members_state`]；`checks` 是勾选的版本（[`CommitState::checks`]）。
    ///
    /// 组、目录下面可能有上万个文件，每帧都要画它们的勾选框：按位置记下结果，勾选变了、或者
    /// 重新排了各行（换成另一张表）时才重新算。
    pub fn members_state(
        &self,
        members: &[usize],
        checks: u64,
        compute: impl FnOnce() -> Option<bool>,
    ) -> Option<bool> {
        if members.is_empty() {
            return compute();
        }
        let offset = (members.as_ptr() as usize - self.members.as_ptr() as usize)
            / std::mem::size_of::<usize>();
        let mut states = self.states.borrow_mut();
        if states.0 != checks {
            *states = (checks, HashMap::new());
        }
        *states
            .1
            .entry((offset, members.len()))
            .or_insert_with(compute)
    }

    /// 排下一层已经排好序的文件 `files`（下标、相对路径）：每个一行，深 `depth` 层。
    fn push_files(&mut self, files: &[(usize, &Path)], depth: usize) {
        self.members.extend(files.iter().map(|&(index, _)| index));
        self.rows.extend(
            files
                .iter()
                .map(|&(index, _)| CommitRow::File { index, depth }),
        );
    }

    /// 排列表：每组一行标题，没收起的组下面按目录排成树（或者平铺）。
    ///
    /// 空的组不显示。`root` 是仓库的根目录，文件的路径按它算相对路径。
    fn build(layout: &RowsLayout<'_>, status: &Status, root: &Path) -> CommitRows {
        let mut out = CommitRows::default();
        for group in CommitGroup::ALL {
            let files: Vec<(usize, &Path)> = status
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| group.holds(e, layout.staging))
                .map(|(i, e)| (i, e.path.strip_prefix(root).unwrap_or(&e.path)))
                .collect();
            if files.is_empty() {
                continue;
            }
            out.rows.push(CommitRow::Group(group));
            let start = out.members.len();
            if layout.collapsed[group.index()] {
                out.members.extend(files.iter().map(|&(index, _)| index));
            } else if layout.flat {
                // 平铺：每个文件一行，按选定的方式排。
                let mut files = files;
                sort_files(layout.sort, status, &mut files);
                out.push_files(&files, 0);
            } else {
                out.push_level(layout, status, group, Path::new(""), files, 0);
            }
            out.groups[group.index()] = start..out.members.len();
        }
        out
    }

    /// 把 `dir` 下面的 `items`（下标、相对 `dir` 的路径）排成一层：先是子目录（未收起的继续
    /// 往下排），再是文件。
    fn push_level<'a>(
        &mut self,
        layout: &RowsLayout<'_>,
        status: &Status,
        group: CommitGroup,
        dir: &Path,
        items: Vec<(usize, &'a Path)>,
        depth: usize,
    ) {
        let mut files = Vec::new();
        let mut dirs: BTreeMap<NaturalName<'a>, Vec<(usize, &'a Path)>> = BTreeMap::new();
        for (index, path) in items {
            let mut parts = path.components();
            match (parts.next(), parts.as_path()) {
                (Some(first), rest) if !rest.as_os_str().is_empty() => {
                    dirs.entry(NaturalName(first.as_os_str()))
                        .or_default()
                        .push((index, rest));
                }
                _ => files.push((index, path)),
            }
        }
        for (NaturalName(name), mut sub) in dirs {
            // 只有一个子目录、没有文件：按设置和子目录合成一行。
            let mut label = PathBuf::from(name);
            while let Some(next) = only_subdir(&sub).filter(|_| layout.compact) {
                label.push(next);
                for (_, p) in &mut sub {
                    let rest: &'a Path = p;
                    *p = rest.strip_prefix(next).unwrap_or(Path::new(""));
                }
            }
            let full = dir.join(&label);
            let start = self.members.len();
            self.rows.push(CommitRow::Dir {
                group,
                dir: full.clone(),
                label: label.to_string_lossy().into_owned(),
                depth,
            });
            if layout.collapsed_dirs[group.index()].contains(&full) {
                self.members.extend(sub.iter().map(|&(index, _)| index));
            } else {
                self.push_level(layout, status, group, &full, sub, depth + 1);
            }
            self.dirs[group.index()].insert(full, start..self.members.len());
        }
        sort_files(layout.sort, status, &mut files);
        self.push_files(&files, depth);
    }
}

/// 按 `sort` 排 `files`（各项在状态中的下标和相对仓库根的路径）。
fn sort_files(sort: CommitSort, status: &Status, files: &mut [(usize, &Path)]) {
    let letter = |index: usize| status.entries.get(index).map(|e| e.shown().letter());
    files.sort_by(|a, b| match sort {
        CommitSort::Path => natural_path_cmp(a.1, b.1),
        CommitSort::Name => {
            let name = |path: &Path| path.file_name().map(Path::new).unwrap_or(path).to_owned();
            natural_path_cmp(&name(a.1), &name(b.1)).then_with(|| natural_path_cmp(a.1, b.1))
        }
        CommitSort::Status => {
            (letter(a.0).cmp(&letter(b.0))).then_with(|| natural_path_cmp(a.1, b.1))
        }
    });
}

impl Deref for CommitRows {
    type Target = [CommitRow];

    fn deref(&self) -> &[CommitRow] {
        &self.rows
    }
}

impl App {
    /// 提交视图列表现在的各行（见 [`CommitRows`]）。
    ///
    /// 按状态的版本（`GitState::generation`，换仓库时也变）和收起、平铺的样子缓存：每帧绘制、
    /// 滚动、悬停都要用，不再每次从头排。
    pub(crate) fn commit_rows(&self) -> Rc<CommitRows> {
        rows_of(
            &self.git.commit,
            self.git.status.as_ref(),
            self.git.root.as_deref(),
            self.git.generation,
        )
    }
}

/// 按 `state` 的收起、平铺设置排出 `status` 的各行，按（`generation`，排列设置的版本）缓存在
/// `state` 中；没有状态或仓库时返回空的列表。
pub(in crate::app::git) fn rows_of(
    state: &CommitState,
    status: Option<&Status>,
    root: Option<&Path>,
    generation: u64,
) -> Rc<CommitRows> {
    let (Some(status), Some(root)) = (status, root) else {
        return Rc::default();
    };
    let key = (generation, state.layout);
    let mut cache = state.rows.borrow_mut();
    if let Some((built, rows)) = cache.as_ref()
        && *built == key
    {
        return Rc::clone(rows);
    }
    let rows = Rc::new(state.build_rows(status, root));
    *cache = Some((key, Rc::clone(&rows)));
    rows
}

/// 把 `entry` 改写为暂存（`on` 为真）或取消暂存之后的状态，要写入暂存区的路径加到 `touched`。
///
/// 已经处于该状态的文件不动；冲突中的文件不能取消暂存。
pub(super) fn flip_staged(entry: &mut Entry, on: bool, touched: &mut Vec<PathBuf>) {
    if on {
        if entry.unstaged.is_none() {
            return;
        }
        entry.staged = Some(match (entry.staged, entry.unstaged) {
            (Some(Change::Added), _) | (_, Some(Change::Untracked)) => Change::Added,
            (_, Some(Change::Deleted)) => Change::Deleted,
            (Some(Change::Renamed), _) => Change::Renamed,
            _ => Change::Modified,
        });
        entry.unstaged = None;
        touched.push(entry.path.clone());
    } else {
        let Some(staged) = entry.staged.filter(|_| !entry.conflicted()) else {
            return;
        };
        entry.unstaged = Some(match (staged, entry.unstaged) {
            (Change::Added, _) => Change::Untracked,
            (Change::Deleted, _) | (_, Some(Change::Deleted)) => Change::Deleted,
            _ => Change::Modified,
        });
        entry.staged = None;
        touched.push(entry.path.clone());
        // 改名的文件，原来的路径一并移出暂存区。
        touched.extend(entry.from.clone());
    }
}

impl CommitState {
    /// 返回当前按下「提交」不会提交的原因；可以提交（或正在提交）时为 `None`。
    ///
    /// 检查项与 [`App::git_commit`] 相同。提交按钮据此置灰，悬停提示显示原因。
    /// 冲突标记按状态中读取的结果判断：编辑器中刚移除、尚未保存时按钮仍置灰，
    /// 按下后先保存再重新检查。
    ///
    /// `others` 是其他仓库里随之提交的文件数：勾选式下勾上的（见 [`App::other_checked_count`]），
    /// 暂存区视图中暂存的（见 [`App::other_staged_count`]）；不为 0 时当前仓库没有内容也可以提交。
    pub fn blocker(&self, status: &Status, others: usize) -> Option<String> {
        if self.busy {
            return None;
        }
        let rebasing = status.operation == Some(Operation::Rebase);
        // 勾选式：至少勾上一个文件，勾上的冲突文件不能仍有冲突标记。
        if !self.staging {
            let mut checked = self.picked_entries(status).peekable();
            if checked.peek().is_none() && others == 0 && !self.amend && !rebasing {
                return Some(t!("git-commit-nothing-checked").to_owned());
            }
            let unresolved = checked
                .filter(|e| status.unresolved.contains(&e.path))
                .count();
            if unresolved > 0 {
                return Some(t!("git-commit-blocker-conflicts", count = unresolved));
            }
            if self.message.text().trim().is_empty() && !rebasing {
                return Some(t!("git-commit-message-required").to_owned());
            }
            return None;
        }
        let unresolved = (status.entries.iter())
            .filter(|e| e.conflicted() && status.unresolved.contains(&e.path))
            .count();
        if unresolved > 0 {
            return Some(t!("git-commit-blocker-conflicts", count = unresolved));
        }
        let nothing = matches!(CommitScope::of(status), CommitScope::Nothing)
            && !status.entries.iter().any(Entry::conflicted)
            && others == 0;
        if nothing && !self.amend && status.operation.is_none() {
            return Some(t!("git-commit-nothing-checked").to_owned());
        }
        if self.message.text().trim().is_empty() && !rebasing {
            return Some(t!("git-commit-message-required").to_owned());
        }
        None
    }

    /// 把正在写的提交信息记到位置 `root` 名下；什么都没写时清掉记录。
    pub(in crate::app) fn keep_draft(&mut self, root: PathBuf) {
        let text = self.draft_text();
        if text.trim().is_empty() {
            self.drafts.remove(&root);
        } else {
            self.drafts.insert(root, text);
        }
    }

    /// 用户自己写的提交信息：修改上一次提交时是打开该模式之前写的那一段。
    pub(in crate::app) fn draft_text(&self) -> String {
        if self.amend {
            self.draft.clone().unwrap_or_default()
        } else {
            self.message.text().to_owned()
        }
    }

    /// 填回位置 `root` 名下记着的提交信息。
    pub(in crate::app) fn restore_draft(&mut self, root: &Path) {
        if let Some(text) = self.drafts.remove(root) {
            self.message.set_text(&text);
        }
    }

    /// 文件 `entry` 是否进入提交：勾选式下是它是否勾上，暂存区视图中是暂存区里是否有它的内容
    /// （全部或一部分改动已暂存）。
    pub fn checked(&self, entry: &Entry) -> bool {
        if self.staging {
            entry.staged.is_some() && !entry.conflicted()
        } else {
            self.picked(entry)
        }
    }

    /// 文件进入提交的程度：`Some(true)` 全部改动，`Some(false)` 只有一部分，`None` 不进入提交。
    ///
    /// 勾选式下取自界面中的勾选，与暂存区无关；暂存区视图中取自暂存区，在终端里暂存的文件同样
    /// 算在内。
    pub fn file_state(&self, entry: &Entry) -> Option<bool> {
        if self.staging {
            self.checked(entry).then_some(entry.unstaged.is_none())
        } else {
            self.pick_state(entry)
        }
    }

    /// 返回文件 `entry` 在差异 `view` 中各段改动的暂存状态，下标同 `view.hunks`。
    ///
    /// [`Pick::Apply`] 为已暂存，[`Pick::Skip`] 为未暂存，[`Pick::Lines`] 列出未暂存的行。该文件
    /// 只能按文件暂存（见 [`stages_by_hunk`]）、暂存区中的内容不是文本时为空。
    pub fn hunk_inclusions(&self, entry: &Entry, view: &DiffView) -> Vec<Pick> {
        if !self.staging {
            let all: HunkIds = (view.keys.iter().zip(&view.hunks))
                .map(|(&key, hunk)| (key, hunk.old.clone()))
                .collect();
            return self.pick_inclusions(entry, &all);
        }
        if !stages_by_hunk(entry) {
            return Vec::new();
        }
        let count = view.hunks.len();
        if entry.staged.is_none() {
            return vec![Pick::Skip; count];
        }
        let Some(index) = self.index.get(&entry.path) else {
            // 暂存区中的内容尚未读到：全部暂存的文件各段都已暂存，其余先按未暂存显示。
            let all = entry.unstaged.is_none();
            return vec![if all { Pick::Apply } else { Pick::Skip }; count];
        };
        let Some(text) = &index.text else {
            return Vec::new();
        };
        let all: HunkIds = (view.keys.iter().zip(&view.hunks))
            .map(|(&key, hunk)| (key, hunk.old.clone()))
            .collect();
        let mut cached = index.picks.borrow_mut();
        if let Some((_, picks)) = cached.as_ref().filter(|(of, _)| *of == all) {
            return picks.clone();
        }
        let (head, work) = (view.old.text.to_string(), view.new.text.to_string());
        let picks = winc_core::diff::staged_picks(&head, text, &work, &view.hunks);
        *cached = Some((all, picks.clone()));
        picks
    }

    /// 返回文件 `path` 在暂存区中的内容；尚未读到、不是文本时为 `None`。
    pub(in crate::app) fn index_text(&self, path: &Path) -> Option<&str> {
        self.index.get(path)?.text.as_deref()
    }

    /// 记下文件 `path` 在暂存区中的内容 `text`（不是文本时为 `None`）。
    pub(in crate::app) fn set_index_text(&mut self, path: PathBuf, text: Option<String>) {
        let picks = RefCell::new(None);
        self.index.insert(path, IndexText { text, picks });
    }

    /// 界面改动了暂存区：此前发出的读取结果不再采用，勾选框的缓存作废。
    pub(in crate::app) fn index_changed(&mut self) {
        self.index_version += 1;
        self.checks += 1;
    }

    /// 刚读到的状态是否已经过时：暂存的操作尚未写完，或者这次读取开始于某次暂存的操作之前。
    ///
    /// 后一种情况只对调用时到达的这一份状态成立，记录随调用清除。
    pub(in crate::app) fn outdates_status(&mut self) -> bool {
        std::mem::take(&mut self.status_outdated) || self.staging_active()
    }

    /// 是否有写入暂存区的操作尚未完成。
    pub(in crate::app) fn staging_active(&self) -> bool {
        self.staging_running || !self.staging_queue.is_empty()
    }

    /// 是否有写入其他仓库暂存区的操作尚未完成。
    pub(in crate::app) fn other_staging_active(&self) -> bool {
        self.other_stage_running || !self.other_stage_queue.is_empty()
    }

    /// 组 `group` 是否收起。
    pub fn group_collapsed(&self, group: CommitGroup) -> bool {
        self.collapsed[group.index()]
    }

    /// 是否平铺显示（不按目录排成树）。
    pub fn flat(&self) -> bool {
        self.flat
    }

    /// 组 `group` 里的目录 `dir`（相对仓库根目录）是否收起。
    pub fn dir_collapsed(&self, group: CommitGroup, dir: &Path) -> bool {
        self.collapsed_dirs[group.index()].contains(dir)
    }

    /// 收起或展开组 `group`。
    pub(super) fn toggle_group(&mut self, group: CommitGroup) {
        let i = group.index();
        self.collapsed[i] = !self.collapsed[i];
        self.layout += 1;
    }

    /// 收起或展开组 `group` 里的目录 `dir`。
    pub(super) fn toggle_dir(&mut self, group: CommitGroup, dir: PathBuf) {
        let dirs = &mut self.collapsed_dirs[group.index()];
        if !dirs.remove(&dir) {
            dirs.insert(dir);
        }
        self.layout += 1;
    }

    /// 收起（`collapse` 为真）或展开组 `group` 里的目录 `dir` 和它下面的各级目录。
    pub(super) fn set_dir_tree_collapsed(
        &mut self,
        status: &Status,
        root: &Path,
        (group, dir): (CommitGroup, &Path),
        collapse: bool,
    ) {
        // 收起的目录下面的行不会排出来：先全部展开排一遍，才看得到各级目录。
        let collapsed = std::mem::take(&mut self.collapsed_dirs[group.index()]);
        let all = self.build_rows(status, root);
        self.collapsed_dirs[group.index()] = collapsed;
        let under = all.iter().filter_map(|row| match row {
            CommitRow::Dir {
                group: g, dir: d, ..
            } if *g == group && d.starts_with(dir) => Some(d.clone()),
            _ => None,
        });
        let dirs = &mut self.collapsed_dirs[group.index()];
        for path in under {
            if collapse {
                dirs.insert(path);
            } else {
                dirs.remove(&path);
            }
        }
        self.layout += 1;
    }

    /// 展开 `dir`（相对仓库根目录）及其各级上级目录，以及所在的组，使其中的文件可见。
    pub(crate) fn expand_to(&mut self, dir: &Path) {
        for i in 0..CommitGroup::COUNT {
            self.collapsed[i] = false;
            self.collapsed_dirs[i].retain(|collapsed| !dir.starts_with(collapsed));
        }
        self.layout += 1;
    }

    /// 设定是否把只有一个子目录、没有文件的各级目录合成一行。
    pub(crate) fn set_compact(&mut self, compact: bool) {
        if self.compact != compact {
            self.compact = compact;
            self.layout += 1;
        }
    }

    /// 设定平铺（`true`）或按目录排成树。
    pub(crate) fn set_flat(&mut self, flat: bool) {
        if self.flat != flat {
            self.flat = flat;
            self.layout += 1;
        }
    }

    /// 各组里文件的排序方式。
    pub(crate) fn sort(&self) -> CommitSort {
        self.sort
    }

    /// 设定各组里文件的排序方式。
    pub(crate) fn set_sort(&mut self, sort: CommitSort) {
        if self.sort != sort {
            self.sort = sort;
            self.layout += 1;
        }
    }

    /// 全部收起，或者全部展开（连收起的目录一起展开）。
    pub(super) fn collapse_all(&mut self, collapse: bool) {
        self.collapsed = [collapse; CommitGroup::COUNT];
        if !collapse {
            self.forget_dirs();
        }
        self.layout += 1;
    }

    /// 清除收起的目录记录（换了目录，它们是旧仓库的）。
    pub(in crate::app) fn forget_dirs(&mut self) {
        self.collapsed_dirs.iter_mut().for_each(HashSet::clear);
        self.layout += 1;
    }

    /// 一些文件的勾选状态：`None` 一个都没勾，`Some(true)` 全勾上，`Some(false)` 勾了一部分。
    fn tri_state<'a>(&self, entries: impl IntoIterator<Item = &'a Entry>) -> Option<bool> {
        let (mut on, mut off) = (false, false);
        for entry in entries {
            match self.file_state(entry) {
                Some(true) => on = true,
                Some(false) => return Some(false),
                None => off = true,
            }
            if on && off {
                return Some(false);
            }
        }
        on.then_some(true)
    }

    /// 一组的勾选状态，含义同 [`CommitState::members_state`]。
    pub fn group_state(&self, status: &Status, group: CommitGroup) -> Option<bool> {
        self.tri_state(
            status
                .entries
                .iter()
                .filter(|e| group.holds(e, self.staging)),
        )
    }

    /// `members`（`Status::entries` 的下标，见 [`CommitRows::group`]、[`CommitRows::dir`]）的
    /// 勾选状态：`None` 一个都没勾，`Some(true)` 全勾上，`Some(false)` 勾了一部分。
    pub fn members_state(&self, status: &Status, members: &[usize]) -> Option<bool> {
        self.tri_state(members.iter().filter_map(|&i| status.entries.get(i)))
    }

    /// 排列表：每组一行标题，没收起的组下面按目录排成树（或者平铺）。
    ///
    /// 空的组不显示。`root` 是仓库的根目录，文件的路径按它算相对路径。
    pub(in crate::app::git) fn build_rows(&self, status: &Status, root: &Path) -> CommitRows {
        CommitRows::build(
            &RowsLayout {
                staging: self.staging,
                flat: self.flat,
                compact: self.compact,
                sort: self.sort,
                collapsed: &self.collapsed,
                collapsed_dirs: &self.collapsed_dirs,
            },
            status,
            root,
        )
    }
}

/// 一个文件在暂存区中的内容，以及按它算出的各段改动的暂存状态。
pub(in crate::app::git) struct IndexText {
    /// 暂存区中的内容；不是 UTF-8 文本时为 `None`，该文件只能按文件暂存。
    text: Option<String>,
    /// 最近一次算出的暂存状态，以及当时的各段改动。
    picks: RefCell<Option<(HunkIds, Vec<Pick>)>>,
}

/// 一份差异中各段改动的键和在上次提交中的行区间：两者都相同时差异没有变化。
type HunkIds = Vec<(u64, Range<usize>)>;

/// 提交按钮按下时提交的范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommitScope {
    /// 暂存区中这么多个文件的内容。
    Staged(usize),
    /// 没有暂存任何内容：全部已跟踪文件的改动，共这么多个文件。
    All(usize),
    /// 没有可以提交的内容。
    Nothing,
}

impl CommitScope {
    /// 返回状态 `status` 下的提交范围；冲突中的文件不计入，由提交的入口另行处理。
    pub(crate) fn of(status: &Status) -> CommitScope {
        let plain = || status.entries.iter().filter(|e| !e.conflicted());
        let staged = plain().filter(|e| e.staged.is_some()).count();
        let tracked = plain().filter(|e| !e.untracked()).count();
        match (staged, tracked) {
            (0, 0) => CommitScope::Nothing,
            (0, tracked) => CommitScope::All(tracked),
            (staged, _) => CommitScope::Staged(staged),
        }
    }
}

/// 文件 `entry` 是否可以按段、按行暂存：冲突中的文件、被删除的文件和目录只能整个暂存。
pub(crate) fn stages_by_hunk(entry: &Entry) -> bool {
    use winc_editor::git::Change::Deleted;
    !entry.conflicted()
        && !entry.is_dir
        && entry.staged != Some(Deleted)
        && entry.unstaged != Some(Deleted)
}

/// 两个相对路径按文件视图的自然顺序比（逐级比名字，`2.png` 在 `10.png` 前面）。
fn natural_path_cmp(a: &Path, b: &Path) -> std::cmp::Ordering {
    a.components()
        .zip(b.components())
        .map(|(x, y)| {
            let (x, y) = (
                x.as_os_str().to_string_lossy(),
                y.as_os_str().to_string_lossy(),
            );
            winc_editor::natural_cmp(&x, &y)
        })
        .find(|o| o.is_ne())
        .unwrap_or_else(|| a.components().count().cmp(&b.components().count()))
}

/// `items`（下标、相对路径）都在同一个子目录里、这一层没有文件时，返回那个子目录的名字。
fn only_subdir<'a>(items: &[(usize, &'a Path)]) -> Option<&'a OsStr> {
    let first = |p: &'a Path| {
        let mut parts = p.components();
        let first = parts.next()?.as_os_str();
        (!parts.as_path().as_os_str().is_empty()).then_some(first)
    };
    let next = first(items.first()?.1)?;
    items
        .iter()
        .all(|&(_, p)| first(p) == Some(next))
        .then_some(next)
}

/// 按自然顺序排的目录名，用作 `BTreeMap` 的键。
#[derive(PartialEq, Eq)]
struct NaturalName<'a>(&'a OsStr);

impl Ord for NaturalName<'_> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        winc_editor::natural_cmp(&self.0.to_string_lossy(), &other.0.to_string_lossy())
            .then_with(|| self.0.cmp(other.0))
    }
}

impl PartialOrd for NaturalName<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
