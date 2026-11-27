//! Git 后台操作执行引擎与共享基础类型：串行执行会改仓库的命令、转发凭据询问、准备历史差异，
//! 以及分支/标签/储藏操作和分支列表行的纯数据定义。

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use winc_editor::diff_view::{DiffSide, DiffView};
use winc_editor::encoding::decode_unknown;
use winc_editor::git::{
    Branch, BranchDeleted, Carried, CommitFile, Git, Operation, Remote, Stash, WorktreeCheck,
};
use winc_editor::shell::Cancel;
use winc_editor::{LARGE_FILE, looks_binary};
use winc_i18n::t;

use super::unusable;
use crate::app::NoticeKind;
use crate::app::animation::{Clock, TREE_ANIMATION};
use crate::app::input::context_menu::{MenuCommand, MenuEntry};
use crate::app::input::palette::rate;
use crate::app::{App, Focus, UserEvent};
use crate::askpass::{Listener, Question};
use crate::winc::WindowProxy;

/// 推送的确认框里最多列出几个提交的标题：确认框不能滚动，一屏之内读得完；超出的写明个数，
/// 全部提交在「查看提交」打开的日志里。
pub(crate) const PUSH_PREVIEW: usize = 8;

/// 提交成功后按钮显示「✓ 已提交」的时长（经验值）。
///
/// 短于状态栏提示的显示时长，足以让用户确认提交已完成。
pub(in crate::app) const COMMITTED_SHOWN: Duration = Duration::from_millis(1600);

/// 最近使用的分支最多列出几个（个）：加上标题不超过选择面板一半的行数，本地分支仍在首屏。
pub(super) const RECENT_BRANCHES: usize = 5;
/// [`rate`] 给字符分散命中的最低得分：低于它的是连续命中。
const SCATTERED: u32 = 1000;
/// 选择面板最多同时显示的行数（行）：分支常常成组展开，比文件列表多留一些行。
pub(super) const GIT_PICKER_ROWS: usize = 16;

/// 需要确认的推送（见 [`App::ask_push`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PushIntent {
    /// 把还没有上游的当前分支发布到这个远程，并设为上游。
    Publish(String),
    /// 把当前分支推送到上游。
    Push,
    /// 同步：拉取上游的新提交，再推送本地的提交。
    Sync,
}

/// 对一个分支做的事。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BranchOp {
    /// 签出该分支；远程分支签出成同名的本地分支并跟踪它。
    Checkout,
    /// 在新工作树中签出（`App::new_worktree_for`），建好在新窗口打开。
    NewWorktree,
    /// 从该分支新建分支（输入名字）。
    NewFrom,
    /// 合并到当前分支。
    Merge,
    /// 把当前分支变基到该分支上。
    Rebase,
    /// 重命名（输入新名字），只用于本地分支。
    Rename,
    /// 删除（先确认；未合并的再确认一次才强制删除）。
    Delete,
    /// 和当前分支比较：打开日志，列出从当前分支到它改了的文件。
    Compare,
}

impl BranchOp {
    /// 操作菜单的各组，顺序就是菜单里的顺序：切换和新建、比较、合到当前分支、删除。
    pub(super) const GROUPS: [&[BranchOp]; 4] = [
        &[BranchOp::Checkout, BranchOp::NewWorktree, BranchOp::NewFrom],
        &[BranchOp::Compare],
        &[BranchOp::Merge, BranchOp::Rebase],
        &[BranchOp::Rename, BranchOp::Delete],
    ];

    /// 菜单里的文字：不写分支名。菜单是从这个分支的那一行弹出来的，另一边总是当前分支。
    pub fn label(self) -> &'static str {
        match self {
            BranchOp::Checkout => t!("git-branch-checkout"),
            BranchOp::NewWorktree => t!("git-branch-checkout-new-worktree"),
            BranchOp::NewFrom => t!("git-branch-new-from"),
            BranchOp::Merge => t!("git-branch-merge-into-current"),
            BranchOp::Rebase => t!("git-branch-rebase-current-onto"),
            BranchOp::Rename => t!("git-branch-rename"),
            BranchOp::Delete => t!("git-branch-delete"),
            BranchOp::Compare => t!("git-branch-compare-with-current"),
        }
    }
}

/// 对日志里一个标签做的事。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TagOp {
    /// 从该标签新建分支（输入名字）。
    NewBranch,
    /// 把该标签指向的提交合并到当前分支。
    Merge,
    /// 推送到远程仓库。
    Push,
    /// 删除本地标签（先确认）。
    Delete,
    /// 从远程仓库删除（先确认）。
    DeleteRemote,
}

impl TagOp {
    /// 全部操作，顺序就是子菜单里的顺序。
    pub const ALL: [TagOp; 5] = [
        TagOp::NewBranch,
        TagOp::Merge,
        TagOp::Push,
        TagOp::Delete,
        TagOp::DeleteRemote,
    ];

    /// 子菜单里的文字。
    pub fn label(self) -> &'static str {
        match self {
            TagOp::NewBranch => t!("git-branch-new-from"),
            TagOp::Merge => t!("git-branch-merge-into-current"),
            TagOp::Push => t!("git-tag-push"),
            TagOp::Delete => t!("git-tag-delete"),
            TagOp::DeleteRemote => t!("git-tag-delete-remote"),
        }
    }
}

