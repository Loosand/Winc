//! 分支选择器：从提交视图的分支行就地展开，或者从状态栏的分支名往上展开，两处是同一个面板。
//!
//! 搜索框下面是一排拉取、推送、获取（按住 ⌥ 时拉取换成拉取并变基）：分支和同步都在这里，
//! 状态栏不再另有一套分支菜单。
//! 依次是最近使用、本地分支和可以展开的远程分支，分支名按 `/` 分层，只展开当前分支所在的目录。
//! 同一个分支只列一次：本地分支一屏放得下时不单列最近使用，单列时下面不再重复；已有本地分支的
//! 远程分支只在搜索时列出。
//! 输入框既能搜索也能新建：搜索时目录全部展开，没有同名分支时最后一行是「新建分支」。回车切换
//! 到选中的分支，有未提交的更改时用提示框询问带过去还是储藏；右键或行尾的「⋯」打开分支的操作菜单。

use winc_editor::git::{Branch, Status};
use winc_i18n::t;
use winc_render::Rect;

use super::super::location_picker::Place;
pub use super::super::operate::{BranchList, BranchRow, GitAnchor, PickerPart};
use super::super::operate::{GIT_PICKER_ROWS, LogConfirmed};
use super::{TagOp, local_name};
use crate::app::animation::TREE_ANIMATION;
use crate::app::input::context_menu::{MenuCommand, MenuTarget};
use crate::app::input::palette::{matched, needle};
use crate::app::{AlertButton, App, Clock, PICKER_ROW, UserEvent};
use crate::layout::{MENU_PICKER_BAR, MENU_PICKER_PAD};

/// 选择面板最小、最大的宽度（逻辑像素）：平时和 Git 上下文的行一样宽，两边对齐。最小宽度放得下
/// 较长的分支名和后面的时间；最大宽度与窗口内绘制的菜单相同，名字和右端的文字不至于隔得太远。
const GIT_PICKER_WIDTH: (f32, f32) = (260.0, 420.0);
/// 选择面板和触发它的按钮之间的间距（逻辑像素）。
const GIT_PICKER_GAP: f32 = 4.0;
/// 选择面板底栏的高度（逻辑像素）：上面留 4，`BUTTON_SMALL` 高的按钮，下面留 6，和顶部的搜索行
/// 对称。
const GIT_PICKER_FOOT: f32 = 34.0;
/// 分支选择器搜索框下面的操作，自上而下各占一行。
pub const GIT_PICKER_ACTION_LIST: [PickerPart; 5] = [
    PickerPart::Pull,
    PickerPart::Push,
    PickerPart::Fetch,
    PickerPart::NewBranch,
    PickerPart::NewWorktree,
];
/// 分支选择器中操作一段的高度（逻辑像素）：上面留 `GIT_PICKER_ACTIONS_TOP`，各项一行，下面是
/// 一条分隔线和它上下的留白。
pub const GIT_PICKER_ACTIONS: f32 = GIT_PICKER_ACTIONS_TOP
    + GIT_PICKER_ACTION_LIST.len() as f32 * PICKER_ROW
    + GIT_PICKER_ACTIONS_END;
/// 操作一段上方的留白（逻辑像素）。
pub const GIT_PICKER_ACTIONS_TOP: f32 = 4.0;
/// 操作一段下方分隔线占的高度（逻辑像素）：线在正中，上下各留 2；加上各行底色上下各缩的 1，
/// 线到两边底色的距离相等。
pub const GIT_PICKER_ACTIONS_END: f32 = 5.0;
/// 从状态栏展开时面板的宽度（逻辑像素）：和默认宽度的侧栏里从分支行展开时一样宽（侧栏 300
/// 减去两边各 8 的边距）。
const STATUS_PICKER_W: f32 = crate::theme::DEFAULT_PANEL_WIDTH - 16.0;

#[cfg(test)]
fn branch_rows(list: &BranchList<'_>) -> Vec<BranchRow> {
    list.rows()
}

