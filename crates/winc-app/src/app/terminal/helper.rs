//! 辅助进程 `winc-pty`：启动、重启和停止，同步面板尺寸和配色，收它发来的帧和回滚缓冲。

use super::*;
use crate::app::NoticeKind;

impl App {
    /// 把等着发出去的新尺寸发出去（上一个尺寸的帧回来了，或者等待时间已到）；返回下一次
    /// 需要唤醒的时刻。
    pub(crate) fn terminal_resize_tick(&mut self, now: Instant) -> Option<Instant> {
        let due = self.terminal.resize_due?;
        if now < due {
            return Some(due);
        }
        // 先清掉：面板收起了、辅助进程退出了时 `sync_terminal` 什么也不做就返回，过了点的时刻
        // 如果保留，每次都会返回它，使事件循环一直空转；需要继续等待时 `sync_terminal` 会重新
        // 记上。
        self.terminal.resize_due = None;
        self.sync_terminal();
        self.terminal.resize_due
    }

    /// 光标在空行行首藏够了（见 [`TermTab::settle`]）：画出来；返回下一次要醒来的时刻。
    pub(crate) fn terminal_settle_tick(&mut self, now: Instant) -> Option<Instant> {
        let tab = self.terminal.tab_mut()?;
        let due = tab.settle? + SETTLE;
        if now < due {
            return Some(due);
        }
        tab.settle = None;
        self.request_redraw();
        None
    }

    /// 开始、停止拖分隔条：拖的时候正在看的标签先把网格上面一屏的历史行取来（`TermTab::ahead`）。
    pub(crate) fn terminal_dragging(&mut self, dragging: bool) {
        if let Some(tab) = self.terminal.tab_mut() {
            tab.ahead = dragging;
        }
        if dragging {
            self.fetch_terminal_history();
        }
    }

    /// 辅助进程意外退出了：标签和主程序这边的最后一屏留着，标签栏上写「终端组件已退出」，按 ↩
    /// 或点「重新启动」重来（[`App::restart_terminal_tab`]）。
    pub(in crate::app) fn terminal_helper_gone(&mut self) {
        let t = &mut self.terminal;
        t.client = None;
        t.ready = false;
        t.find = None;
        t.errors = None;
        t.link = None;
        t.requests.clear();
        t.resize_due = None;
        if self.focus == Focus::TerminalFind {
            self.focus = Focus::Terminal;
        }
        let t = &mut self.terminal;
        t.crashed = !t.tabs.is_empty();
        if !t.crashed {
            t.error = Some(winc_i18n::t!("terminal-helper-exited").into());
        }
    }

    /// 重新开始正在看的标签需不需要先确认：伪终端断开时里面的程序可能还在运行，重新开始会结束
    /// 它们；是否正在运行不一定查得到（Windows 上没有前台进程），所以一律确认。
    pub(in crate::app) fn terminal_restart_needs_confirm(&self) -> bool {
        let t = &self.terminal;
        self.window.handle.is_some()
            && !t.crashed
            && self.config.terminal_confirm_close != Some(false)
            && t.tab()
                .is_some_and(|tab| tab.ended == Some(TabEnd::Disconnected))
    }

    /// 用户要重新开始（点按钮、按 ↩）：需要时先确认（[`App::terminal_restart_needs_confirm`]）。
    pub(in crate::app) fn request_terminal_restart(&mut self) {
        let Some(id) = (self.terminal_restart_needs_confirm())
            .then(|| self.terminal.tab().map(|tab| tab.id))
            .flatten()
        else {
            self.restart_terminal_tab();
            return;
        };
        let message = winc_i18n::t!("terminal-restart-confirm");
        let detail = winc_i18n::t!("terminal-restart-confirm-detail");
        let action = winc_i18n::t!("terminal-restart-confirm-action");
        self.show_confirm(message, detail, action, move |ok| {
            ok.then_some(UserEvent::TerminalRestart(id))
        });
    }

