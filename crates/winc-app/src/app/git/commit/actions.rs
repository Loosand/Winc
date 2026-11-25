//! 提交和回滚：先保存要提交的文件、请用户确认回滚，在后台运行 git 并收下结果。

use std::sync::{Arc, PoisonError};

use winc_core::diff::Pick;
use winc_editor::format::Cancel;
use winc_editor::git::{CommitOptions, Synced};
use winc_i18n::t;

use super::*;
use crate::app::NoticeKind;
use crate::app::git::operate::{Then, keep_changes};
use crate::app::lifecycle::notice::NoticeAction;

/// 分支行右端同步控件的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyncState {
    /// 仓库没有配置远程。
    NoRemote,
    /// 这时不同步：不在分支上、有进行中的操作，或者在修改上一次提交。
    Hidden,
    /// 和上游一致。
    InSync,
    /// 分支还没有上游：发布到远程。
    Publish,
    /// 领先上游这么多个提交：推送。
    Push(u32),
    /// 落后上游这么多个提交：拉取。
    Pull(u32),
    /// 两边都有新提交：同步。
    Both {
        /// 领先的提交数。
        ahead: u32,
        /// 落后的提交数。
        behind: u32,
    },
}

impl App {
    /// 分支和上游的关系决定的同步控件状态。
    pub(crate) fn git_sync_state(&self) -> SyncState {
        let Some(status) = self.git.status.as_ref() else {
            return SyncState::Hidden;
        };
        if self.git.remotes.is_empty() {
            return SyncState::NoRemote;
        }
        let on_branch = matches!(status.head, Some(Head::Branch(_)));
        if !on_branch || status.operation.is_some() || self.git.commit.amend {
            return SyncState::Hidden;
        }
        // 分支列表还没读到时不知道有没有上游。
        if self.git.branches.is_empty() {
            return SyncState::Hidden;
        }
        if !self.has_upstream() {
            return SyncState::Publish;
        }
        match (status.ahead, status.behind) {
            (0, 0) => SyncState::InSync,
            (ahead, 0) => SyncState::Push(ahead),
            (0, behind) => SyncState::Pull(behind),
            (ahead, behind) => SyncState::Both { ahead, behind },
        }
    }

    /// 点击同步控件：有差异时同步，没有上游时发布，其余时候获取远程更新。
    ///
    /// 会推送的发布和同步先确认（[`App::ask_push`]）；只拉取时不确认。
    pub(in crate::app) fn git_sync(&mut self) {
        match self.git_sync_state() {
            SyncState::Publish => {
                if let Some(intent) = self.push_intent() {
                    self.ask_push(intent);
                }
            }
            SyncState::Push(_) | SyncState::Both { .. } => self.ask_push(PushIntent::Sync),
            SyncState::Pull(_) => self.run_git_sync(false),
            SyncState::NoRemote | SyncState::Hidden | SyncState::InSync => {
                self.git_operate(t!("git-op-fetch").into(), |git, root| {
                    git.operate(root, &["fetch", "--all", "--prune"])
                });
            }
        }
    }

    /// 在后台同步（[`winc_editor::git::Git::sync`]）；需要强制推送而 `force` 为假时转为询问。
    /// 确认强制推送以后，询问前一并提交了的其他仓库照样推送出去。
    pub(in crate::app) fn run_git_sync(&mut self, force: bool) {
        let others = match force {
            true => std::mem::take(&mut self.git.commit.force_others),
            false => Vec::new(),
        };
        self.run_git_sync_with(others, force);
    }

    /// 同步当前分支，之后把一并提交了的其他仓库 `others` 推送出去（见 [`push_others`]）。
    /// 需要先确认强制推送时记下 `others`，确认以后再推（见 [`App::run_git_sync`]）。
    fn run_git_sync_with(&mut self, others: Vec<PathBuf>, force: bool) {
        self.git.commit.force_others = others.clone();
        self.git_operate_then(t!("git-op-sync").into(), move |git, root| {
            let (synced, then) =
                keep_changes(git, root, t!("git-op-sync"), |git| git.sync(root, force))?;
            Ok(match synced {
                Synced::Done(summary) => {
                    push_others(git, &others)?;
                    (summary, then)
                }
                Synced::NeedsForce(count) => (String::new(), Then::ConfirmForcePush(count)),
            })
        });
    }

    /// 发布、推送或同步当前分支前用提示框确认，写明推到哪个远程仓库；配置项 `git.confirmPush`
    /// 为 `false`（提示框上勾选过「不再询问」）时不询问，直接执行。
    pub(in crate::app) fn ask_push(&mut self, intent: PushIntent) {
        if self.config.git_confirm_push == Some(false) {
            self.run_push(intent);
            return;
        }
        // 先在后台读出会送出的提交，读到以后再问（[`App::take_push_preview`]）：确认框里列出它们，
        // 推送之前看得到送出去的是什么。
        let slot = Arc::clone(&self.git.push_preview);
        let Some(root) = self.git.root.clone() else {
            return;
        };
        self.spawn_git(UserEvent::GitStatus, move |tool| {
            let target = match intent {
                PushIntent::Publish(_) => winc_editor::git::Outgoing::Publish,
                _ => winc_editor::git::Outgoing::Upstream,
            };
            let read = (tool.ok()).map(|git| git.outgoing(&root, target, PUSH_PREVIEW));
            let (subjects, total) = read.and_then(Result::ok).unwrap_or_default();
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some((intent, subjects, total));
        });
        // 没有事件循环（测试中）时后台的工作已经就地做完：直接取用。
        self.take_push_preview();
    }