/// 对一个远程仓库做的事。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RemoteOp {
    /// 修改地址。
    Edit,
    /// 移除（先确认）。
    Remove,
}

impl RemoteOp {
    /// 子菜单里的文字。
    pub fn label(self) -> &'static str {
        match self {
            RemoteOp::Edit => t!("git-remote-edit"),
            RemoteOp::Remove => t!("git-remote-remove"),
        }
    }
}

/// 对一条储藏做的事。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StashOp {
    /// 应用到工作区，储藏留着（`stash apply`）。
    Apply,
    /// 应用到工作区并删掉这条储藏（`stash pop`）。
    Pop,
    /// 删掉这条储藏（`stash drop`，先确认）。
    Drop,
}

impl StashOp {
    /// 全部操作，顺序就是子菜单里的顺序。
    pub const ALL: [StashOp; 3] = [StashOp::Apply, StashOp::Pop, StashOp::Drop];

    /// 子菜单里的文字。
    pub fn label(self) -> &'static str {
        match self {
            StashOp::Apply => t!("git-stash-apply"),
            StashOp::Pop => t!("git-stash-pop"),
            StashOp::Drop => t!("git-stash-drop"),
        }
    }
}

/// 分支、位置选择器从哪儿展开。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitAnchor {
    /// 提交视图的分支行下方。
    Branch,
    /// 提交视图的位置行下方。
    Location,
    /// 状态栏的分支名上方：侧栏收起、看着文件视图时也能切换分支和同步。
    Status,
    /// Git 日志的分支筛选按钮下方。
    Log,
}

/// 选择面板里除了各行以外可以点的地方。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerPart {
    /// 仅搜索笔记标题的开关。
    NoteTitles,
    /// 笔记预览开关。
    NotePreview,
    /// 关闭笔记搜索。
    CloseNoteSearch,
    /// 属性日历控件的部件。
    PropertyCalendar(crate::app::preview::markdown::properties::popup::CalendarPart),
    /// 第几行行尾的「⋯」。
    More(usize),
    /// 搜索框下面的「拉取」；按住 ⌥ 时是拉取并变基。
    Pull,
    /// 搜索框下面的「推送」。
    Push,
    /// 搜索框下面的「获取」。
    Fetch,
    /// 底栏的「新建分支」。
    NewBranch,
    /// 底栏的「新建工作树」。
    NewWorktree,
    /// 最近目录列表第几行行尾的「✕」：从最近列表中移除该目录。
    ForgetFolder(usize),
}

/// 检查完、等着确认移除的工作树。
pub(crate) struct RemoveCheck {
    /// 工作树的目录。
    path: PathBuf,
    /// 签出的分支。
    branch: Option<String>,
    /// 检查结果：没提交的改动数、未跟踪的文件数、游离的提交数，以及分支是否已合并。
    check: WorktreeCheck,
}

impl RemoveCheck {
    /// 创建待确认移除的工作树检查结果。
    pub(super) fn new(path: PathBuf, branch: Option<String>, check: WorktreeCheck) -> RemoveCheck {
        RemoveCheck {
            path,
            branch,
            check,
        }
    }

    /// 拆出目录、分支名和检查结果。
    pub(super) fn into_parts(self) -> (PathBuf, Option<String>, WorktreeCheck) {
        (self.path, self.branch, self.check)
    }
}

/// 输入请求的来源：决定请求何时作废，以及用户取消后由谁提示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum Asker {
    /// 第几个 Git 操作。
    Git(u64),
}

impl Asker {
    /// 开始接收第 `id` 个操作的输入请求：写入 `questions` 并唤醒界面线程。
    ///
    /// `remember` 表示设置允许记住密码；系统没有可用的凭据存储时不提供。在后台线程中调用。
    pub(super) fn listen(
        proxy: WindowProxy,
        questions: Sender<(u64, Question)>,
        id: u64,
        remember: bool,
    ) -> Option<Listener> {
        Listener::bind(
            remember && crate::credentials::available(),
            move |question| {
                let _ = questions.send((id, question));
                let _ = proxy.send_event(UserEvent::GitQuestion);
            },
        )
    }

    /// 开始只提供已保存凭据的端点，用于无人值守的操作：输入请求一律按取消答复。
    ///
    /// 在后台线程中调用。
    pub(super) fn listen_unattended(remember: bool) -> Option<Listener> {
        (remember && crate::credentials::available())
            .then(|| Listener::bind(true, drop))
            .flatten()
    }

    /// 在后台线程中删除全部已保存的凭据。
    pub(in crate::app) fn forget_credentials() {
        let _ = std::thread::Builder::new()
            .name("winc-credentials".into())
            .spawn(|| {
                crate::credentials::forget_all();
            });
    }
}

/// 日志中需要确认后才执行的提交操作。
///
/// 确认框由系统绘制，确认后经 `UserEvent` 返回。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogConfirmed {
    /// 签出提交本身，进入分离的 `HEAD`。
    ///
    /// 参数是完整的提交号。
    Detach(String),
    /// 签出一个标签指向的提交，进入分离的 `HEAD`；参数是标签名。
    CheckoutTag(String),
    /// 将当前分支硬重置到该提交。
    ResetHard(String),
    /// 删除一条储藏；执行时按提交号查找（[`winc_editor::git::Git::stash`]）。
    ///
    /// 使用 `Box`：储藏包含父提交、时间和说明，直接内联会增大发往事件循环的每个事件的大小。
    DropStash(Box<Stash>),
    /// 删除一个本地分支；第二项为真时连同未合并的提交一并删除（[`App::run_delete_branch`]）。
    DeleteBranch(String, bool),
    /// 删除一个标签。
    DeleteTag(String, winc_editor::git::TagTarget),
    /// 移除一个远程仓库。
    RemoveRemote(String),
}