    /// 确认框上确认了重新开始编号是 `id` 的标签。
    pub(crate) fn confirmed_terminal_restart(&mut self, id: u16) {
        if let Some(index) = self.terminal.index_of(id) {
            self.terminal.active = index;
            self.restart_terminal_tab();
        }
    }

    /// 重新开始：辅助进程退出了时所有标签在各自最后的目录里重开；shell 退出了或者伪终端断开了时
    /// 只重开正在看的这一个，还在原来的位置。
    pub(in crate::app) fn restart_terminal_tab(&mut self) {
        // 正在改的名字先定下来：重开以后标签换了编号，输入框找不到它了。
        self.finish_terminal_rename(true);
        if self.terminal.crashed {
            self.recover_crashed_terminal();
            return;
        }
        let t = &mut self.terminal;
        let index = t.active;
        let count = t.tabs.len();
        let Some(tab) = t.tabs.get(index) else {
            return;
        };
        if let Some(config) = tab.run.clone() {
            self.start_run(config);
            return;
        }
        let (cwd, name, old, shell) =
            (tab.cwd.clone(), tab.name.clone(), tab.id, tab.shell.clone());
        // 先建新的再关旧的：关掉最后一个标签时辅助进程跟着退出（见 `close_terminal_tab_now`），
        // 只有这一个标签时不必为了重开换一个辅助进程。
        self.new_terminal_tab_with(cwd, shell);
        self.close_terminal_now(old);
        let t = &mut self.terminal;
        // 新标签排在最后，挪回原来的位置，名字不变（辅助进程还没准备好、标签还没建出来时名字
        // 跟着等着建的标签走，位置不挪）。
        if t.tabs.len() == count {
            let mut tab = t.tabs.pop().expect("刚建的");
            tab.name = name;
            t.tabs.insert(index, tab);
            t.active = index;
        } else if let Some(at) = t.want.len().checked_sub(1) {
            t.want_names.resize(at, String::new());
            t.want_names.push(name);
        }
    }

    /// 关掉终端模块：辅助进程连同里面的 shell 一起结束，面板收起。
    ///
    /// 保留面板高度和检测到的 shell；后者与面板无关，设置中的「默认 Shell」菜单仍需列出。
    pub(crate) fn stop_terminal(&mut self) {
        self.terminal = TerminalPanel {
            height: self.terminal.height,
            shells: std::mem::take(&mut self.terminal.shells),
            shells_detected: self.terminal.shells_detected,
            ..TerminalPanel::default()
        };
        if self.focus.in_terminal() {
            self.focus = Focus::Editor;
        }
        self.relayout();
        self.request_redraw();
    }

    /// 主题变了：告诉辅助进程，程序再来问颜色时答新的。
    pub(crate) fn terminal_theme_changed(&mut self) {
        if let Some(client) = &self.terminal.client {
            client.send(&ToHelper::Theme(theme_colors(self.palette)));
        }
    }

    /// 网格放得下几列几行，以及像素尺寸（物理像素，程序查询时回答它）。
    pub(in crate::app) fn terminal_size(&self) -> Size {
        let grid = grid_rect(
            self.terminal_content_card(),
            self.fonts.terminal_line_height,
        );
        let cols = (grid.w / self.metrics.terminal_cell).floor().max(2.0);
        let rows = (grid.h / self.fonts.terminal_line_height).floor().max(1.0);
        let scale = self
            .frame
            .renderer
            .as_ref()
            .map_or(1.0, winc_render::Renderer::scale_factor);
        Size {
            cols: cols.min(f32::from(u16::MAX)) as u16,
            rows: rows.min(f32::from(u16::MAX)) as u16,
            width: (cols * self.metrics.terminal_cell * scale) as u16,
            height: (rows * self.fonts.terminal_line_height * scale) as u16,
        }
    }