    /// 后台读到了会送出的提交：弹出确认框，说明后面列出这些提交的标题；超出的写明还有几个。
    ///
    /// 第二个按钮「查看提交」打开 Git 日志：送出的就是上游的标记上面的那些提交，点开能看各自
    /// 改了哪些文件。
    pub(in crate::app) fn take_push_preview(&mut self) {
        let preview = (self.git.push_preview.lock())
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some((intent, subjects, total)) = preview else {
            return;
        };
        // 推送和发布用推送对话框：可以改远程和目标分支、一并推送标签。同步先拉取再推送，目标
        // 不能改，仍用提示框确认。
        if intent != PushIntent::Sync {
            self.open_push_dialog(intent, subjects, total);
            return;
        }
        let (message, mut detail, button) = self.push_prompt(&intent);
        for subject in &subjects {
            detail.push_str("\n· ");
            detail.push_str(subject);
        }
        if total > subjects.len() {
            let more = total - subjects.len();
            detail.push('\n');
            detail.push_str(&t!("git-push-confirm-more", count = more));
        }
        let buttons = vec![
            AlertButton::new(&button),
            AlertButton::new(t!("git-push-confirm-review")),
            AlertButton::cancel(t!("common-cancel")),
        ];
        let asked = Some((t!("common-dont-ask"), false));
        self.show_alert(
            &message,
            &detail,
            buttons,
            asked,
            move |choice, never| match choice {
                0 => Some(UserEvent::GitPushConfirmed(intent.clone(), never)),
                1 => Some(UserEvent::GitShowLog),
                _ => None,
            },
        );
    }

    /// 提示框中确认了 `intent`：勾选了「不再询问」（`never`）时写入配置，然后执行。
    pub(in crate::app) fn push_confirmed(&mut self, intent: PushIntent, never: bool) {
        if never {
            self.config.git_confirm_push = Some(false);
            self.save_config();
        }
        self.run_push(intent);
    }

    /// 推送对话框按了主按钮：推到 `remote` 的分支 `target`，`tags` 为真时一并推送全部标签；
    /// 勾选了「不再询问」（`never`）时写入配置。
    ///
    /// 目标没有改、也不推送标签时与直接推送相同（[`App::run_push`]）；发布时把目标设为上游。
    pub(in crate::app) fn submit_push(
        &mut self,
        form: super::super::dialog::PushForm,
        (remote, target): (String, String),
        tags: bool,
        never: bool,
    ) {
        if never {
            self.config.git_confirm_push = Some(false);
            self.save_config();
        }
        if !tags && remote == form.remote && target == form.target {
            self.run_push(form.intent);
            return;
        }
        let track = matches!(form.intent, PushIntent::Publish(_));
        // 同步改了目标或要一并推送标签：仍然先拉取（与同步相同），再按对话框推送，本地落后上游
        // 时推送才不会被拒绝。
        let sync = matches!(form.intent, PushIntent::Sync);
        let what = if sync {
            t!("git-op-sync")
        } else {
            t!("git-op-push")
        };
        self.git_operate_then(what.into(), move |git, root| {
            let then = match sync {
                true => keep_changes(git, root, t!("git-op-pull"), |git| git.pull(root))?.1,
                false => Then::Nothing,
            };
            let summary = git.push_to(root, &remote, &target, tags, track)?;
            Ok((summary, then))
        });
    }

    /// 确认 `intent` 的提示框内容：（标题，说明，确认按钮的文字）。
    pub(in crate::app) fn push_prompt(&self, intent: &PushIntent) -> (String, String, String) {
        let current = self.git.branches.iter().find(|b| b.current && !b.remote);
        let branch = current.map_or("HEAD", |b| b.name.as_str());
        let upstream = current.and_then(|b| b.upstream.as_deref()).unwrap_or("");
        let (ahead, behind) = (self.git.status.as_ref()).map_or((0, 0), |s| (s.ahead, s.behind));
        // 上游写作「远程名/分支名」，远程名本身可以含 `/`，按已知的远程名匹配最长的一个。
        let upstream_remote = (self.git.remotes.iter())
            .filter(|r| upstream.starts_with(&format!("{r}/")))
            .max_by_key(|r| r.len())
            .map(String::as_str);
        // 推送的目标写远程的地址；取不到地址时写远程名。
        let target = |remote: &str| {
            (self.git.remote_list.iter())
                .find(|r| r.name == remote)
                .map_or_else(|| remote.to_owned(), |r| r.url.clone())
        };
        let upstream_target = target(upstream_remote.unwrap_or(upstream));
        let (message, detail, button) = match intent {
            PushIntent::Publish(remote) => (
                t!("git-push-confirm-publish", remote = remote.as_str()),
                t!(
                    "git-push-confirm-publish-detail",
                    branch = branch,
                    target = target(remote)
                ),
                t!("git-push-confirm-publish-button"),
            ),
            PushIntent::Push => (
                t!("git-push-confirm-push", upstream = upstream),
                t!(
                    "git-push-confirm-push-detail",
                    count = ahead,
                    target = upstream_target
                ),
                t!("git-push-confirm-push-button"),
            ),
            PushIntent::Sync => (
                t!("git-push-confirm-sync", upstream = upstream),
                t!(
                    "git-push-confirm-sync-detail",
                    ahead = ahead,
                    behind = behind,
                    target = upstream_target
                ),
                t!("git-push-confirm-sync-button"),
            ),
        };
        (message.to_owned(), detail.to_owned(), button.to_owned())
    }

    /// 执行确认过的 `intent`：发布推到确认时写明的远程。
    pub(in crate::app) fn run_push(&mut self, intent: PushIntent) {
        self.run_push_with(intent, Vec::new());
    }

    /// 按 `intent` 推送当前分支，之后把一并提交了的其他仓库 `others` 推送出去（见
    /// [`push_others`]）。
    fn run_push_with(&mut self, intent: PushIntent, others: Vec<PathBuf>) {
        match intent {
            PushIntent::Publish(remote) => {
                self.git_operate(t!("git-op-push").into(), move |git, root| {
                    let summary = git.publish(root, &remote)?;
                    push_others(git, &others).map(|()| summary)
                });
            }
            PushIntent::Push => self.git_operate(t!("git-op-push").into(), move |git, root| {
                let summary = git.push(root, true)?;
                push_others(git, &others).map(|()| summary)
            }),
            PushIntent::Sync => self.run_git_sync_with(others, false),
        }
    }