#[cfg(test)]
fn without_recent(rows: Vec<BranchRow>) -> Vec<BranchRow> {
    BranchRow::without_recent(rows)
}

/// 已跟踪文件上未提交的更改数：切换分支时只有它们需要考虑，未跟踪的文件不受影响。
fn tracked_changes(status: &Status) -> usize {
    status.entries.iter().filter(|e| !e.untracked()).count()
}

/// 目录、组展开或收起之前的状态。
#[derive(Debug, Clone)]
pub(crate) struct FoldBefore {
    /// 分支选择器的各行。
    pub branches: Vec<BranchRow>,
    /// 展开或收起的那一行的下标。
    pub parent: usize,
    /// 面板的高度（逻辑像素）。
    height: f32,
    /// 滚动量（逻辑像素）。
    scroll: f32,
}

/// 选择面板里目录、组展开或收起的动画，实现方式与改动列表的 `CommitMotion` 相同：子行从父行下方
/// 逐渐显示或收回，其后的各行随之移动，面板的高度和滚动量同时过渡到新的位置。
#[derive(Debug, Clone)]
pub(crate) struct PickerFold {
    /// 之前的状态：收起时画它的各行（收回的行还在），展开时画现在的各行。
    pub before: FoldBefore,
    /// 露出或收回的行数。
    pub count: usize,
    /// 是展开；`false` 是收起。
    pub expanding: bool,
    /// 这一段动画的时钟。
    pub(in crate::app) clock: Clock,
}

impl PickerFold {
    /// 还没露出来的子行占的高度（逻辑像素）：下面的行往上收该距离。
    pub fn hidden(&self) -> f32 {
        let eased = self.clock.progress();
        let reveal = if self.expanding { eased } else { 1.0 - eased };
        (1.0 - reveal) * self.count as f32 * PICKER_ROW
    }

    /// 展开的箭头转过的比例：0 朝右，1 朝下。
    pub fn turned(&self) -> f32 {
        1.0 - self.hidden() / (self.count as f32 * PICKER_ROW)
    }

    /// 现在的第 `row` 行在动画期间所画各行里的下标：收起时收回的行还在，下面的行要往后数。
    pub fn drawn_index(&self, row: usize) -> usize {
        if !self.expanding && row > self.before.parent {
            row + self.count
        } else {
            row
        }
    }
}

impl App {
    /// 记下选中的行展开或收起之前的状态，改完以后交给 [`App::start_picker_fold`]。
    pub(in crate::app) fn picker_fold_before(&self) -> FoldBefore {
        FoldBefore {
            branches: self.overlays.picker.branch_rows.clone().unwrap_or_default(),
            parent: self.overlays.picker.selected,
            height: self.layout.picker.h,
            scroll: self.picker_drawn_scroll(),
        }
    }

    /// 选中的行刚展开或收起：调整滚动量，开始动画。
    ///
    /// 展开时让新露出的行尽量都看得见，展开的那一行不滚出去；收起以后不留空白。打开了「减弱动态
    /// 效果」时直接到位。
    pub(in crate::app) fn start_picker_fold(&mut self, before: FoldBefore) {
        self.overlays.picker.fold = None;
        self.relayout();
        let rows = self.overlays.picker.hits.len();
        let was = before.branches.len();
        let (expanding, count) = (rows > was, rows.abs_diff(was));
        let span = crate::app::workspace::picker::picker_span(self);
        let mut scroll = self.overlays.picker.scroll;
        if expanding {
            let bottom = (before.parent + 1 + count) as f32 * PICKER_ROW;
            let top = before.parent as f32 * PICKER_ROW;
            scroll = scroll.max(bottom - span).min(top);
        }
        let most = (rows as f32 * PICKER_ROW - span).max(0.0);
        self.overlays.picker.scroll = scroll.clamp(0.0, most);
        if count > 0 {
            self.overlays.picker.fold = self.clock(TREE_ANIMATION).map(|clock| PickerFold {
                before,
                count,
                expanding,
                clock,
            });
        }
        self.relayout();
        self.request_redraw();
    }