/// 一次操作成功之后要做的事。
pub(super) enum Then {
    /// 没有了：说一声做完了。
    Nothing,
    /// 工作树变了：重新取工作树列表。
    Refresh,
    /// 新建了工作树或克隆了仓库：重新取列表，打开它；`true` 是在新窗口打开。
    Open(PathBuf, bool),
    /// 当前目录成为了仓库：重新查找仓库。
    Rescan,
    /// 读取好了交互式变基的范围：打开对话框，不说做完了。
    RebasePlan(Box<winc_editor::git::RebasePlan>),
    /// 检查完要移除的工作树：确认以后移除（[`App::confirm_remove_worktree`]），不说做完了。
    ConfirmRemove(RemoveCheck),
    /// 同步需要强制推送：确认以后推送（[`App::confirm_force_push`]），不说做完了；内容是上游将被
    /// 替换的提交数。
    ConfirmForcePush(usize),
    /// 操作已完成，但有需要用户处理的情况：显示这条提示，不说做完了。
    Warn(String),
    /// 要删除的分支未完全合并：确认以后强制删除（[`App::confirm_force_delete_branch`]），不说
    /// 做完了；内容是分支名。
    ConfirmForceDelete(String),
}

/// 一次后台 Git 操作的完成结果：成功时带简短摘要和后续动作，失败时带说明。
pub(super) type DoneOutcome = Result<(String, Then), String>;

/// 一次操作的结果。
pub(super) struct Done {
    /// 第几个操作（[`BranchState::started`]）：换目录以后又起了新操作时，认得出这是之前的。
    id: u64,
    /// 在哪个仓库里做的；操作开始时当前目录不是仓库时为 `None`。
    root: Option<PathBuf>,
    /// 做的是什么（「推送」「签出「dev」」），用在提示里。
    what: String,
    result: DoneOutcome,
}

impl Done {
    /// 拆出操作编号、仓库根目录、名称和结果。
    pub(super) fn into_parts(self) -> (u64, Option<PathBuf>, String, DoneOutcome) {
        (self.id, self.root, self.what, self.result)
    }
}

/// 输入请求从后台线程到界面线程的通道，每项是所属操作的编号和请求。
type QuestionChannel = (Sender<(u64, Question)>, Receiver<(u64, Question)>);

/// 分支操作的状态。
pub(crate) struct BranchState {
    /// 正在做的事；状态栏的分支名后面写着它。
    pub running: Option<String>,
    /// 上一个操作成功后，同步控件显示对勾的时钟。
    pub finished: Option<Clock>,
    /// 起过几个操作；正在做的是第几个。
    pub(in crate::app) started: u64,
    results: (Sender<Done>, Receiver<Done>),
    /// 后台送达的输入请求及其所属操作的编号。
    pub(super) questions: QuestionChannel,
    /// 对话框正在处理的请求：所属操作的编号和答复通道；丢弃通道表示取消。
    pub(in crate::app) asking: Option<(Asker, Sender<String>)>,
    /// 正在询问的登录请求的「记住密码」标记；该请求不提供此选项时为 `None`。
    pub(in crate::app) remember: Option<Arc<AtomicBool>>,
    /// 用户是否取消了当前操作的输入请求；为真时该操作的失败不按错误提示。
    pub(in crate::app) declined: bool,
    /// 登录对话框中填写的密码及其远程地址，用于答复随后的密码请求，操作结束时清除。
    pub(in crate::app) password: Option<(String, String)>,
}

impl BranchState {
    /// 取出一条已完成的后台操作结果。
    pub(super) fn try_recv(&self) -> Option<Done> {
        self.results.1.try_recv().ok()
    }

    /// 测试用：向操作结果通道送入一条完成记录。
    #[cfg(all(test, unix))]
    pub(super) fn send_test_done(&self, root: PathBuf, what: String) {
        let _ = self.results.0.send(Done {
            id: 0,
            root: Some(root),
            what,
            result: Ok((String::new(), Then::Nothing)),
        });
    }
}

impl Default for BranchState {
    fn default() -> Self {
        BranchState {
            running: None,
            finished: None,
            started: 0,
            results: channel(),
            questions: channel(),
            asking: None,
            remember: None,
            declined: false,
            password: None,
        }
    }
}