    /// 同步需要强制推送：用提示框询问，确认后才替换上游的 `count` 个提交。
    pub(in crate::app) fn confirm_force_push(&mut self, count: usize) {
        let branch = (self.git.branches.iter())
            .find(|b| b.current)
            .map_or("HEAD", |b| b.name.as_str());
        let message = t!("git-force-push-confirm", branch = branch);
        let detail = t!("git-force-push-confirm-detail", count = count);
        let button = t!("git-force-push-confirm-button");
        self.show_confirm(&message, &detail, button, |ok| {
            ok.then_some(UserEvent::GitForcePush)
        });
    }

    /// 提交并推送：可以提交时用提示框询问，写明提交多少文件、推到哪个分支；不能提交时由提交的入口
    /// 说明原因。
    pub(in crate::app) fn confirm_commit_and_push(&mut self) {
        self.confirm_commit_then(false);
    }

    /// 提交并同步：可以提交时用提示框询问，写明提交多少文件、在哪个分支上同步；不能提交时由提交的
    /// 入口说明原因。
    pub(in crate::app) fn confirm_commit_and_sync(&mut self) {
        self.confirm_commit_then(true);
    }

    /// 「提交并推送」（`sync` 为假）或「提交并同步」的询问。
    fn confirm_commit_then(&mut self, sync: bool) {
        let ready = !self.git.commit.busy
            && (self.git.status.as_ref())
                .is_some_and(|status| self.commit_blocker(status).is_none());
        if !ready {
            self.git_commit_then(sync);
            return;
        }
        let branch = self
            .git
            .branches
            .iter()
            .find(|b| b.current)
            .map(|b| b.name.clone());
        let count = self.commit_scope().map(|scope| match scope {
            CommitScope::Staged(count) | CommitScope::All(count) => count,
            CommitScope::Nothing => 0,
        });
        let detail = match (count, branch, sync) {
            (Some(count), Some(branch), false) => t!(
                "git-commit-push-confirm-scope",
                count = count,
                branch = branch.as_str()
            ),
            (Some(count), Some(branch), true) => t!(
                "git-commit-sync-confirm-scope",
                count = count,
                branch = branch.as_str()
            ),
            (_, _, false) => t!("git-commit-push-confirm-detail").to_owned(),
            (_, _, true) => t!("git-commit-sync-confirm-detail").to_owned(),
        };
        // 一并提交的其他仓库也会推送出去：写明是哪几个。
        let others = self.other_commit_names();
        let detail = match others.is_empty() {
            true => detail,
            false => {
                let names = others.join(t!("common-list-separator"));
                let extra = t!(
                    "git-commit-push-others",
                    count = others.len(),
                    names = names
                );
                format!("{detail}\n{extra}")
            }
        };
        let (message, button) = match sync {
            false => (
                t!("git-commit-push-confirm"),
                t!("git-commit-push-confirm-button"),
            ),
            true => (
                t!("git-commit-sync-confirm"),
                t!("git-commit-sync-confirm-button"),
            ),
        };
        let buttons = vec![
            AlertButton::new(button),
            AlertButton::cancel(t!("common-cancel")),
        ];
        self.show_alert(message, &detail, buttons, None, move |choice, _| {
            (choice == 0).then_some(match sync {
                false => UserEvent::GitCommitAndPush,
                true => UserEvent::GitCommitAndSync,
            })
        });
    }

    /// 提交暂存区的内容；没有暂存任何内容时提交全部已跟踪文件的改动。
    ///
    /// 没写提交信息、没有可以提交的内容时在状态栏说明，不提交。冲突已解决（不再有冲突标记）的
    /// 文件随提交加入暂存区。
    pub(in crate::app) fn git_commit(&mut self) {
        if self.git.commit.busy {
            return;
        }
        // 暂存的操作尚未写完：写完后再提交，提交的才是界面上显示的内容。
        if self.git.commit.staging_active() || self.git.commit.other_staging_active() {
            self.git.commit.commit_queued = true;
            return;
        }
        // 提交前先格式化、再检查新增的 TODO：都在后台进行，做完再接着提交（见
        // `App::reformatted`、`App::todo_checked`）。
        if self.reformat_first() || self.check_todo_first() {
            return;
        }
        if !self.git.commit.staging() {
            return self.git_commit_picked();
        }
        let message = self.git.commit.message.text().trim().to_owned();
        let Some(status) = self.git.status.as_ref() else {
            return;
        };
        let rebasing = status.operation == Some(Operation::Rebase);
        if message.is_empty() && !rebasing {
            self.flash(t!("git-commit-message-required"));
            self.focus = Focus::CommitMessage;
            return;
        }
        let amend = self.git.commit.amend;
        let scope = CommitScope::of(status);
        // 修改上次提交、合并或变基暂停时只动当前仓库。其他仓库有暂存的内容时，当前仓库只提交
        // 已暂存的部分，不扩大为全部已跟踪文件。
        let held = amend || status.operation.is_some();
        let others = if held {
            Vec::new()
        } else {
            self.other_staged_roots()
        };
        let all = matches!(scope, CommitScope::All(_)) && !held && others.is_empty();
        let paths_of = |keep: &dyn Fn(&Entry) -> bool| -> Vec<PathBuf> {
            let kept = status.entries.iter().filter(|e| keep(e));
            kept.map(|e| e.path.clone()).collect()
        };
        let conflicted = paths_of(&Entry::conflicted);
        let own = held || all || !conflicted.is_empty() || matches!(scope, CommitScope::Staged(_));
        if !own && others.is_empty() {
            self.flash(t!("git-commit-nothing-checked"));
            return;
        }
        // 提交全部时按磁盘上的内容提交：这些文件在编辑器里未保存的修改先保存。冲突的文件同样
        // 先保存，下面检查的是磁盘上的内容。
        let mut saved = conflicted.clone();
        if all {
            saved.extend(paths_of(&|e| !e.untracked() && !e.conflicted()));
        }
        if !self.save_dirty_documents(&saved) {
            return;
        }
        // 仍有冲突标记的文件不能提交，否则 `git add` 将其视为已解决，标记和两侧内容会一并进入
        // 提交；这里读取刚保存的磁盘内容，使编辑器刚移除标记、状态尚未刷新时的结果也正确。
        let unresolved = (conflicted.iter())
            .filter(|path| winc_editor::git::merge::file_has_conflicts(path))
            .count();
        if unresolved > 0 {
            self.flash(t!("git-commit-unresolved-conflicts", count = unresolved));
            return;
        }
        // 变基停下来时：解决了的文件标记为已解决，继续变基（git 用原来的提交信息）。
        if rebasing {
            self.git_operate(t!("git-op-continue-rebase").to_owned(), move |git, root| {
                if !conflicted.is_empty() {
                    git.stage(root, &conflicted)?;
                }
                git.continue_operation(root, Operation::Rebase)
            });
            return;
        }
        let options = winc_editor::git::CommitOptions {
            all,
            amend,
            sign_off: self.commit_signs_off(),
            no_verify: self.git.commit.no_verify,
        };
        let committed: Vec<PathBuf> = others.iter().map(|(root, _)| root.clone()).collect();
        self.git.commit.others_left = self.others_left(&committed);
        self.git.commit.committed_with = committed;
        let sender = self.git.commit.results.0.clone();
        self.git.commit.busy = true;
        self.run_git_job(move |git, root| {
            // 先提交当前仓库，再依次提交其他仓库暂存的内容；其中一个失败就停下，已经提交的保留。
            let result = git.and_then(|git| {
                let mut done = CommittedIn::default();
                if own {
                    if !conflicted.is_empty() {
                        git.stage(root, &conflicted)?;
                    }
                    done.add(root, git.commit_with(root, &message, options)?);
                }
                for (other, sign_off) in &others {
                    let options = winc_editor::git::CommitOptions {
                        all: false,
                        amend: false,
                        sign_off: *sign_off,
                        ..options
                    };
                    let oid = git.commit_with(other, &message, options);
                    done.add(other, oid.map_err(|error| done.failed(other, &error))?);
                }
                Ok(done.first.unwrap_or_default())
            });
            let _ = sender.send(Outcome::Committed(result, message, root.to_path_buf()));
        });
    }