    /// 排版之后：要新建标签时建；尺寸变了告诉每个标签；往上翻着缺历史行时取回来。
    pub(in crate::app) fn sync_terminal(&mut self) {
        if !self.terminal.open || self.layout.terminal.h <= 0.0 {
            return;
        }
        let size = self.terminal_size();
        let slack = grid_slack(self.layout.terminal, self.fonts.terminal_line_height);
        for tab in &mut self.terminal.tabs {
            tab.shown = usize::from(size.rows);
            tab.slack = slack;
        }
        if !self.terminal.ready {
            return;
        }
        // 要建新标签：建好以后看的是别的标签，原来那个先失去焦点（1004）。
        let waiting = !self.terminal.want.is_empty() || !self.terminal.run_queue.is_empty();
        if self.terminal.error.is_none() && waiting {
            self.blur_terminal_tab();
        }
        // 只有新建标签时才用到；每次重绘都会运行到这里，不必每次都读取配置和拼接路径。
        // 待建标签的目录中有尚未询问过的 Python 环境时先询问，回答之后再建。
        let python = (!self.terminal.want.is_empty())
            .then(|| self.python_choices())
            .flatten();
        let spawn = python.map(|python| {
            (
                self.terminal_cwd(),
                self.config.terminal_shell.clone(),
                integration_dir(&self.config),
                python,
            )
        });
        let scrollback = self.config.terminal_scrollback.unwrap_or(SCROLLBACK);
        let prompt = self.config.terminal_prompt != Some(false);
        // 拆分过的标签：各终端按自己那一块的大小。
        let count = self.terminal.tab().map_or(1, TermTab::pane_count);
        let sizes: Vec<(u16, Size)> = (0..count)
            .filter_map(|at| {
                self.with_terminal_pane(at, |app| {
                    let id = app.terminal.tab()?.id;
                    Some((id, app.terminal_size()))
                })
            })
            .collect();
        let t = &mut self.terminal;
        let Some(client) = &t.client else {
            return;
        };
        // 每个标签同时只有一个尺寸在路上：上一个尺寸重排好的一帧回来后发下一个（中间的跳过），
        // 辅助进程和 shell 不会积压；只改正在看的标签，改宽度要重排整个回滚缓冲（一万行约 11 ms），
        // 后台标签一起排会阻塞几十毫秒，切过来时再改。
        let now = Instant::now();
        t.resize_due = None;
        let active = t.active;
        let mut due = None;
        if let Some(tab) = t.tabs.get_mut(active) {
            let mut resize = |tab: &mut TermTab| {
                let size = (sizes.iter())
                    .find(|(id, _)| *id == tab.id)
                    .map_or(size, |(_, size)| *size);
                tab.shown = usize::from(size.rows);
                if tab.size == size || tab.ended.is_some() {
                    return;
                }
                match tab.resizing {
                    Some(sent) if now < sent + RESIZE_WAIT => {
                        let at = sent + RESIZE_WAIT;
                        due = Some(due.map_or(at, |d: Instant| d.min(at)));
                    }
                    _ => {
                        tab.size = size;
                        tab.resizing = Some(now);
                        client.send(&ToHelper::Resize { tab: tab.id, size });
                    }
                }
            };
            resize(tab);
            if let Some(split) = &mut tab.split {
                split.others.iter_mut().for_each(&mut resize);
            }
        }
        t.resize_due = due;
        if let Some((cwd, shell, integration, python)) = spawn.filter(|_| t.error.is_none()) {
            let names = std::mem::take(&mut t.want_names);
            let shells = std::mem::take(&mut t.want_shells);
            let intos = std::mem::take(&mut t.want_into);
            // 等着送的输入给建好以后要看的那个标签：新建的（排在最后），或者重开时原来看的那个。
            let target = t
                .want_active
                .unwrap_or(t.tabs.len() + t.want.len().saturating_sub(1));
            for (i, want) in std::mem::take(&mut t.want).into_iter().enumerate() {
                t.next_id = t.next_id.wrapping_add(1);
                let id = t.next_id;
                let dir = if want.is_empty() { cwd.clone() } else { want };
                let chosen = shells.get(i).cloned().flatten();
                // 设置中的默认 shell 是本机的命令：目录在 WSL 发行版中时不适用，新终端进入该发行版。
                let shell = match default_shell_applies(&dir, cfg!(windows)) {
                    true => shell.clone(),
                    false => None,
                };
                let python = python.get(i).copied().unwrap_or(false);
                let shell = chosen.as_deref().or(shell.as_deref());
                let launch = launch(shell, dir.clone(), cfg!(windows));
                client.send(&ToHelper::Spawn {
                    tab: id,
                    program: launch.program,
                    args: launch.args,
                    login: launch.login,
                    cwd: launch.cwd,
                    env: environment(prompt, python),
                    scrollback,
                    size,
                    integration: integration.clone(),
                    banner: Vec::new(),
                });
                if t.tabs.len() == target
                    && let Some(bytes) = t.pending_input.take()
                {
                    client.send(&ToHelper::Input {
                        tab: id,
                        bytes,
                        rest: 0,
                    });
                }
                let mut tab = TermTab::new(id, size);
                tab.slack = slack;
                tab.name = names.get(i).cloned().unwrap_or_default();
                tab.shell = chosen;
                // shell 报告当前目录之前先记下启动目录：此时重启标签或保存现场仍回到这里，
                // 远端的终端不会因目录为空而换成本机的 shell。
                tab.cwd = dir;
                // 拆分出的终端加进原来的标签，排在前台终端的后面并成为前台。
                let into = (intos.get(i).copied().flatten()).and_then(|into| t.index_of(into));
                match into {
                    Some(index) => {
                        let (mut all, at) = super::take_group(&mut t.tabs, index);
                        all.insert(at + 1, tab);
                        super::put_group(&mut t.tabs, index, all, at + 1);
                        t.active = index;
                    }
                    None => {
                        t.tabs.push(tab);
                        t.active = t.tabs.len() - 1;
                    }
                }
                t.focus_sent = false;
            }
            if let Some(active) = t.want_active.take() {
                t.active = active.min(t.tabs.len().saturating_sub(1));
            }
        }
        let mut replaced = Vec::new();
        if t.error.is_none() {
            for start in std::mem::take(&mut t.run_queue) {
                t.next_id = t.next_id.wrapping_add(1);
                let id = t.next_id;
                client.send(&ToHelper::Spawn {
                    tab: id,
                    program: start.launch.program,
                    args: start.launch.args,
                    login: start.launch.login,
                    cwd: start.launch.cwd,
                    env: start.env,
                    scrollback,
                    size,
                    integration: None,
                    banner: start.banner,
                });
                let mut tab = TermTab::new(id, size);
                tab.slack = slack;
                tab.name.clone_from(&start.config.name);
                tab.run = Some(start.config);
                t.tabs.push(tab);
                t.active = t.tabs.len() - 1;
                t.focus_sent = false;
                replaced.extend(start.replaces.map(|old| (id, old)));
            }
        }
        self.retire_run_tabs(replaced);
        self.drop_other_tab_state();
        self.fetch_terminal_history();
    }