/// 分支选择器的一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchRow {
    /// 清除日志的分支筛选。
    All,
    /// 可以收起的「最近」或「本地」一组，默认展开。
    Locals {
        /// 是否是「最近」一组。
        recent: bool,
        /// 这一组的分支数。
        count: usize,
        /// 是否展开。
        open: bool,
    },
    /// 可以展开的「远程分支」一组。
    Remotes {
        /// 这一组的分支数。
        count: usize,
        /// 是否展开。
        open: bool,
    },
    /// 分支名按 `/` 分出的一层目录。
    Folder {
        /// 是否在远程分支一组。
        remote: bool,
        /// 从分支名开头到这一层的路径。
        path: String,
        /// 显示的名字：只有一个子目录的目录与子目录合成一行。
        label: String,
        /// 缩进的层数。
        depth: usize,
        /// 目录下的分支数。
        count: usize,
        /// 是否展开。
        open: bool,
    },
    /// 可以展开的「标签」一组，排在分支之后。
    Tags {
        /// 这一组的标签数。
        count: usize,
        /// 是否展开。
        open: bool,
    },
    /// 一个标签：[`GitState::tags`](crate::app::git::GitState::tags) 的下标。
    Tag(usize),
    /// 一个分支。
    Branch {
        /// [`GitState::branches`](crate::app::git::GitState::branches) 的下标。
        index: usize,
        /// 缩进的层数。
        depth: usize,
        /// 显示的名字：去掉上级目录。
        label: String,
    },
    /// 新建名字是查询文字的分支。
    Create(String),
}

impl BranchRow {
    /// 两行是否是同一项：展开状态变化、列表刷新以后据此留住选中项。
    ///
    /// 分支的下标按各自所在的列表解释：`self` 是 `old` 的，`other` 是 `new` 的。
    pub(super) fn same(&self, other: &BranchRow, old: &[Branch], new: &[Branch]) -> bool {
        match (self, other) {
            (BranchRow::All, BranchRow::All) => true,
            (BranchRow::Remotes { .. }, BranchRow::Remotes { .. }) => true,
            (BranchRow::Locals { recent: a, .. }, BranchRow::Locals { recent: b, .. }) => a == b,
            (BranchRow::Tags { .. }, BranchRow::Tags { .. }) => true,
            // 标签只在列表末尾增减，名字不随刷新移动到别的下标的情况不多；下标相同即视为同一项。
            (BranchRow::Tag(index), BranchRow::Tag(i)) => index == i,
            (
                BranchRow::Folder { remote, path, .. },
                BranchRow::Folder {
                    remote: r, path: p, ..
                },
            ) => remote == r && path == p,
            (
                BranchRow::Branch { index, depth, .. },
                BranchRow::Branch {
                    index: i, depth: d, ..
                },
            ) => depth == d && old.get(*index).map(|b| &b.name) == new.get(*i).map(|b| &b.name),
            _ => false,
        }
    }

    /// 去掉「最近」一组（标题和其中的分支）：日志里按分支筛选时用不上捷径，同一个分支列两遍、
    /// 筛选着的分支亮两处，反而看不清（日志左侧的分支面板和日志的分支筛选）。
    pub fn without_recent(rows: Vec<BranchRow>) -> Vec<BranchRow> {
        let mut recent = false;
        let kept = rows.into_iter().filter(|row| {
            match row {
                BranchRow::Locals { recent: r, .. } => recent = *r,
                BranchRow::Remotes { .. } | BranchRow::Tags { .. } => recent = false,
                _ => {}
            }
            !recent
        });
        kept.collect()
    }

    /// 「最近」（`recent` 为真）或「本地」一组在 [`BranchList::toggled`] 中的记法：记作本地的
    /// 一项，路径以分支名中不会出现的空字符开头，不与目录的路径相同。
    pub fn locals_key(recent: bool) -> (bool, String) {
        let name = if recent { "\0recent" } else { "\0local" };
        (false, name.to_owned())
    }

    /// 分支排序用的值，大的在前：当前分支最前，本地分支按最后一次提交的时间，远程分支都一样
    /// （再按名字）。
    pub(in crate::app) fn rank(branch: &Branch) -> i64 {
        match (branch.remote, branch.current) {
            (true, _) => 0,
            (false, true) => i64::MAX,
            (false, false) => branch.time,
        }
    }
}

/// 排列分支选择器时用到的内容。
pub struct BranchList<'a> {
    /// 全部分支。
    pub branches: &'a [Branch],
    /// 最近切换到过的分支名，最近的在前。
    pub recent: &'a [String],
    /// `needle` 处理过的查询。
    pub query: &'a [char],
    /// 原样的查询文字，新建分支时用作名字。
    pub text: &'a str,
    /// 改变过默认展开状态的目录：（是否远程，目录路径）；远程分支一组的路径是空的。标签一组
    /// 记作本地、路径为空的一项：本地分支的目录路径不会为空。
    pub toggled: &'a HashSet<(bool, String)>,
    /// 各个标签的名字，最近创建的在前。
    pub tags: &'a [String],
}