    /// 当前仓库提交时是否附加签署行（在提交按钮的菜单中打开，按仓库记在状态文件里）。
    pub(in crate::app) fn commit_signs_off(&self) -> bool {
        (self.git.root.as_ref()).is_some_and(|root| self.state.sign_off_repos.contains(root))
    }

    /// 提交勾上的文件，成功以后继续推送。
    pub(in crate::app) fn git_commit_and_push(&mut self) {
        self.git_commit_then(false);
    }

    /// 提交勾上的文件，成功以后与上游同步（先拉取再推送）。
    pub(in crate::app) fn git_commit_and_sync(&mut self) {
        self.git_commit_then(true);
    }

    /// 提交，成功以后推送（`sync` 时同步）。
    fn git_commit_then(&mut self, sync: bool) {
        self.git.commit.push_after = true;
        self.git.commit.sync_after = sync;
        self.git_commit();
        // 没有真的开始提交（没写信息、没勾文件），就不留着「之后推送」。
        if !self.git.commit.busy {
            self.git.commit.push_after = false;
            self.git.commit.sync_after = false;
        }
    }

    /// 新增 TODO 的确认框选了 `ok`（仍然提交）或取消。
    pub(in crate::app) fn todo_confirmed(&mut self, ok: bool) {
        if ok {
            self.git.commit.todo_passed = true;
            self.git_commit();
        } else {
            self.git.commit.push_after = false;
        }
    }

    /// 确认后中止进行中的合并、变基等操作。
    ///
    /// 中止会丢弃已部分解决的冲突，因此须先确认。
    pub(super) fn confirm_abort(&mut self) {
        let Some(operation) = self.git.status.as_ref().and_then(|s| s.operation) else {
            return;
        };
        let label = operation.label();
        let message = t!("git-abort-confirm", operation = label);
        if self.window.handle.is_some() {
            let detail = t!("git-abort-confirm-detail");
            let button = t!("git-abort-confirm-button", operation = label);
            self.show_confirm(&message, detail, &button, move |ok| {
                ok.then_some(UserEvent::GitAbort)
            });
            return;
        }
        // 没有窗口（测试中）时直接中止。
        let what = match operation {
            Operation::Merge => t!("git-op-abort-merge"),
            Operation::Rebase => t!("git-op-abort-rebase"),
            Operation::CherryPick => t!("git-op-abort-cherry-pick"),
            Operation::Revert => t!("git-op-abort-revert"),
        };
        self.git_operate(what.into(), move |git, root| {
            git.abort_operation(root, operation)
        });
    }

    /// 确认后跳过变基、拣选等操作停着的这个提交。
    ///
    /// 跳过会丢弃这个提交的更改和为它解决到一半的冲突，因此须先确认。
    pub(super) fn confirm_skip(&mut self) {
        let operation = self.git.status.as_ref().and_then(|s| s.operation);
        if operation.is_none_or(|operation| operation == Operation::Merge) {
            return;
        }
        if self.window.handle.is_some() {
            let message = t!("git-skip-confirm");
            let detail = t!("git-skip-confirm-detail");
            let button = t!("git-skip-confirm-button");
            self.show_confirm(message, detail, button, move |ok| {
                ok.then_some(UserEvent::GitSkip)
            });
            return;
        }
        // 没有窗口（测试中）时直接跳过。
        if let Some(operation) = operation {
            self.git_operate(t!("git-op-skip").into(), move |git, root| {
                git.skip_operation(root, operation)
            });
        }
    }