    /// 绘制用的滚动量：展开、收起的动画期间从原来的位置过渡到新的位置。
    pub(crate) fn picker_drawn_scroll(&self) -> f32 {
        (self.overlays.picker.fold.as_ref()).map_or(self.overlays.picker.scroll, |fold| {
            fold.clock
                .between(fold.before.scroll, self.overlays.picker.scroll)
        })
    }

    /// 把面板摆到触发它的按钮下方，左右和提交视图的行对齐。放不下全部行时滚动。
    pub(in crate::app) fn place_git_picker(&mut self) {
        if !self.overlays.picker.open || !self.picker_is_git() {
            return;
        }
        let window = self.layout.window;
        let most = (window.w - 16.0).max(0.0);
        let chrome = MENU_PICKER_BAR + MENU_PICKER_PAD + self.git_picker_footer();
        let wanted = self.overlays.picker.hits.len().clamp(1, GIT_PICKER_ROWS);
        let chrome = chrome
            + if self.picker_has_actions() {
                GIT_PICKER_ACTIONS
            } else {
                0.0
            };
        let list = |room: f32| {
            let fit = ((room - chrome) / PICKER_ROW).floor() as usize;
            wanted.min(fit.max(1)) as f32 * PICKER_ROW
        };
        // 展开、收起的动画期间高度跟着过渡。
        let height = |h: f32| {
            (self.overlays.picker.fold.as_ref())
                .map_or(h, |fold| fold.clock.between(fold.before.height, h))
        };
        if self.overlays.picker.anchor == GitAnchor::Status {
            // 从状态栏往上展开：底边贴着分支名的上沿，右边和它对齐，放不下时往左挪。
            let Some(&(item, _)) =
                (self.hits.status.iter()).find(|(_, i)| *i == crate::app::StatusItem::Branch)
            else {
                return;
            };
            let bottom = item.y - GIT_PICKER_GAP;
            let room = bottom - crate::theme::TOP_BAR_END;
            let w = STATUS_PICKER_W.min(most);
            let h = height(chrome + list(room));
            let x = (item.right() - w).min(window.right() - 8.0 - w).max(8.0);
            self.layout.picker = Rect::new(x, bottom - h, w, h);
            return;
        }
        let (row, bottom) = if self.picker_filters_log() {
            // 从日志标题栏的「分支」按钮往下展开：位置取绘制时记录的按钮区域。底边与日志列表的
            // 底边相距 8，与面板离窗口左右边缘的距离相同。
            let Some(&(button, _)) =
                (self.hits.log.iter()).find(|(_, t)| *t == crate::app::git::log::LogTarget::Branch)
            else {
                return;
            };
            (button, self.log_parts().list.bottom() - 8.0)
        } else {
            let parts = self.commit_parts();
            let row = match self.overlays.picker.anchor {
                GitAnchor::Location => parts.location.unwrap_or(parts.branch),
                _ => parts.branch,
            };
            let sidebar = self.layout.sidebar;
            (row, sidebar.bottom() - (row.x - sidebar.x))
        };
        let y = row.bottom() + GIT_PICKER_GAP;
        let room = bottom - y;
        let w = row
            .w
            .clamp(GIT_PICKER_WIDTH.0, GIT_PICKER_WIDTH.1)
            .min(most);
        let h = height(chrome + list(room));
        let x = row.x.min(window.right() - 8.0 - w).max(8.0);
        self.layout.picker = Rect::new(x, y, w, h);
    }