impl BranchList<'_> {
    /// 排出分支选择器的各行。
    pub fn rows(&self) -> Vec<BranchRow> {
        let (branches, query) = (self.branches, self.query);
        let searching = !query.is_empty();
        let hit = branch_hit(branches, query);
        let current = branches.iter().find(|b| b.current).map(|b| b.name.as_str());
        let mut rows = Vec::new();
        // 本地分支一屏放得下时都看得见，单列最近使用只是重复。
        let many = branches.iter().filter(|b| !b.remote).count() > GIT_PICKER_ROWS;
        let latest = if many && !searching {
            quick_branches(branches, self.recent)
        } else {
            Vec::new()
        };
        // 「最近」「本地」两组默认展开，点标题行收起；搜索时都展开。
        let shown =
            |recent: bool| searching || !self.toggled.contains(&BranchRow::locals_key(recent));
        if !latest.is_empty() {
            let open = shown(true);
            rows.push(BranchRow::Locals {
                recent: true,
                count: latest.len(),
                open,
            });
            let latest = latest.iter().filter(|_| open);
            rows.extend(latest.map(|&index| BranchRow::Branch {
                index,
                depth: 0,
                label: branches[index].name.clone(),
            }));
        }
        let items = |remote: bool| -> Vec<(usize, &str)> {
            branches
                .iter()
                .enumerate()
                .filter(|(_, b)| b.remote == remote && hit(b))
                .map(|(i, b)| (i, b.name.as_str()))
                .collect()
        };
        let tree = |remote| Tree {
            branches,
            remote,
            searching,
            current,
            toggled: self.toggled,
        };
        // 各组都列全：「最近」只是捷径，其中的分支在「本地」里照样有；「远程」不因为已有本地分支
        // 而少列，否则看起来像远程上没有这个分支。
        let local = items(false);
        if !local.is_empty() {
            let open = shown(false);
            rows.push(BranchRow::Locals {
                recent: false,
                count: local.len(),
                open,
            });
            if open {
                tree(false).push(local, "", 0, &mut rows);
            }
        }
        let remote = items(true);
        if !remote.is_empty() {
            let flipped = self.toggled.contains(&(true, String::new()));
            let open = searching || flipped;
            rows.push(BranchRow::Remotes {
                count: remote.len(),
                open,
            });
            if open {
                // 和本地分支同一层：这一行只是分组标题，名字的列上下对齐。
                let remote = strip_single_remote(remote);
                tree(true).push(remote, "", 0, &mut rows);
            }
        }
        // 标签一组平时收起：标签可以很多，展开或搜索时才列出。
        let tags: Vec<usize> = (self.tags.iter().enumerate())
            .filter(|(_, tag)| !searching || rate(query, tag).is_some_and(|s| s < SCATTERED))
            .map(|(index, _)| index)
            .collect();
        if !tags.is_empty() {
            let open = searching || self.toggled.contains(&(false, String::new()));
            rows.push(BranchRow::Tags {
                count: tags.len(),
                open,
            });
            if open {
                rows.extend(tags.into_iter().map(BranchRow::Tag));
            }
        }
        let name = self.text.trim();
        let exists = branches.iter().any(|b| !b.remote && b.name == name);
        if searching && !exists {
            rows.push(BranchRow::Create(
                name.split_whitespace().collect::<Vec<_>>().join("-"),
            ));
        }
        rows
    }
}

/// 返回判断分支是否命中查询 `query` 的函数；没有查询时全部命中。
///
/// 有连续命中的分支时不计字符分散命中的，否则短查询会带出大量无关分支。
fn branch_hit<'a>(branches: &[Branch], query: &'a [char]) -> impl Fn(&Branch) -> bool + 'a {
    let searching = !query.is_empty();
    let score = move |branch: &Branch| rate(query, &branch.name);
    let contiguous = searching
        && branches
            .iter()
            .any(|b| score(b).is_some_and(|s| s < SCATTERED));
    move |branch| !searching || score(branch).is_some_and(|s| !contiguous || s < SCATTERED)
}

/// 只有一个远程时去掉分支名里的远程名：少一层没有信息量的目录。
fn strip_single_remote(items: Vec<(usize, &str)>) -> Vec<(usize, &str)> {
    let first = items
        .first()
        .and_then(|(_, n)| n.split_once('/'))
        .map(|(r, _)| r);
    let single = first.is_some_and(|remote| {
        (items.iter()).all(|(_, n)| n.split_once('/').is_some_and(|(r, _)| r == remote))
    });
    if !single {
        return items;
    }
    items
        .into_iter()
        .map(|(i, name)| (i, name.split_once('/').map_or(name, |(_, n)| n)))
        .collect()
}

/// 「最近使用」里的本地分支：默认分支（`main`，没有时 `master`）排最前，之后是最近切换到过的；
/// 不含当前分支。
fn quick_branches(branches: &[Branch], recent: &[String]) -> Vec<usize> {
    let default = ["main", "master"]
        .into_iter()
        .find(|name| branches.iter().any(|b| !b.remote && b.name == *name));
    let names = default.into_iter().chain(recent.iter().map(String::as_str));
    let mut out = Vec::new();
    for name in names {
        let found = branches
            .iter()
            .position(|b| !b.remote && !b.current && b.name == name);
        if let Some(index) = found
            && !out.contains(&index)
        {
            out.push(index);
        }
    }
    out.truncate(RECENT_BRANCHES);
    out
}

/// 排一组分支的树时不变的条件。
struct Tree<'a> {
    branches: &'a [Branch],
    remote: bool,
    searching: bool,
    current: Option<&'a str>,
    toggled: &'a HashSet<(bool, String)>,
}