    /// 回滚前先确认（系统确认框）；确认后回滚组 `group` 中勾上的文件，没有勾上的文件时回滚
    /// 该组的全部文件。
    pub(super) fn ask_rollback(&mut self, group: CommitGroup) {
        let Some(status) = self.git.status.as_ref() else {
            return;
        };
        let staging = self.git.commit.staging();
        let held = || status.entries.iter().filter(|e| group.holds(e, staging));
        let mut paths: Vec<PathBuf> = held()
            .filter(|e| self.git.commit.checked(e))
            .map(|e| e.path.clone())
            .collect();
        // 勾选式下只回滚勾上的文件；暂存区视图中组里没有已暂存的文件时回滚整组。
        if paths.is_empty() && !self.git.commit.staging() {
            self.flash(t!("git-rollback-nothing-checked"));
            return;
        }
        if paths.is_empty() {
            paths = held().map(|e| e.path.clone()).collect();
        }
        if !paths.is_empty() {
            self.confirm_rollback(paths);
        }
    }

    /// 先请用户确认，确认后回滚 `paths`。
    pub(in crate::app) fn confirm_rollback(&mut self, paths: Vec<PathBuf>) {
        let message = match paths.as_slice() {
            [one] => {
                let name = winc_editor::display_name(one);
                t!("git-rollback-confirm-one", name = name)
            }
            many => t!("git-rollback-confirm-many", count = many.len()),
        };
        if self.window.handle.is_some() {
            // 远端主机上没有废纸篓：未跟踪的文件直接删除。
            let remote = paths.iter().any(|path| winc_backend::is_remote(path));
            let detail = if remote {
                t!("git-rollback-confirm-detail-remote")
            } else {
                t!("git-rollback-confirm-detail")
            };
            let button = t!("git-rollback-confirm-button");
            self.show_confirm(&message, detail, button, move |ok| {
                ok.then(|| UserEvent::GitRollback(paths.clone()))
            });
            return;
        }
        // 没有窗口（测试中）时直接回滚。
        self.git_rollback(paths);
    }

    /// 回滚 `paths`：已跟踪的恢复成 `HEAD` 里的内容，新加入版本管理的从暂存区移除并移到废纸篓，
    /// 未跟踪的移到废纸篓。
    ///
    /// 打开着的文档由目录监视仍然重新读取。
    pub(in crate::app) fn git_rollback(&mut self, paths: Vec<PathBuf>) {
        let Some(status) = self.git.status.as_ref() else {
            return;
        };
        let mut restore = Vec::new();
        let mut added = Vec::new();
        let mut trash = Vec::new();
        for entry in status.entries.iter().filter(|e| paths.contains(&e.path)) {
            if entry.untracked() {
                trash.push(entry.path.clone());
            } else if entry.staged == Some(winc_editor::git::Change::Added) {
                added.push(entry.path.clone());
                trash.push(entry.path.clone());
            } else {
                restore.push(entry.path.clone());
                restore.extend(entry.from.clone());
            }
        }
        let count = paths.len();
        // 本机的文件在界面线程上移到废纸篓（和文件树的删除同一个入口）；远端主机上的文件在后台
        // 直接删除，删完后再刷新一次状态。git 在后台。
        if let Some(first) = trash.first().cloned() {
            let (to_trash, remote) = (self.trash, winc_backend::is_remote(&first));
            let remove = move || -> Vec<(PathBuf, std::io::Result<()>)> {
                (trash.into_iter())
                    .map(|path| {
                        let result = match winc_backend::is_remote(&path) {
                            true => winc_backend::fs::remove_all(&path),
                            false => to_trash(&path),
                        };
                        (path, result)
                    })
                    .collect()
            };
            self.file_work(&first, remove, move |app, results| {
                for (path, result) in results {
                    if let Err(e) = result {
                        let (name, error) = (winc_editor::display_name(&path), e.to_string());
                        app.notify(
                            NoticeKind::Error,
                            t!("git-rollback-file-failed", name = name, error = error),
                        );
                    }
                }
                if remote {
                    app.git_refresh();
                }
            });
        }
        if restore.is_empty() && added.is_empty() {
            self.flash(t!("git-rollback-done", count = count));
            self.git_refresh();
            return;
        }
        let sender = self.git.commit.results.0.clone();
        self.git.commit.busy = true;
        self.run_git_job(move |git, root| {
            let result = git
                .and_then(|git| git.rollback(root, &restore, &added))
                .map(|()| count);
            let _ = sender.send(Outcome::RolledBack(result, root.to_path_buf()));
        });
    }