    /// 新终端的工作目录：打开的目录，没有时是家目录（辅助进程按空路径处理）。
    fn terminal_cwd(&self) -> Vec<u8> {
        self.editor
            .root()
            .map(|root| os_bytes(root.as_os_str()))
            .unwrap_or_default()
    }

    /// 返回各个待建标签是否加载启动目录中的 Python 环境；有环境尚未询问过时弹出提示并返回
    /// `None`，回答之后再建。
    ///
    /// 环境里的程序排在 `PATH` 最前，会先于系统中的同名命令运行，因此每个环境只在用户同意后加载；
    /// 回答按环境的目录记录（见 [`App::answered_python_environment`]）。没有窗口（测试）时无法
    /// 询问，按同意处理。
    pub(in crate::app) fn python_choices(&mut self) -> Option<Vec<bool>> {
        let count = self.terminal.want.len();
        if self.config.terminal_shell_integration == Some(false) {
            return Some(vec![false; count]);
        }
        if self.terminal.python_asking.is_some() {
            return None;
        }
        let root = self.terminal_cwd();
        let mut choices = Vec::with_capacity(count);
        for want in &self.terminal.want {
            let dir = bytes_path(if want.is_empty() { &root } else { want });
            // 没有打开目录时终端从用户的家目录启动，那里的环境是用户自己的，不询问。
            if dir.as_os_str().is_empty() {
                choices.push(true);
                continue;
            }
            let candidates = [".venv", "venv"].map(|name| dir.join(name));
            let found = |venv: &PathBuf| winc_backend::fs::is_file(venv.join("pyvenv.cfg"));
            let venv = candidates.into_iter().find(found);
            let Some(venv) = venv else {
                choices.push(true);
                continue;
            };
            // 只有同意被长期记录（记为 1）；拒绝只在本次运行中有效，不会永久关闭这个环境的加载。
            if self.terminal.python_declined.contains(&venv) {
                choices.push(false);
            } else if self.state.trusted_repo(&venv) == Some(1) || self.window.handle.is_none() {
                choices.push(true);
            } else {
                self.ask_python_environment(venv);
                return None;
            }
        }
        Some(choices)
    }