impl Tree<'_> {
    /// 把 `items`（下标、去掉 `prefix` 的分支名）排成一层。
    fn push(
        &self,
        items: Vec<(usize, &str)>,
        prefix: &str,
        depth: usize,
        rows: &mut Vec<BranchRow>,
    ) {
        let mut folders: BTreeMap<&str, Vec<(usize, &str)>> = BTreeMap::new();
        let mut leaves = Vec::new();
        for (index, name) in items {
            match name.split_once('/') {
                Some((folder, rest)) => folders.entry(folder).or_default().push((index, rest)),
                None => leaves.push((index, name)),
            }
        }
        let time = |index: usize| self.branches.get(index).map_or(0, BranchRow::rank);
        let mut nodes: Vec<_> = (folders.into_iter())
            .map(|(folder, sub)| {
                let newest = sub.iter().map(|&(i, _)| time(i)).max().unwrap_or(0);
                (newest, false, folder, sub)
            })
            .chain(
                leaves
                    .into_iter()
                    .map(|(i, name)| (time(i), true, name, vec![(i, name)])),
            )
            .collect();
        nodes.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(b.2)));
        for (_, leaf, folder, mut sub) in nodes {
            if leaf {
                let (index, name) = sub[0];
                rows.push(BranchRow::Branch {
                    index,
                    depth,
                    label: name.to_owned(),
                });
                continue;
            }
            // 只有一个子目录、没有分支：和子目录合成一行。
            let mut label = folder.to_owned();
            while let Some(next) = only_folder(&sub) {
                label.push('/');
                label.push_str(next);
                for (_, rest) in &mut sub {
                    *rest = &rest[next.len() + 1..];
                }
            }
            let path = format!("{prefix}{label}");
            // 默认只展开当前分支所在的目录，列表短，一眼能看全。
            let inside = !self.remote
                && self
                    .current
                    .is_some_and(|name| name.starts_with(&format!("{path}/")));
            let flipped = self.toggled.contains(&(self.remote, path.clone()));
            let open = self.searching || inside != flipped;
            rows.push(BranchRow::Folder {
                remote: self.remote,
                path: path.clone(),
                label,
                depth,
                count: sub.len(),
                open,
            });
            if open {
                self.push(sub, &format!("{path}/"), depth + 1, rows);
            }
        }
    }
}

/// 这些分支都在同一个子目录里时返回它的名字。
fn only_folder<'a>(items: &[(usize, &'a str)]) -> Option<&'a str> {
    let (first, _) = items.first()?.1.split_once('/')?;
    items
        .iter()
        .all(|(_, name)| {
            name.split_once('/')
                .is_some_and(|(folder, _)| folder == first)
        })
        .then_some(first)
}

/// 签出分支 `branch` 得到的本地分支名：远程分支去掉远程名（`origin/dev` → `dev`）。
pub(crate) fn local_name(branch: &Branch) -> &str {
    if branch.remote {
        branch
            .name
            .split_once('/')
            .map_or(branch.name.as_str(), |(_, l)| l)
    } else {
        &branch.name
    }
}

/// 储藏在菜单里的名字：git 记下的说明去掉「On main: 」这样的开头。
///
/// 超过 40 个字符的截短并加省略号（经验值：菜单一行放得下的长度）。
pub(crate) fn stash_title(stash: &Stash) -> String {
    let message = stash
        .message
        .split_once(": ")
        .map_or(stash.message.as_str(), |(_, rest)| rest);
    let mut title: String = message.chars().take(40).collect();
    if message.chars().count() > 40 {
        title.push('…');
    }
    title
}

/// 提交视图「⋯」里的 Git 操作：进行中的合并、变基的继续、跳过和中止，储藏更改，远程仓库。
pub(crate) fn branch_menu(
    remotes: &[Remote],
    operation: Option<Operation>,
    busy: bool,
) -> Vec<MenuEntry> {
    use MenuCommand::*;
    let mut entries = Vec::new();
    // 合并、变基停下来等着处理冲突：最上面是继续和中止。
    if let Some(operation) = operation {
        entries.push(MenuEntry::Item(GitContinue, !busy));
        // 合并只有一个提交，没有可以跳过的。
        if operation != Operation::Merge {
            entries.push(MenuEntry::Item(GitSkip, !busy));
        }
        entries.push(MenuEntry::Item(GitAbort, !busy));
        entries.push(MenuEntry::Separator);
    }
    entries.push(MenuEntry::Item(GitStash, !busy));
    entries.push(MenuEntry::Separator);
    // 远程仓库：每个一个子菜单（修改地址、移除），最后是添加。
    let mut items: Vec<MenuEntry> = (remotes.iter().enumerate())
        .map(|(i, remote)| {
            let ops = [RemoteOp::Edit, RemoteOp::Remove]
                .into_iter()
                .map(|op| MenuEntry::Item(GitRemote(op, i), !busy))
                .collect();
            MenuEntry::Submenu(remote.name.clone(), ops)
        })
        .collect();
    if !items.is_empty() {
        items.push(MenuEntry::Separator);
    }
    items.push(MenuEntry::Item(GitAddRemote, !busy));
    entries.push(MenuEntry::Submenu(t!("git-remotes-menu").to_owned(), items));
    entries
}

/// 执行包含拉取的操作 `pull`（拉取、同步），完成后检查未提交的更改是否已恢复（见
/// [`Git::carried`]），返回操作的结果和后续动作：未能完整恢复时提示更改所在的位置。`what` 是
/// 操作的名称。
pub(super) fn keep_changes<T>(
    git: &Git,
    root: &Path,
    what: &str,
    pull: impl FnOnce(&Git) -> Result<T, String>,
) -> Result<(T, Then), String> {
    let before = git.stash_tip(root);
    let summary = pull(git)?;
    let then = match git.carried(root, before.as_deref()) {
        Carried::Restored => Then::Nothing,
        Carried::Conflicted => Then::Warn(t!("git-carry-conflict", what = what)),
        Carried::Kept => Then::Warn(t!("git-carry-kept", what = what)),
    };
    Ok((summary, then))
}