    /// 收下后台交回来的提交、回滚结果和上一次提交的信息。
    pub(in crate::app) fn poll_commit(&mut self) {
        while let Ok(outcome) = self.git.commit.results.1.try_recv() {
            match outcome {
                Outcome::StashFiles(oid, name, files) => self.stash_files_arrived(oid, name, files),
                Outcome::StashDiff(title, view) => self.stash_diff_arrived(title, view),
                Outcome::LastMessage(message) => {
                    // 等结果的时候用户已经关闭了「修改上一次提交」，就不填。
                    if self.git.commit.amend {
                        self.git.commit.message.set_text(&message);
                        self.git.commit.message.set_caret(message.len(), false);
                    }
                }
                Outcome::Committed(result, message, root) => {
                    // 提交的时候已经换到了别的仓库：只说一声结果，不推送（会推到现在这个仓库）、
                    // 也不动现在这个仓库的提交视图（换的时候已经清过，见 `reset_commit_view`）。
                    if self.git.root.as_deref() != Some(root.as_path()) {
                        let name = winc_editor::display_name(&root);
                        match result {
                            Ok(oid) => self.flash(t!("git-commit-done-in", name = name, oid = oid)),
                            Err(e) => self.notify_with(
                                NoticeKind::Error,
                                t!("git-commit-failed-in", name = name, error = e),
                                NoticeAction::GitOutput,
                            ),
                        }
                        continue;
                    }
                    self.git.commit.busy = false;
                    match result {
                        Ok(oid) => {
                            // 一并提交了的其他仓库：清掉它们的勾选，重读状态。没有一并提交的勾选
                            // 留着，并说明一声。
                            let others = self.git.commit.committed_with.clone();
                            let left = std::mem::take(&mut self.git.commit.others_left);
                            if !others.is_empty() {
                                self.clear_committed_checks();
                                self.git_read_summaries();
                            }
                            // 提交期间用户又改了信息，就留着改过的，不清空。
                            self.remember_commit_message(&message);
                            if self.git.commit.message.text().trim() == message {
                                self.git.commit.message.remember();
                                self.git.commit.message.set_text("");
                            }
                            let subject = message.lines().next().unwrap_or_default();
                            // 「提交并推送」「提交并同步」已经确认过，推送、同步不再询问。没有上游
                            // 时同步无从谈起，与推送一样发布到远程。
                            let sync = std::mem::take(&mut self.git.commit.sync_after);
                            if std::mem::take(&mut self.git.commit.push_after)
                                && let Some(intent) = self.push_intent()
                            {
                                match (sync, intent) {
                                    (true, PushIntent::Push) => {
                                        self.run_git_sync_with(others, false)
                                    }
                                    (_, intent) => self.run_push_with(intent, others),
                                }
                            }
                            // 勾上的都提交了；取消勾选的各处改动还在工作区里，下次重新勾选。
                            self.git.commit.skipped.clear();
                            self.git.commit.reset_checks();
                            self.git.commit.amend = false;
                            self.git.commit.no_verify = false;
                            self.git.commit.draft = None;
                            if left > 0 {
                                self.notify(
                                    NoticeKind::Warning,
                                    t!("git-commit-others-left", count = left),
                                );
                            } else {
                                self.flash(t!("git-commit-done", oid = &oid, subject = subject));
                            }
                            let short: String = oid.chars().take(7).collect();
                            self.git.commit.committed =
                                Some((short, Instant::now() + COMMITTED_SHOWN));
                        }
                        Err(e) => {
                            self.git.commit.push_after = false;
                            self.git.commit.sync_after = false;
                            self.git.commit.committed_with.clear();
                            self.git.commit.others_left = 0;
                            self.notify_with(
                                NoticeKind::Error,
                                t!("git-commit-failed", error = e),
                                NoticeAction::GitOutput,
                            );
                            // 失败前可能已经提交了其他仓库：重读它们的状态。
                            if self.commit_shows_others() {
                                self.git_read_summaries();
                            }
                        }
                    }
                    self.git_refresh();
                }
                Outcome::Staged(result, root) => {
                    if self.staging_finished(result, &root) {
                        self.git_commit();
                    }
                }
                Outcome::OtherStaged(result) => {
                    if self.other_staging_finished(result) {
                        self.git_commit();
                    }
                }
                Outcome::TodoChecked(count, root) => {
                    if self.todo_checked(count, &root) {
                        self.git_commit();
                    }
                }
                Outcome::Reformatted(count, root) => {
                    if self.reformatted(count, &root) {
                        self.git_commit();
                    }
                }
                Outcome::IndexText(root, path, version, text) => {
                    self.index_text_arrived(&root, path, version, text);
                }
                Outcome::Last(last, root) => {
                    let oid = self.git.status.as_ref().and_then(|s| s.oid.as_ref());
                    if self.git.root.as_deref() == Some(root.as_path())
                        && last.as_ref().map(|l| &l.oid) == oid
                    {
                        self.git.commit.last = last;
                    }
                }
                Outcome::Undone(result, last, root) => {
                    let name = winc_editor::display_name(&root);
                    let here = self.git.root.as_deref() == Some(root.as_path());
                    let tag: String = last.oid.chars().take(7).collect();
                    match result {
                        Ok(()) if here => {
                            if self.git.commit.message.text().trim().is_empty() {
                                self.git.commit.message.set_text(&last.message);
                                let end = last.message.len();
                                self.git.commit.message.set_caret(end, false);
                            }
                            self.flash(t!("git-undo-commit-done", oid = tag));
                        }
                        Ok(()) => self.flash(t!("git-undo-commit-done-in", name = name, oid = tag)),
                        Err(e) => {
                            self.git.commit.last_for = None;
                            self.notify(NoticeKind::Error, t!("git-undo-commit-failed", error = e));
                        }
                    }
                    if here {
                        self.git_refresh();
                    }
                }
                Outcome::RolledBack(result, root) => {
                    if self.git.root.as_deref() != Some(root.as_path()) {
                        let name = winc_editor::display_name(&root);
                        match result {
                            Ok(count) => {
                                self.flash(t!("git-rollback-done-in", name = name, count = count))
                            }
                            Err(e) => self.notify(
                                NoticeKind::Error,
                                t!("git-rollback-failed-in", name = name, error = e),
                            ),
                        }
                        continue;
                    }
                    self.git.commit.busy = false;
                    match result {
                        Ok(count) => self.flash(t!("git-rollback-done", count = count)),
                        Err(e) => {
                            self.notify(NoticeKind::Error, t!("git-rollback-failed", error = e))
                        }
                    }
                    // 重新读打开着的文档，顺带刷新仓库状态。
                    self.check_disk();
                }
            }
        }
        self.request_redraw();
    }

    /// 换了仓库：提交视图回到初始的样子（写了一半的信息、「修改上一次提交」、勾选、正在提交都是
    /// 旧仓库的）；列表的显示方式（平铺、收起的分组）保留，旧仓库尚未交回的结果仍然收下。
    pub(in crate::app::git) fn reset_commit_view(&mut self) {
        let old = std::mem::take(&mut self.git.commit);
        self.git.commit.results = old.results;
        self.git.commit.flat = old.flat;
        self.git.commit.sort = old.sort;
        self.git.commit.staging = old.staging;
        self.git.commit.other_checked = old.other_checked;
        self.git.commit.other_skipped = old.other_skipped;
        self.git.commit.other_heads = old.other_heads;
        self.git.commit.other_folds = old.other_folds;
        // 写入其他仓库暂存区的操作按仓库的路径进行，与当前是哪个仓库无关：换仓库时接着做完，
        // 界面上已经按做完的结果显示了。
        self.git.commit.other_stage_queue = old.other_stage_queue;
        self.git.commit.other_stage_running = old.other_stage_running;
        self.git.commit.collapsed = old.collapsed;
        self.git.commit.drafts = old.drafts;
        // 保留底部各组的展开状态和高度、卡片和「上一次提交」的显示比例：切换仓库后从原状态过渡。
        self.git.commit.quiet_open = old.quiet_open;
        self.git.commit.stashes = Drawer {
            motion: None,
            ..old.stashes
        };
        self.git.commit.worktrees = Drawer {
            motion: None,
            ..old.worktrees
        };
        self.git.commit.card = old.card;
        self.git.commit.undo = old.undo;
        self.git.commit.undo_drawn = old.undo_drawn;
        self.git.commit.sections_known = old.sections_known;
    }