    /// 询问是否为终端加载 Python 环境 `venv`。
    fn ask_python_environment(&mut self, venv: PathBuf) {
        let message = winc_i18n::t!("terminal-python-confirm");
        let path = venv.display().to_string();
        let detail = winc_i18n::t!("terminal-python-confirm-detail", path = path);
        use crate::app::lifecycle::alert::AlertButton;
        let buttons = vec![
            AlertButton::new(winc_i18n::t!("terminal-python-confirm-action")),
            AlertButton::cancel(winc_i18n::t!("terminal-python-confirm-decline")),
        ];
        let asked = venv.clone();
        let shown = self.show_alert(message, &detail, buttons, None, move |index, _| {
            Some(UserEvent::PythonEnvironment(asked.clone(), index == 0))
        });
        // 提示框暂时无法弹出（已有别的提示框）时不记录，下一次排版再问。
        if shown {
            self.terminal.python_asking = Some(venv);
        }
    }

    /// 提示框中回答了是否加载 Python 环境 `venv`，然后建等着的标签。
    ///
    /// 同意长期记录，同一个环境不再询问；拒绝只记到本次运行结束。
    pub(crate) fn answered_python_environment(&mut self, venv: PathBuf, load: bool) {
        if load {
            self.terminal
                .python_declined
                .retain(|declined| *declined != venv);
            self.update_state(|state| state.trust_repo(&venv, 1));
        } else if !self.terminal.python_declined.contains(&venv) {
            self.terminal.python_declined.push(venv);
        }
        self.terminal.python_asking = None;
        self.relayout();
        self.request_redraw();
    }

    /// 读线程叫醒了事件循环：把收到的消息取完。
    ///
    /// 只有消息改变了界面上的内容时才重绘：后台标签和收起的面板持续发送标题、进程、命令块时不重绘
    /// 窗口。提示通过 `notify`、`flash` 显示，它们自行请求重绘。
    pub(in crate::app) fn poll_terminal(&mut self) {
        let before = self.terminal.visible_state();
        let mut shown = false;
        loop {
            let Some(client) = &self.terminal.client else {
                self.request_redraw();
                return;
            };
            match client.poll() {
                Poll::Message(message) => {
                    shown |= self.terminal.message_is_shown(&message);
                    self.terminal_message(message);
                }
                Poll::Empty => break,
                Poll::Closed => {
                    self.terminal_helper_gone();
                    shown = true;
                    break;
                }
            }
        }
        if shown || self.terminal.visible_state() != before {
            self.request_redraw();
        }
    }