    /// 点了选择面板里的 `part`。
    pub(in crate::app) fn press_picker_part(&mut self, part: PickerPart) {
        match part {
            PickerPart::NoteTitles => {
                if let Some(search) = &mut self.overlays.picker.notebook {
                    search.titles_only = !search.titles_only;
                    self.picker_refresh();
                    self.request_redraw();
                }
            }
            PickerPart::NotePreview => {
                if let Some(search) = &mut self.overlays.picker.notebook {
                    search.preview = !search.preview;
                    self.request_redraw();
                }
            }
            PickerPart::CloseNoteSearch => self.close_picker(),
            PickerPart::PropertyCalendar(part) => self.press_property_calendar(part),
            PickerPart::More(row) => self.press_picker_more(row),
            PickerPart::Pull | PickerPart::Push | PickerPart::Fetch => {
                if !self.picker_action_enabled(part) {
                    return;
                }
                let command = self.picker_action_command(part);
                self.close_picker();
                self.run_branch_command(command);
            }
            // 新建分支使用窗口正中的表单对话框（名称、基于、创建后签出），与命令面板和分支菜单中
            // 的「新建分支」共用。
            PickerPart::NewBranch => {
                self.close_picker();
                self.open_branch_dialog(None, None);
            }
            PickerPart::NewWorktree => {
                self.close_picker();
                self.open_worktree_dialog();
            }
            PickerPart::ForgetFolder(row) => self.forget_picker_folder(row),
            PickerPart::ForgetRemote(row) => self.forget_remote_row(row),
        }
        self.request_redraw();
    }

    /// 点了第 `row` 行行尾的「⋯」。
    fn press_picker_more(&mut self, row: usize) {
        self.overlays.picker.selected = row;
        let branch =
            (self.overlays.picker.branch_rows.as_ref()).and_then(|rows| match rows.get(row) {
                Some(BranchRow::Branch { index, .. }) => Some(*index),
                _ => None,
            });
        if let Some(index) = branch {
            self.open_branch_row_menu(MenuTarget::BranchOps(index));
            return;
        }
        let tag =
            (self.overlays.picker.branch_rows.as_ref()).and_then(|rows| match rows.get(row) {
                Some(BranchRow::Tag(index)) => Some(*index),
                _ => None,
            });
        if let Some(index) = tag {
            self.open_branch_row_menu(MenuTarget::TagOps(index));
            return;
        }
        let rows = self.overlays.picker.location_rows.as_ref();
        let place = rows.and_then(|rows| rows.get(row)).cloned();
        if let Some(place) = place {
            self.open_location_row_menu(row, &place);
        }
    }

    /// 选择面板的第 `row` 行能否选中：列出分支时每一行都能选，只要这一行存在。
    pub(in crate::app) fn picker_row_selectable(&self, row: usize) -> bool {
        (self.overlays.picker.branch_rows.as_ref()).is_none_or(|rows| row < rows.len())
    }

    /// 面板是否正在列出分支。
    pub(crate) fn picker_lists_branches(&self) -> bool {
        self.overlays.picker.branch_rows.is_some()
    }

    /// 是否正从 `anchor` 展开着分支选择器：触发它的按钮据此保持按下的样式。
    pub(crate) fn branch_picker_from(&self, anchor: GitAnchor) -> bool {
        self.picker_lists_branches() && self.overlays.picker.anchor == anchor
    }

    /// 是否为日志的分支筛选：从日志标题栏展开，选中的分支只修改日志的筛选条件，不切换分支。
    pub(crate) fn picker_filters_log(&self) -> bool {
        self.branch_picker_from(GitAnchor::Log)
    }

    /// 返回选择面板底栏的高度（逻辑像素）：只有列出工作树时有底栏。
    pub(crate) fn git_picker_footer(&self) -> f32 {
        if self.picker_lists_worktrees() {
            GIT_PICKER_FOOT
        } else {
            0.0
        }
    }

    /// 搜索框下面有没有操作一段（拉取、推送、获取、新建分支、新建工作树）：列出分支时有。
    pub(crate) fn picker_has_actions(&self) -> bool {
        self.picker_lists_branches() && !self.picker_filters_log()
    }

    /// 拉取、推送、获取（`part`）对应的 Git 操作：按住 ⌥ 时拉取是拉取并变基。
    pub(crate) fn picker_action_command(&self, part: PickerPart) -> MenuCommand {
        match part {
            PickerPart::Pull if self.pointer.modifiers.alt_key() => MenuCommand::GitPullRebase,
            PickerPart::Pull => MenuCommand::GitPull,
            PickerPart::Push => MenuCommand::GitPush,
            _ => MenuCommand::GitFetch,
        }
    }