    /// 勾选式下提交勾上的文件：把勾上的内容写入暂存区，再提交暂存区。
    ///
    /// 提交按磁盘上的内容：勾上的文件在编辑器中未保存的修改先保存。没写提交信息、一个都没勾时在
    /// 状态栏说明，不提交。
    fn git_commit_picked(&mut self) {
        let message = self.git.commit.message.text().trim().to_owned();
        let Some(status) = self.git.status.as_ref() else {
            return;
        };
        let rebasing = status.operation == Some(Operation::Rebase);
        if message.is_empty() && !rebasing {
            self.flash(t!("git-commit-message-required"));
            self.focus = Focus::CommitMessage;
            return;
        }
        let checked: Vec<PathBuf> = (self.git.commit.picked_entries(status))
            .map(|e| e.path.clone())
            .collect();
        if !self.save_dirty_documents(&checked) {
            return;
        }
        let Some(status) = self.git.status.as_ref() else {
            return;
        };
        // 勾上的冲突文件中仍有冲突标记时不提交，否则标记和两侧的内容会一并进入提交；这里读取刚
        // 保存的磁盘内容。
        let unresolved = (self.git.commit.picked_entries(status))
            .filter(|e| e.conflicted() && winc_editor::git::merge::file_has_conflicts(&e.path))
            .count();
        if unresolved > 0 {
            self.flash(t!("git-commit-unresolved-conflicts", count = unresolved));
            return;
        }
        let state = &self.git.commit;
        let mut plan = PickedCommit::default();
        for entry in state.picked_entries(status) {
            match state.skipped.get(&entry.path).filter(|s| !s.is_empty()) {
                Some(skipped) => plan.partial.push((entry.path.clone(), skipped.clone())),
                None => {
                    plan.whole.push(entry.path.clone());
                    // 改名的文件，原来的路径一并提交，否则提交里同时留着新旧两个文件。
                    plan.whole.extend(entry.from.clone());
                }
            }
            if entry.untracked() {
                plan.added.push(entry.path.clone());
            }
        }
        let left = (status.entries.iter())
            .filter(|e| e.staged.is_some() && !e.conflicted() && !state.picked(e));
        plan.dropped = left
            .flat_map(|e| std::iter::once(e.path.clone()).chain(e.from.clone()))
            .collect();
        let amend = state.amend;
        // 修改上次提交、合并或变基暂停时只动当前仓库。
        let others = if amend || status.operation.is_some() {
            Vec::new()
        } else {
            self.other_plans()
        };
        let own = !(plan.whole.is_empty() && plan.partial.is_empty());
        if !own && others.is_empty() && !amend && !rebasing {
            self.flash(t!("git-commit-nothing-checked"));
            return;
        }
        // 变基停下来时：勾上的文件标记为已解决，继续变基（git 用原来的提交信息）。
        if rebasing {
            let paths = plan.whole;
            self.git_operate(t!("git-op-continue-rebase").to_owned(), move |git, root| {
                if !paths.is_empty() {
                    git.stage(root, &paths)?;
                }
                git.continue_operation(root, Operation::Rebase)
            });
            return;
        }
        let merging = status.operation.is_some();
        let committed: Vec<PathBuf> = others.iter().map(|(root, ..)| root.clone()).collect();
        self.git.commit.others_left = self.others_left(&committed);
        self.git.commit.committed_with = committed;
        let state = &self.git.commit;
        let options = CommitOptions {
            all: false,
            amend,
            sign_off: self.commit_signs_off(),
            no_verify: state.no_verify,
        };
        let sender = state.results.0.clone();
        self.git.commit.busy = true;
        self.run_git_job(move |git, root| {
            // 先提交当前仓库，再依次提交其他仓库勾上的文件；其中一个失败就停下，已经提交的保留。
            let work = |git: &Git| {
                let mut done = CommittedIn::default();
                if own || amend {
                    let oid = write_picked(git, root, (&message, options), &plan, merging)?;
                    done.add(root, oid);
                }
                for (other, plan, sign_off) in &others {
                    let options = CommitOptions {
                        amend: false,
                        sign_off: *sign_off,
                        ..options
                    };
                    let oid = write_picked(git, other, (&message, options), plan, false);
                    done.add(other, oid.map_err(|error| done.failed(other, &error))?);
                }
                Ok(done.first.unwrap_or_default())
            };
            let result = git.and_then(work);
            let _ = sender.send(Outcome::Committed(result, message, root.to_path_buf()));
        });
    }

    /// 其他仓库中勾上的文件各自的提交内容：仓库的根目录、要写入暂存区的内容、是否附加签署行。
    ///
    /// 换仓库时带过来的、只勾了一部分改动的文件只提交勾上的改动；有合并、变基等操作进行中的
    /// 仓库不在这里提交。
    fn other_plans(&self) -> Vec<(PathBuf, PickedCommit, bool)> {
        if self.other_checked_count() == 0 {
            return Vec::new();
        }
        let root = self.git.root.as_deref();
        let summaries = (self.git.summaries.lock()).unwrap_or_else(PoisonError::into_inner);
        let mut plans = Vec::new();
        for repo in &self.git.repositories {
            let Some(status) = summaries.get(repo).filter(|_| Some(repo.as_path()) != root) else {
                continue;
            };
            if status.operation.is_some() {
                continue;
            }
            let mut plan = PickedCommit::default();
            let skipped = &self.git.commit.other_skipped;
            for entry in &status.entries {
                let checked = self.other_checked(&entry.path) && !entry.conflicted();
                let partial = skipped.get(&entry.path).filter(|s| !s.is_empty());
                if let Some(partial) = partial.filter(|_| checked) {
                    plan.partial.push((entry.path.clone(), partial.clone()));
                } else if checked {
                    plan.whole.push(entry.path.clone());
                    plan.whole.extend(entry.from.clone());
                    if entry.untracked() {
                        plan.added.push(entry.path.clone());
                    }
                } else if entry.staged.is_some() && !entry.conflicted() {
                    plan.dropped.push(entry.path.clone());
                    plan.dropped.extend(entry.from.clone());
                }
            }
            if !(plan.whole.is_empty() && plan.partial.is_empty()) {
                let sign_off = self.state.sign_off_repos.contains(repo);
                plans.push((repo.clone(), plan, sign_off));
            }
        }
        plans
    }
}