    /// 处理辅助进程发来的一条消息。
    pub(in crate::app) fn terminal_message(&mut self, message: FromHelper) {
        let t = &mut self.terminal;
        match message {
            FromHelper::Hello { magic, version, .. } => {
                if magic == MAGIC && version == VERSION {
                    t.ready = true;
                    self.relayout();
                } else {
                    t.client = None;
                    t.error = Some(winc_i18n::t!("terminal-helper-version-mismatch").into());
                }
            }
            FromHelper::Frame { tab: id, frame } => {
                let full = frame.full;
                let mut resized = false;
                if let Some(tab) = t.by_id(id) {
                    // 等的那个尺寸的帧回来了：如果面板在这期间又变了，下一个尺寸马上发。
                    if tab.resizing.is_some()
                        && (frame.cols, frame.rows) == (tab.size.cols, tab.size.rows)
                    {
                        tab.resizing = None;
                        resized = true;
                    }
                    tab.apply(frame);
                    // 按这一帧的行号算的块先到了：现在换上。
                    if let Some(blocks) = tab.early_blocks.take() {
                        tab.set_blocks(blocks);
                    }
                    tab.credit_due = true;
                }
                self.terminal_find_output(id);
                let t = &mut self.terminal;
                if resized {
                    t.resize_due = Some(Instant::now());
                }
                if full {
                    self.terminal_renumbered(id);
                }
                self.fetch_terminal_history();
            }
            FromHelper::History {
                tab: id,
                id: request,
                start,
                lines,
                more,
            } => {
                let Some(at) = t.requests.iter().position(|r| r.0 == request) else {
                    return;
                };
                // 分几条回来的：最后一条才算这个请求结束。
                let purpose = if more {
                    t.requests[at].2
                } else {
                    t.requests.remove(at).2
                };
                match purpose {
                    Request::Find => {
                        let done = !t.waiting(id, Request::Find);
                        self.terminal_find_loaded(id, start, lines, done);
                    }
                    Request::Errors => {
                        // 分几段要的：都回来了才一起认，否则只认了第一段。
                        let Some(nav) = t.errors.as_mut().filter(|n| n.tab == id) else {
                            return;
                        };
                        nav.got.push((start, lines));
                        if !t.waiting(id, Request::Errors) {
                            let got = t.errors.as_mut().map(|n| std::mem::take(&mut n.got));
                            let got = got.unwrap_or_default();
                            let from = got.first().map_or(start, |g| g.0);
                            let lines = got.into_iter().flat_map(|g| g.1).collect();
                            self.terminal_errors_loaded(id, from, lines);
                        }
                    }
                    Request::View | Request::Copy => {
                        if let Some(tab) = t.by_id(id) {
                            tab.store_history(start, lines);
                        }
                        // 选区要的行都回来了：复制这个标签的选区。还缺的是辅助进程那边已经
                        // 不再保留的，跳过即可。
                        if purpose == Request::Copy && !t.waiting(id, Request::Copy) {
                            let text = t.by_id(id).and_then(|tab| {
                                let text = tab.selected_text_lossy();
                                if std::mem::take(&mut tab.clear_after_copy) {
                                    tab.selection = None;
                                }
                                text
                            });
                            if let Some(text) = text {
                                self.set_clipboard(text, false);
                                self.flash(winc_i18n::t!("terminal-selection-copied"));
                            }
                        }
                    }
                }
            }
            FromHelper::Event { tab: id, event } => match event {
                Event::Title(title) => {
                    if let Some(tab) = t.by_id(id) {
                        tab.title = title;
                    }
                }
                // 只接受正在看的标签写剪贴板：看不见的标签里的输出不能在用户不知情时改写它。
                Event::Clipboard(text) => {
                    if t.open && t.index_of(id) == Some(t.active) {
                        self.set_clipboard(text, false);
                    }
                }
                Event::Link(link, uri) => {
                    if let Some(tab) = t.by_id(id) {
                        tab.set_link(link, uri);
                    }
                }
                Event::Cwd(path) => {
                    let path = local_cwd(self.editor.root(), path);
                    if let Some(tab) = t.by_id(id) {
                        tab.cwd = path;
                    }
                }
                Event::Bell => self.terminal_alert(id, None),
                Event::Notify { title, body } => {
                    let message = match (title.is_empty(), body.is_empty()) {
                        (false, false) => {
                            winc_i18n::t!("terminal-notification", title = title, body = body)
                        }
                        (true, _) => body,
                        (false, true) => title,
                    };
                    self.terminal_alert(id, Some(message));
                }
                // 命令边界由辅助进程切成块再发（`Blocks`）。
                Event::Command(_) => {}
            },
            FromHelper::Blocks {
                tab: id,
                blocks,
                early,
            } => self.terminal_blocks(id, blocks, early),
            FromHelper::Process { tab: id, name } => {
                if let Some(tab) = t.by_id(id) {
                    tab.process = name;
                }
            }
            FromHelper::Exited { tab: id, code } => {
                let mut again = None;
                self.run_tab_exited(id, code);
                let t = &mut self.terminal;
                if let Some(tab) = t.by_id(id) {
                    tab.ended = Some(TabEnd::Exited(code));
                    if std::mem::take(&mut tab.rerun) {
                        again.clone_from(&tab.run);
                    }
                }
                if let Some(config) = again {
                    self.start_run(config);
                    return;
                }
                // shell 正常退出（`exit`、⌃D）时关掉标签，不留一个等回车的空标签；异常退出的
                // 和运行配置的标签留着，用户要看退出码和输出。
                // 拆分出的终端也一样，只关它自己。
                let shell = (self.terminal.tabs.iter().flat_map(TermTab::panes))
                    .any(|tab| tab.id == id && tab.run.is_none());
                if code == 0 && shell {
                    self.close_terminal_now(id);
                }
            }
            FromHelper::Disconnected { tab: id } => {
                // 面板收着、在看别的标签时看不到标签栏上的说明：提示一声，标签上留红点。
                let seen = t.open && t.index_of(id) == Some(t.active);
                if let Some(tab) = t.by_id(id) {
                    tab.ended = Some(TabEnd::Disconnected);
                    if !seen {
                        tab.unseen = Some(true);
                    }
                }
                let message = (!seen).then(|| winc_i18n::t!("terminal-disconnected").to_string());
                self.terminal_alert(id, message);
            }
            FromHelper::Failed { tab: id, message } => {
                // 只是这个终端起不来（shell 不存在等）：说一声，其他标签和新开的终端不受影响。
                if let Some((index, gone)) = t.remove_terminal(id) {
                    self.forget_terminal_tab(id);
                    let t = &mut self.terminal;
                    if gone && (t.active > index || t.active >= t.tabs.len()) {
                        t.active = t.active.saturating_sub(1);
                    }
                    if t.tabs.is_empty() {
                        self.set_terminal_open(false);
                        self.focus = Focus::Editor;
                        self.relayout();
                    }
                }
                self.notify(NoticeKind::Error, message);
            }
        }
    }
}

/// 辅助进程的路径：环境变量 `WINC_PTY` 指定的，否则是和主程序放在一起的 `winc-pty`（Windows 上
/// 是 `winc-pty.exe`；安装包中是 `Contents/MacOS/winc-pty`，开发时是 `target/debug/winc-pty`）。
// 测试用 sh 脚本冒充辅助进程。
#[cfg(all(test, unix))]
mod tests;