    /// 操作一段里的 `part` 现在能不能用：新建分支、新建工作树始终可用，其余见
    /// [`Self::picker_action_enabled`]。
    pub(crate) fn picker_part_enabled(&self, part: PickerPart) -> bool {
        match part {
            PickerPart::NewBranch | PickerPart::NewWorktree => true,
            part => self.picker_action_enabled(part),
        }
    }

    /// 命令「Git：切换分支…」：提交视图开着时从分支行展开分支选择器，否则从状态栏的分支名；状态栏
    /// 也收着时先打开提交视图。
    pub(in crate::app) fn switch_branch_from_command(&mut self) {
        if self.commit_shown() {
            return self.open_branch_picker(GitAnchor::Branch);
        }
        let on_status =
            (self.hits.status.iter()).any(|(_, i)| *i == crate::app::StatusItem::Branch);
        if on_status {
            return self.open_branch_picker(GitAnchor::Status);
        }
        self.show_commit();
        self.open_branch_picker(GitAnchor::Branch);
    }

    /// 按查询重新排列各行，并选中一行。
    ///
    /// 没有查询（只有空白字符同样视为没有）时：日志的分支筛选选中已应用的筛选；其余情况在没有
    /// 单列最近使用时选中当前分支，选中的行记为自动选中（`Picker::default`）。其他情况选中第一个
    /// 分支，没有分支时选中第一个能选中的行。
    pub(in crate::app) fn branch_picker_refresh(&mut self) {
        let rows = self.current_branch_rows();
        let plain = needle(self.overlays.picker.input.text()).is_empty();
        let current = if !plain {
            None
        } else if self.picker_filters_log() {
            self.applied_log_filter_row(&rows)
        } else if matches!(rows.first(), Some(BranchRow::Locals { recent: true, .. })) {
            None
        } else {
            rows.iter().position(|row| {
                matches!(row, BranchRow::Branch { index, .. }
                    if self.git.branches.get(*index).is_some_and(|b| b.current))
            })
        };
        self.overlays.picker.selected = current
            .or_else(|| {
                rows.iter()
                    .position(|row| matches!(row, BranchRow::Branch { .. }))
            })
            .or_else(|| (!rows.is_empty()).then_some(0))
            .unwrap_or(0);
        self.overlays.picker.default = plain.then_some(self.overlays.picker.selected);
        self.set_branch_rows(rows);
        self.picker_reveal();
    }

    /// 返回日志分支筛选中已应用的筛选所在的行；筛选的分支已不在列表中（如已删除）时返回
    /// 「全部」一行，回车即清除该筛选。
    fn applied_log_filter_row(&self, rows: &[BranchRow]) -> Option<usize> {
        let filter = self.log_branch_filter();
        let applied = rows.iter().position(|row| match row {
            BranchRow::Branch { index, .. } => {
                (self.git.branches.get(*index)).is_some_and(|b| filter == Some(b.name.as_str()))
            }
            _ => false,
        });
        applied.or_else(|| rows.iter().position(|row| *row == BranchRow::All))
    }

    /// 分支列表从 `before` 刷新以后重排：下标可能已经指向别的分支，选中项按名字留住。
    pub(in crate::app) fn branch_picker_refreshed(&mut self, before: &[Branch]) {
        if self.picker_lists_branches() {
            self.keep_branch_selection(before);
        }
    }

    /// 查询在名字 `text` 里命中的字符下标：绘制时标出来。
    pub(crate) fn git_picker_matched(&self, text: &str) -> Vec<usize> {
        matched(&needle(self.overlays.picker.input.text()), text)
    }