/// 返回提交号的前 7 位；不是提交号（分支名、标签名）时原样返回，名字里有汉字也不会被截断在
/// 一个字的中间。
pub(crate) fn short(oid: &str) -> &str {
    if oid.len() > 7 && oid.bytes().all(|b| b.is_ascii_hexdigit()) {
        &oid[..7]
    } else {
        oid
    }
}

/// 在后台准备一个文件在两个版本之间的差异。
///
/// 左侧为 `old` 中的版本（重命名的文件取旧路径；`None` 表示根提交之前，内容为空），
/// 右侧为 `new` 中的版本。
pub(in crate::app) fn commit_diff(
    git: &Git,
    root: &Path,
    old_rev: Option<&str>,
    new_rev: &str,
    file: &CommitFile,
) -> Result<DiffView, String> {
    let name = winc_editor::display_name(&file.path).into_owned();
    let cancel = Cancel::default();
    let read = |rev: &str, path: &Path| -> Result<Option<String>, String> {
        let bytes = git
            .content_at(root, rev, path, &cancel)
            .map_err(|e| t!("git-read-file-failed", name = name.as_str(), error = e))?;
        match bytes {
            Some(b) if b.len() as u64 > LARGE_FILE => {
                Err(t!("git-diff-too-large", name = name.as_str()))
            }
            Some(b) if looks_binary(&b) => Err(t!("git-diff-binary", name = name.as_str())),
            Some(b) => Ok(Some(decode_unknown(&b).0)),
            None => Ok(None),
        }
    };
    let old_path: PathBuf = file.from.clone().unwrap_or_else(|| file.path.clone());
    let old = match old_rev {
        Some(rev) => read(rev, &old_path)?,
        None => None,
    };
    let new = read(new_rev, &file.path)?;
    let language = winc_syntax::detect(&name);
    let side_title = |oid: Option<&str>, present: bool| match (oid, present) {
        (Some(oid), true) => short(oid).to_owned(),
        (Some(oid), false) => t!("git-diff-side-commit-missing", commit = short(oid)),
        (None, _) => t!("git-diff-side-before-root").to_owned(),
    };
    let mut view = DiffView::new(
        DiffSide::new(
            side_title(old_rev, old.is_some()),
            old.as_deref().unwrap_or(""),
            None,
            language,
        )
        .with_missing(old_rev.is_some() && old.is_none()),
        DiffSide::new(
            side_title(Some(new_rev), new.is_some()),
            new.as_deref().unwrap_or(""),
            None,
            language,
        )
        .with_missing(new.is_none()),
    );
    view.snapshot_path = Some(file.path.clone());
    Ok(view)
}

impl App {
    /// 推送当前分支要做的事：有上游时推送，没有时发布到 `publish_remote`（状态还没读到时按
    /// `origin`、第一个远程的顺序选择）；没有远程时提示原因并返回 `None`。
    pub(in crate::app) fn push_intent(&mut self) -> Option<PushIntent> {
        if self.has_upstream() {
            return Some(PushIntent::Push);
        }
        let fallback = (self.git.remotes.iter())
            .find(|r| *r == "origin")
            .or(self.git.remotes.first());
        let remote = self.git.publish_remote.clone().or(fallback.cloned());
        if remote.is_none() {
            self.notify(NoticeKind::Warning, t!("git-push-no-remote"));
        }
        remote.map(PushIntent::Publish)
    }

    /// 当前分支是否配置了上游分支。
    pub(in crate::app) fn has_upstream(&self) -> bool {
        self.git
            .branches
            .iter()
            .any(|b| b.current && b.upstream.is_some())
    }

    /// 拉取、推送、获取（`part`）现在能不能用：有 Git 操作在跑时都不能；没有远程时都不能；拉取要
    /// 当前分支有上游；推送要在分支上（没有上游时推上去并设为上游）。
    pub(crate) fn picker_action_enabled(&self, part: PickerPart) -> bool {
        if self.git.branch.running.is_some() || self.git.remotes.is_empty() {
            return false;
        }
        let on_branch = (self.git.status.as_ref())
            .is_some_and(|s| matches!(s.head, Some(winc_editor::git::Head::Branch(_))));
        match part {
            PickerPart::Pull => self.has_upstream(),
            PickerPart::Push => on_branch,
            _ => true,
        }
    }

    /// 从 `anchor` 展开分支选择器。
    pub(in crate::app) fn open_branch_picker(&mut self, anchor: GitAnchor) {
        self.finish_name_input();
        self.overlays.picker.places = None;
        self.overlays.picker.location_rows = None;
        self.overlays.picker.folders = false;
        self.overlays.picker.languages = false;
        self.overlays.picker.vim_list = None;
        self.overlays.picker.anchor = anchor;
        self.overlays.picker.open = true;
        self.overlays.picker.doc = self.editor.active().map(|d| d.id);
        self.overlays.picker.input.set_text("");
        self.overlays.picker.branch_rows = Some(Vec::new());
        self.focus = Focus::Picker;
        self.picker_refresh();
        self.relayout();
        self.reset_blink();
        self.request_redraw();
    }