/// 勾选式下一次提交要写入暂存区的内容。
#[derive(Debug, Default)]
struct PickedCommit {
    /// 整个提交的文件，连同改名文件原来的路径。
    whole: Vec<PathBuf>,
    /// 只提交一部分改动的文件，以及其中取消勾选的各处改动。
    partial: Vec<(PathBuf, Vec<Skip>)>,
    /// 其中未跟踪的文件：提交失败时移出暂存区，恢复为未跟踪。
    added: Vec<PathBuf>,
    /// 暂存区中已有内容而没有勾上的文件：提交前移出暂存区。
    dropped: Vec<PathBuf>,
}

/// 把 `plan` 写入暂存区并提交，返回新提交号的前 7 位。
///
/// `merging` 为真（合并、拣选、还原进行中）时暂存区里已有自动合并的结果，只加入勾上的文件，不把
/// 其他内容移出。提交失败时把刚加入的未跟踪文件移出暂存区。
fn write_picked(
    git: &Git,
    root: &Path,
    (message, options): (&str, CommitOptions),
    plan: &PickedCommit,
    merging: bool,
) -> Result<String, String> {
    if !merging && !plan.dropped.is_empty() {
        git.unstage(root, &plan.dropped)?;
    }
    if !plan.whole.is_empty() {
        git.stage(root, &plan.whole)?;
    }
    for (path, skipped) in &plan.partial {
        git.stage_content(root, path, partial_content(git, root, path, skipped)?)?;
    }
    let result = git.commit_with(root, message, options);
    if result.is_err() && !plan.added.is_empty() {
        let _ = git.unstage(root, &plan.added);
    }
    result
}

/// 返回文件 `path` 只提交 `skipped` 以外的改动时提交成的内容：`HEAD` 中的版本应用磁盘上其余的
/// 改动。
fn partial_content(
    git: &Git,
    root: &Path,
    path: &Path,
    skipped: &[Skip],
) -> Result<Vec<u8>, String> {
    use winc_core::diff::{hunk_key, line_hunks, lines, pick_changes};
    let name = winc_editor::display_name(path).into_owned();
    let head = git.head_content(root, path, &Cancel::default())?;
    let disk = winc_backend::fs::read(path)
        .map_err(|e| t!("git-read-file-failed", name = &name, error = e.to_string()))?;
    let texts = (
        String::from_utf8(head.unwrap_or_default()),
        String::from_utf8(disk),
    );
    let (Ok(head), Ok(disk)) = texts else {
        return Err(t!("git-commit-partial-not-utf8", name = name));
    };
    // 各处改动和取消勾选的记录一一对应（见 `assign`），内容相同的几处各按各的勾选提交。
    let (a, b) = (lines(&head), lines(&disk));
    let all: Vec<(u64, Range<usize>)> = (line_hunks(&head, &disk).into_iter())
        .map(|h| {
            let key = hunk_key(a[h.old.clone()].iter().copied(), b[h.new].iter().copied());
            (key, h.old)
        })
        .collect();
    let picks = picks::picks(skipped, &all);
    let content = pick_changes(&head, &disk, |key, old| {
        (all.iter().position(|(k, o)| *k == key && *o == old))
            .map_or(Pick::Apply, |h| picks[h].clone())
    });
    Ok(content.into_bytes())
}

/// 「提交并推送」「提交并同步」时把一并提交了的其他仓库 `others` 的当前分支依次推到各自的
/// 上游，没有上游时发布。只推送、不拉取。当前仓库这时已经推送了：一个仓库失败不影响其余的，
/// 最后的说明写明当前仓库已推送、哪些仓库失败及原因，由用户到那些仓库同步。
fn push_others(git: &Git, others: &[PathBuf]) -> Result<(), String> {
    let mut failed = Vec::new();
    for other in others {
        let upstream = [
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ];
        let tracked = git.operate(other, &upstream).is_ok();
        if let Err(error) = git.push(other, tracked) {
            failed.push(format!("{}: {error}", winc_editor::display_name(other)));
        }
    }
    if !failed.is_empty() {
        let failed = failed.join(t!("common-list-separator"));
        return Err(t!("git-push-others-failed", failed = failed));
    }
    Ok(())
}

/// 一次提交依次写入几个仓库时，已经提交了的：失败时的说明里写明哪些仓库已经提交，用户不会
/// 以为什么都没发生。
#[derive(Default)]
struct CommittedIn {
    /// 第一个新提交的提交号。
    first: Option<String>,
    /// 已经提交了的仓库的名字。
    names: Vec<String>,
}

impl CommittedIn {
    /// 记下仓库 `root` 已经提交，新提交号是 `oid`。
    fn add(&mut self, root: &Path, oid: String) {
        self.first.get_or_insert(oid);
        self.names
            .push(winc_editor::display_name(root).into_owned());
    }

    /// 仓库 `root` 提交失败（原因 `error`）时的说明：写明是哪个仓库，前面已经提交了的一并写出。
    fn failed(&self, root: &Path, error: &str) -> String {
        let name = winc_editor::display_name(root);
        if self.names.is_empty() {
            return format!("{name}: {error}");
        }
        let done = self.names.join(t!("common-list-separator"));
        t!(
            "git-commit-partial",
            done = done,
            name = name,
            error = error
        )
    }
}

#[cfg(test)]
mod tests;