    /// 重新排列各行，留住 `before` 列表下选中的那一项；它不在了时选中第一个能选中的行。
    fn keep_branch_selection(&mut self, before: &[Branch]) {
        let selected = (self.overlays.picker.branch_rows.as_ref())
            .and_then(|rows| rows.get(self.overlays.picker.selected))
            .cloned();
        let default = self.overlays.picker.default == Some(self.overlays.picker.selected);
        let rows = self.current_branch_rows();
        let same = |row: &BranchRow| {
            selected
                .as_ref()
                .is_some_and(|s| s.same(row, before, &self.git.branches))
        };
        self.overlays.picker.selected = rows
            .iter()
            .position(same)
            .or_else(|| (!rows.is_empty()).then_some(0))
            .unwrap_or(0);
        // 自动选中的行换了位置仍是自动选中的。
        self.overlays.picker.default = default.then_some(self.overlays.picker.selected);
        self.set_branch_rows(rows);
        self.relayout();
        self.request_redraw();
    }

    fn current_branch_rows(&self) -> Vec<BranchRow> {
        let query = needle(self.overlays.picker.input.text());
        let rows = BranchList {
            branches: &self.git.branches,
            recent: &self.git.recent_branches,
            query: &query,
            text: self.overlays.picker.input.text(),
            toggled: &self.git.branch_folders,
            tags: &self.git.tags,
        }
        .rows();
        if !self.picker_filters_log() {
            return rows;
        }
        // 日志的分支筛选：与平时同样的分组、目录（日志左侧的分支面板也是这样），最上面是「全部
        // 分支」；没有「最近」一组（见 `without_recent`），筛选不新建分支。
        let all = query.is_empty().then_some(BranchRow::All);
        let rows = rows
            .into_iter()
            .filter(|r| !matches!(r, BranchRow::Create(_)));
        all.into_iter().chain(rows).collect()
    }

    fn set_branch_rows(&mut self, rows: Vec<BranchRow>) {
        self.overlays.picker.hits = (0..rows.len()).collect();
        self.overlays.picker.branch_rows = Some(rows);
    }

    /// 返回向下（`down`）或向上移动一步后选中的行，跳过分组标题；到达两端后不动。
    pub(in crate::app) fn branch_picker_step(&self, down: bool) -> usize {
        let rows = self
            .overlays
            .picker
            .branch_rows
            .as_deref()
            .unwrap_or_default();
        let at = self.overlays.picker.selected;
        let next = if down {
            (at + 1 < rows.len()).then_some(at + 1)
        } else {
            at.checked_sub(1)
        };
        next.unwrap_or(at)
    }

    /// 确认选中的行。
    pub(in crate::app) fn branch_picker_accept(&mut self) {
        let row = (self.overlays.picker.branch_rows.as_ref())
            .and_then(|rows| rows.get(self.overlays.picker.selected))
            .cloned();
        // 日志的分支筛选：选分支、标签只看它的提交，「全部分支」看全部；组、目录照常展开、收起。
        if self.picker_filters_log() {
            let chosen = match &row {
                Some(BranchRow::All) => Some(None),
                Some(BranchRow::Branch { index, .. }) => {
                    self.git.branches.get(*index).map(|b| Some(b.name.clone()))
                }
                Some(BranchRow::Tag(index)) => self.git.tags.get(*index).map(|t| Some(t.clone())),
                _ => None,
            };
            if let Some(branch) = chosen {
                self.close_picker();
                self.change_log_query(|q| q.branches = branch.into_iter().collect());
                return;
            }
            if matches!(row, Some(BranchRow::Create(_))) {
                return;
            }
        }
        match row {
            Some(BranchRow::Locals { recent, .. }) => {
                let (remote, path) = BranchRow::locals_key(recent);
                self.toggle_branch_folder(remote, path);
            }
            Some(BranchRow::Remotes { .. }) => self.toggle_branch_folder(true, String::new()),
            Some(BranchRow::Tags { .. }) => self.toggle_branch_folder(false, String::new()),
            Some(BranchRow::Tag(index)) => self.run_picker_tag(None, index),
            Some(BranchRow::Folder { remote, path, .. }) => self.toggle_branch_folder(remote, path),
            Some(BranchRow::Branch { index, .. }) => self.pick_branch(index),
            Some(BranchRow::Create(name)) => self.create_branch_from_picker(name, None),
            Some(BranchRow::All) | None => {}
        }
    }