    /// 对储藏 `stash` 执行 `op`。
    ///
    /// 储藏按提交号指定（[`Git::stash`]），确认和执行期间其他储藏的增删不影响目标。
    pub(in crate::app) fn run_stash_op(&mut self, op: StashOp, stash: Stash) {
        let (name, oid) = (stash.name.clone(), stash.oid.clone());
        match op {
            StashOp::Apply => self.git_operate(
                t!("git-op-stash-apply", name = name.as_str()),
                move |git, root| git.stash(root, "apply", &oid),
            ),
            StashOp::Pop => self.git_operate(
                t!("git-op-stash-pop", name = name.as_str()),
                move |git, root| git.stash(root, "pop", &oid),
            ),
            StashOp::Drop => self.confirm_drop_stash(stash),
        }
    }

    /// 删除储藏前先确认（删掉就找不回来了）；没有窗口时（测试中）直接删。
    fn confirm_drop_stash(&mut self, stash: Stash) {
        let message = t!("git-stash-drop-confirm", name = stash_title(&stash));
        if self.window.handle.is_some() {
            let detail = t!("git-stash-drop-confirm-detail");
            let button = t!("git-stash-drop-confirm-button");
            self.show_confirm(&message, detail, button, move |ok| {
                let op = LogConfirmed::DropStash(Box::new(stash.clone()));
                ok.then_some(UserEvent::GitLogConfirmed(op))
            });
            return;
        }
        // 没有窗口（测试中）时直接删除。
        self.drop_stash(stash);
    }

    /// 删除一条储藏（`git stash drop`）。
    pub(super) fn drop_stash(&mut self, stash: Stash) {
        self.git_operate(
            t!("git-op-stash-drop", name = stash.name.as_str()),
            move |git, root| git.stash(root, "drop", &stash.oid),
        );
    }

    /// 在后台删除分支 `name`（[`Git::delete_branch`]）；未完全合并而 `force` 为假时转为询问。
    pub(in crate::app) fn run_delete_branch(&mut self, name: String, force: bool) {
        self.git_operate_then(
            t!("git-op-delete-branch", name = name.as_str()),
            move |git, root| {
                let deleted = git.delete_branch(root, &name, force)?;
                Ok(match deleted {
                    BranchDeleted::Done(summary) => (summary, Then::Nothing),
                    BranchDeleted::NeedsForce => (String::new(), Then::ConfirmForceDelete(name)),
                })
            },
        );
    }

    /// 在后台做一件会改仓库的事 `what`，同一时刻只做一件。
    pub(super) fn git_operate(
        &mut self,
        what: String,
        work: impl FnOnce(&Git, &Path) -> Result<String, String> + Send + 'static,
    ) {
        self.git_operate_then(what, move |git, root| {
            work(git, root).map(|summary| (summary, Then::Nothing))
        });
    }

    /// 和 [`App::git_operate`] 一样，成功之后还要做 `work` 交回来的 [`Then`]。
    pub(super) fn git_operate_then(
        &mut self,
        what: String,
        work: impl FnOnce(&Git, &Path) -> Result<(String, Then), String> + Send + 'static,
    ) {
        let Some(root) = self.git.root.clone() else {
            return;
        };
        self.git_operate_in(root, what, work);
    }

    /// 和 [`App::git_operate_then`] 一样，但在目录 `dir` 中进行，`dir` 可以不是当前仓库。
    pub(super) fn git_operate_in(
        &mut self,
        dir: PathBuf,
        what: String,
        work: impl FnOnce(&Git, &Path) -> Result<(String, Then), String> + Send + 'static,
    ) {
        if self.git.branch.running.is_some() {
            self.flash(t!("git-op-busy"));
            return;
        }
        let root = self.git.root.clone();
        let sender = self.git.branch.results.0.clone();
        self.git.branch.running = Some(what.clone());
        self.git.branch.finished = None;
        self.git.branch.declined = false;
        self.git.branch.started += 1;
        let id = self.git.branch.started;
        let wake = self.link.proxy.clone();
        let questions = self.git.branch.questions.0.clone();
        // 凭据只为本机运行的 `git` 保存：远端主机和发行版中的 `git` 访问不到本机的凭据存储。
        let remember = self.config.git_remember_credentials != Some(false)
            && winc_editor::git::runs_here(&dir);
        self.spawn_git(UserEvent::GitOperated, move |tool| {
            let result = match tool {
                Ok(git) => {
                    // 端点只在操作期间存在，`listener` 在本分支结束时被丢弃。
                    let listener =
                        wake.and_then(|proxy| Asker::listen(proxy, questions, id, remember));
                    let askpass = listener.as_ref().and_then(Listener::askpass);
                    work(&git.with_askpass(askpass), &dir)
                }
                Err(e) => Err(unusable(&e)),
            };
            let _ = sender.send(Done {
                id,
                root,
                what,
                result,
            });
        });
        self.request_redraw();
    }

    /// 同步控件上对勾的不透明度：过渡和侧栏里展开、收起的时长相同，没有对勾时为 0。
    pub(crate) fn sync_check(&self) -> f32 {
        self.git
            .branch
            .finished
            .map_or(0.0, |clock| clock.pulse(TREE_ANIMATION))
    }
}