    /// 选了第 `index` 个分支：当前分支只关闭选择器；被别的工作树占着时切到那个工作树；不能签出
    /// 的（工作树目录已不在）打开操作菜单，灰掉的菜单项说明原因；有未提交的更改时先询问。
    pub(in crate::app) fn pick_branch(&mut self, index: usize) {
        let Some(branch) = self.git.branches.get(index).cloned() else {
            return;
        };
        let local = local_name(&branch);
        if branch.current {
            self.close_picker();
            return;
        }
        if self.git.stale_branches().contains(&local) {
            self.open_branch_row_menu(MenuTarget::BranchOps(index));
            return;
        }
        if let Some(i) = self.git.worktree_holding(local) {
            self.close_picker();
            self.open_worktree_asking(i);
            return;
        }
        let changes = self.git.status.as_ref().map_or(0, tracked_changes);
        self.close_picker();
        // 未跟踪的文件不计入：它们同样可能使 git 拒绝切换，由 `switch_branch` 处理。
        if changes == 0 {
            self.switch_branch(branch, false);
            return;
        }
        let head = (self.git.branches.iter())
            .find(|b| b.current)
            .map_or("HEAD", |b| b.name.as_str());
        let message = t!("git-switch-dirty", branch = local);
        let detail = t!("git-switch-dirty-detail", current = head, count = changes);
        let buttons = vec![
            AlertButton::new(t!("git-switch-carry")),
            AlertButton::new(t!("git-switch-stash")),
            AlertButton::cancel(t!("common-cancel")),
        ];
        self.show_alert(&message, &detail, buttons, None, move |choice, _| {
            (choice < 2).then(|| UserEvent::GitSwitchBranch(branch.clone(), choice == 1))
        });
    }

    /// 切换到 `branch`；`stash` 时先储藏未提交的更改，否则更改留在工作区。
    pub(in crate::app) fn switch_branch(&mut self, branch: Branch, stash: bool) {
        if stash {
            self.stash_then_checkout(branch);
        } else {
            self.carry_then_checkout(branch);
        }
    }

    fn toggle_branch_folder(&mut self, remote: bool, path: String) {
        let before = self.picker_fold_before();
        let key = (remote, path);
        if !self.git.branch_folders.remove(&key) {
            self.git.branch_folders.insert(key);
        }
        let branches = self.git.branches.clone();
        self.keep_branch_selection(&branches);
        self.start_picker_fold(before);
    }

    /// 处理没有查询时的左右键：右键展开目录、打开分支的操作菜单，左键收起目录、回到上一级目录。
    /// 返回是否已处理；有查询时左右键移动输入框的光标。
    pub(in crate::app) fn branch_picker_side(&mut self, right: bool) -> bool {
        if !self.picker_lists_branches() || !self.overlays.picker.input.text().is_empty() {
            return false;
        }
        let Some(row) = (self.overlays.picker.branch_rows.as_ref())
            .and_then(|rows| rows.get(self.overlays.picker.selected))
            .cloned()
        else {
            return false;
        };
        match (row, right) {
            (BranchRow::Locals { recent, open, .. }, _) if open != right => {
                let (remote, path) = BranchRow::locals_key(recent);
                self.toggle_branch_folder(remote, path);
            }
            (BranchRow::Remotes { open, .. }, _) if open != right => {
                self.toggle_branch_folder(true, String::new());
            }
            (BranchRow::Tags { open, .. }, _) if open != right => {
                self.toggle_branch_folder(false, String::new());
            }
            (BranchRow::Tag(index), true) => {
                self.open_branch_row_menu(MenuTarget::TagOps(index));
            }
            (
                BranchRow::Folder {
                    remote, path, open, ..
                },
                _,
            ) if open != right => self.toggle_branch_folder(remote, path),
            (BranchRow::Branch { index, .. }, true) => {
                self.open_branch_row_menu(MenuTarget::BranchOps(index));
            }
            (
                BranchRow::Folder { .. }
                | BranchRow::Locals { .. }
                | BranchRow::Remotes { .. }
                | BranchRow::Tags { .. },
                true,
            ) => {
                self.overlays.picker.selected = self.branch_picker_step(true);
                self.overlays.picker.default = None;
                self.picker_reveal();
                self.request_redraw();
            }
            (_, false) => self.select_parent_branch_folder(),
            _ => {}
        }
        true
    }

    /// 选中当前行所在的目录；已经在最外层时不动。
    fn select_parent_branch_folder(&mut self) {
        let rows = self
            .overlays
            .picker
            .branch_rows
            .as_deref()
            .unwrap_or_default();
        let depth_of = |row: &BranchRow| match row {
            BranchRow::Folder { depth, .. } | BranchRow::Branch { depth, .. } => Some(*depth),
            _ => None,
        };
        let Some(depth) = rows.get(self.overlays.picker.selected).and_then(depth_of) else {
            return;
        };
        // 远程分支一组没有层数，`None` 比任何层数都小：最外层的远程分支回到这一组。
        let parent = (0..self.overlays.picker.selected).rev().find(|&i| {
            matches!(
                rows[i],
                BranchRow::Folder { .. } | BranchRow::Remotes { .. } | BranchRow::Locals { .. }
            ) && depth_of(&rows[i]) < Some(depth)
        });
        if let Some(parent) = parent {
            self.overlays.picker.selected = parent;
            self.overlays.picker.default = None;
            self.picker_reveal();
            self.request_redraw();
        }
    }

    /// 对第 `index` 个标签执行 `op`：`None` 是签出它指向的提交（先确认），其余见 [`TagOp`]。
    pub(in crate::app) fn run_picker_tag(&mut self, op: Option<TagOp>, index: usize) {
        let Some(name) = self.git.tags.get(index).cloned() else {
            return;
        };
        self.close_picker();
        match op {
            Some(op) => self.run_tag_op(op, name),
            None => self.confirm_log(LogConfirmed::CheckoutTag(name)),
        }
    }

    /// 确认位置选择器中选中的行：切换到这个位置。
    pub(in crate::app) fn location_picker_accept(&mut self) {
        let rows = self.overlays.picker.location_rows.as_ref();
        let place = rows
            .and_then(|rows| rows.get(self.overlays.picker.selected))
            .cloned();
        if let Some(place) = place {
            self.go_to_place(&place);
        }
        self.request_redraw();
    }

    /// 切换到位置 `place`：打开的目录里的仓库只换当前仓库，其余的（别处的工作树）在当前窗口打开
    /// 那个目录，文件树和编辑区跟着换。
    pub(in crate::app) fn go_to_place(&mut self, place: &Place) {
        // 和失效的分支一样：点了弹出它能做的事。
        if place.missing {
            if place.worktree.is_some() {
                self.open_location_row_menu(self.overlays.picker.selected, place);
            } else {
                self.flash(t!("git-worktree-missing"));
            }
            return;
        }
        self.close_picker();
        if place.current {
            return;
        }
        if let Some(i) = self.git.repositories.iter().position(|r| *r == place.path) {
            let scroll = self.git.commit.scroll;
            self.git_switch_repository(i);
            self.keep_commit_scroll(scroll);
        } else if let Some(i) = place.worktree {
            self.open_worktree_asking(i);
        } else {
            self.switch_folder(&place.path);
        }
    }

    /// 在选中行的右端弹出菜单 `target`。
    pub(in crate::app) fn open_branch_row_menu(&mut self, target: MenuTarget) {
        let parts = crate::app::workspace::picker::picker_parts(self, self.layout.picker);
        let row = parts.row(self.overlays.picker.selected, self.overlays.picker.scroll);
        self.open_context_menu(target, (row.right() - 14.0, row.bottom()));
    }

    /// 关闭选择器，从 `from`（`None` 是当前分支）新建分支 `name` 并切换过去。
    fn create_branch_from_picker(&mut self, name: String, from: Option<String>) {
        self.close_picker();
        self.git_new_branch(name, from, true);
    }
}

#[cfg(test)]
mod tests;
