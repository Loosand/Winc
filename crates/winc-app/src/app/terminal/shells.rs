//! 新建终端时可选的 shell：「+」右侧的下拉菜单，列出终端所在主机上存在的 shell，默认 shell
//! 排在最前。打开的目录在远端主机上时列出那台主机的 shell（[`remote_shells`]），否则是本机的。
//!
//! 首次启动终端时在后台线程检测：Windows 上列出 WSL 发行版需要启动 `wsl.exe`，不能占用界面线程。
//! 启动命令按空白分词，双引号括起的部分不拆开（[`command_words`]），含空白的路径加上双引号。

#![allow(
    clippy::disallowed_methods,
    reason = "只访问本机：列出本机安装的 shell；远端的由连接上的命令列出"
)]

use std::path::PathBuf;

use super::HeaderHit;
use crate::app::input::context_menu::{MenuButton, MenuTarget, menu_below};
use crate::app::{App, UserEvent};

/// 列出的 Unix shell，按这个顺序排在默认 shell 后面。
///
/// `sh`、`csh` 等虽然登记在 `/etc/shells`，但很少用作交互 shell，需要时在 `terminal.shell` 中设置。
const UNIX_SHELLS: &[&str] = &["zsh", "bash", "fish", "nu", "pwsh"];

/// `/etc/shells` 之外还要找的目录：用包管理器安装的 shell 不一定登记在那里。
const UNIX_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"];

/// 菜单里的一个 shell。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShellChoice {
    /// 显示的名字。
    pub name: String,
    /// 启动命令，空白分隔程序和参数。
    pub command: String,
}

/// 把启动命令 `command` 按空白拆成程序和参数；双引号括起的部分（如含空格的路径）不拆开，
/// 引号去掉。
///
/// 反斜杠不作转义，Windows 路径中的 `\` 原样保留。
pub(super) fn command_words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    // `started`：当前词已经开始，空的引号 `""` 也算一个词。
    let (mut quoted, mut started) = (false, false);
    for c in command.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if std::mem::take(&mut started) {
                    words.push(std::mem::take(&mut word));
                }
            }
            c => {
                word.push(c);
                started = true;
            }
        }
    }
    if started {
        words.push(word);
    }
    words
}

/// 返回能写入启动命令的 `word`：含空白时加上双引号。
fn quoted(word: &str) -> String {
    if word.contains(char::is_whitespace) {
        format!("\"{word}\"")
    } else {
        word.to_owned()
    }
}

/// 返回命令 `command` 的程序名：去掉目录和 `.exe`。
fn program_name(command: &str) -> String {
    let words = command_words(command);
    let program = words.first().map_or("", String::as_str);
    let file = program.rsplit(['/', '\\']).next().unwrap_or(program);
    file.strip_suffix(".exe").unwrap_or(file).to_owned()
}

/// 返回 Unix 上的 shell：`login` 是默认 shell，`list` 是 `/etc/shells` 的内容。
fn unix_shells(list: &str, login: Option<&str>, exists: impl Fn(&str) -> bool) -> Vec<ShellChoice> {
    let listed = list
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('/'));
    let listed: Vec<&str> = listed.collect();
    let choice = |path: &str| {
        let command = quoted(path);
        ShellChoice {
            name: program_name(&command),
            command,
        }
    };
    let mut shells: Vec<ShellChoice> = login
        .filter(|path| path.starts_with('/') && exists(path))
        .map(choice)
        .into_iter()
        .collect();
    for name in UNIX_SHELLS {
        if shells.iter().any(|shell| shell.name == *name) {
            continue;
        }
        let extra = UNIX_DIRS.iter().map(|dir| format!("{dir}/{name}"));
        let found = (listed.iter().copied())
            .filter(|path| program_name(path) == *name)
            .map(str::to_owned)
            .chain(extra)
            .find(|path| exists(path));
        shells.extend(found.as_deref().map(choice));
    }
    shells
}

/// 返回 Windows 上的 shell：`has` 判断程序是否在 `PATH` 中，`git_bash` 是 Git Bash 的路径，
/// `distros` 是 WSL 发行版。
///
/// 前三个的顺序与辅助进程选默认 shell 的顺序相同：`pwsh`、`powershell`、`cmd`。Git Bash 以登录、
/// 交互方式启动（`--login -i`），读取 `/etc/profile` 和用户的配置。
#[cfg(any(windows, test))]
fn windows_shells(
    has: impl Fn(&str) -> bool,
    git_bash: Option<&str>,
    distros: &[String],
) -> Vec<ShellChoice> {
    let programs = ["pwsh", "powershell", "cmd"].into_iter();
    let programs = programs.filter(|name| has(&format!("{name}.exe")));
    let programs = programs.map(|name| ShellChoice {
        name: name.to_owned(),
        command: format!("{name}.exe"),
    });
    let git_bash = git_bash.map(|path| ShellChoice {
        name: "Git Bash".to_owned(),
        command: format!("{} --login -i", quoted(path)),
    });
    let distros = distros.iter().map(|distro| ShellChoice {
        name: format!("{distro} (WSL)"),
        command: format!("wsl.exe -d {}", quoted(distro)),
    });
    programs.chain(git_bash).chain(distros).collect()
}

/// Git for Windows 的默认安装目录，每项为（环境变量，其下的相对路径）：全局安装（64 位、
/// 32 位）、当前用户安装和 Scoop。
#[cfg(any(windows, test))]
const GIT_DIRS: [(&str, &str); 5] = [
    ("ProgramW6432", "Git"),
    ("ProgramFiles", "Git"),
    ("ProgramFiles(x86)", "Git"),
    ("LocalAppData", r"Programs\Git"),
    ("UserProfile", r"scoop\apps\git\current"),
];

/// 返回 Git Bash（Git for Windows 安装目录下的 `bin\bash.exe`）的路径；找不到时返回 `None`。
///
/// `path` 是 `PATH` 中的目录，`var` 读取环境变量，`exists` 判断文件是否存在。先查 `PATH` 中
/// 含 `git.exe` 的 `cmd` 目录（安装程序将 `<安装目录>\cmd` 加入 `PATH`），以找到自定义位置的
/// 安装；再查 [`GIT_DIRS`] 中的默认安装目录。
#[cfg(any(windows, test))]
fn git_bash(
    path: &[String],
    var: impl Fn(&str) -> Option<String>,
    exists: impl Fn(&str) -> bool,
) -> Option<String> {
    let on_path = path.iter().filter_map(|dir| {
        let dir = dir.trim_end_matches('\\');
        let (parent, name) = dir.rsplit_once('\\')?;
        let git = name.eq_ignore_ascii_case("cmd") && exists(&format!(r"{dir}\git.exe"));
        git.then(|| parent.to_owned())
    });
    let defaults = GIT_DIRS
        .into_iter()
        .filter_map(|(key, dir)| Some(format!(r"{}\{dir}", var(key)?)));
    let mut bash = on_path
        .chain(defaults)
        .map(|dir| format!(r"{dir}\bin\bash.exe"));
    bash.find(|bash| exists(bash))
}

/// 解析 `wsl.exe -l -q` 的输出（UTF-16 小端）。
///
/// 不列出 `docker-desktop` 开头的内部发行版。
#[cfg(any(windows, test))]
fn parse_wsl_list(output: &[u8]) -> Vec<String> {
    let (units, _) = output.as_chunks::<2>();
    let units: Vec<u16> = units.iter().map(|&unit| u16::from_le_bytes(unit)).collect();
    let text = String::from_utf16_lossy(&units);
    let names = text
        .lines()
        .map(|line| line.trim_matches(['\u{feff}', '\0', ' ', '\r']));
    names
        .filter(|name| !name.is_empty() && !name.starts_with("docker-desktop"))
        .map(str::to_owned)
        .collect()
}

/// 检测本机的 shell。
#[cfg(not(windows))]
fn detect() -> Vec<ShellChoice> {
    let list = std::fs::read_to_string("/etc/shells").unwrap_or_default();
    let login = std::env::var("SHELL").ok();
    let exists = |path: &str| std::path::Path::new(path).is_file();
    unix_shells(&list, login.as_deref(), exists)
}

/// 检测本机的 shell。
#[cfg(windows)]
fn detect() -> Vec<ShellChoice> {
    use std::os::windows::process::CommandExt;
    /// `CREATE_NO_WINDOW`：图形程序启动命令行程序时不弹出控制台窗口。
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let path = std::env::var_os("PATH").unwrap_or_default();
    let dirs: Vec<_> = std::env::split_paths(&path).collect();
    let has = |name: &str| dirs.iter().any(|dir| dir.join(name).is_file());
    let dirs: Vec<String> = dirs
        .iter()
        .map(|dir| dir.to_string_lossy().into_owned())
        .collect();
    let exists = |path: &str| std::path::Path::new(path).is_file();
    let git_bash = git_bash(&dirs, |key| std::env::var(key).ok(), exists);
    let output = std::process::Command::new("wsl.exe")
        .args(["-l", "-q"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let distros = match output {
        Ok(output) if output.status.success() => parse_wsl_list(&output.stdout),
        _ => Vec::new(),
    };
    windows_shells(has, git_bash.as_deref(), &distros)
}

/// 记录的 shell 是否仍然可用：绝对路径的程序已卸载时，恢复现场改用默认 shell，避免启动失败。
pub(super) fn still_usable(command: &str) -> bool {
    let words = command_words(command);
    let program = words.first().map_or("", String::as_str);
    !program.starts_with('/') || cfg!(windows) || std::path::Path::new(program).is_file()
}

impl App {
    /// 第一次启动终端或打开设置时在后台检测本机的 shell，结果由事件
    /// [`UserEvent::TerminalShells`] 送回。
    pub(in crate::app) fn detect_terminal_shells(&mut self) {
        if std::mem::replace(&mut self.terminal.shells_detected, true) {
            return;
        }
        let Some(proxy) = self.link.proxy.clone() else {
            return;
        };
        std::thread::spawn(move || {
            let _ = proxy.send_event(UserEvent::TerminalShells(detect()));
        });
    }

    /// 返回配置的 `terminal.shell`，未设置时返回 `None`。
    ///
    /// 命令与检测到的某个 shell 相同时沿用其名称（如「Git Bash」）。
    pub(crate) fn configured_shell(&self) -> Option<ShellChoice> {
        let command = self.config.terminal_shell.as_deref()?;
        let command = command.trim();
        if command.is_empty() {
            return None;
        }
        let known = self
            .terminal
            .shells
            .iter()
            .find(|shell| shell.command == command);
        Some(ShellChoice {
            name: known.map_or_else(|| program_name(command), |shell| shell.name.clone()),
            command: command.to_owned(),
        })
    }

    /// 菜单里的 shell：设置了 `terminal.shell` 时它是默认的，排在最前面。
    pub(crate) fn terminal_shell_list(&self) -> Vec<ShellChoice> {
        let first = self.configured_shell();
        let name = first.as_ref().map(|shell| shell.name.clone());
        let others = (self.terminal.shells.iter())
            .filter(|shell| Some(&shell.name) != name.as_ref())
            .cloned();
        first.into_iter().chain(others).collect()
    }

    /// 打开「+」右边的 shell 菜单。
    pub(in crate::app) fn open_terminal_shell_menu(&mut self) {
        let menu = (self.hits.terminal.iter()).find(|(_, hit)| *hit == HeaderHit::ShellMenu);
        let Some(&(menu, _)) = menu else {
            return;
        };
        let at = menu_below(menu);
        self.open_button_menu(MenuButton::TerminalShells, MenuTarget::TerminalShells, at);
    }

    /// 用菜单里第 `index` 个 shell 新建一个终端标签。
    pub(in crate::app) fn new_terminal_with_shell(&mut self, index: usize) {
        if let Some(shell) = self.terminal_shell_list().into_iter().nth(index) {
            self.new_terminal_tab_with(Vec::new(), Some(shell.command));
        }
    }
}

/// 字符 `c` 在 shell 中是否不需要引号。
pub(crate) fn shell_safe(c: char) -> bool {
    c.is_alphanumeric()
        || matches!(
            c,
            '/' | '.' | '_' | '-' | '+' | ',' | ':' | '@' | '%' | '=' | '~'
        )
}

/// 给 shell 用的路径：只有安全的字符（[`shell_safe`]）时原样，否则用单引号包起来（里面的
/// 单引号写成 `'\''`）。
pub(crate) fn shell_quote(path: &std::path::Path) -> String {
    let text = path.to_string_lossy();
    if !text.is_empty() && text.chars().all(shell_safe) {
        return text.into_owned();
    }
    let mut out = String::with_capacity(text.len() + 2);
    out.push('\'');
    for c in text.chars() {
        if c == '\'' {
            out.push_str(r"'\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// 辅助进程 `winc-pty` 的路径：优先用环境变量 `WINC_PTY`，否则在主程序同目录查找。
#[allow(
    clippy::disallowed_methods,
    reason = "只访问本机：辅助进程与主程序在同一目录"
)]
pub(super) fn helper_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("WINC_PTY") {
        return Some(path.into());
    }
    let exe = std::env::current_exe().ok()?;
    let path = exe
        .parent()?
        .join(format!("winc-pty{}", std::env::consts::EXE_SUFFIX));
    path.exists().then_some(path)
}

/// 设置中的默认 shell 是否用于在目录 `dir` 新建的终端：在 Windows 上位于 WSL 发行版中时不用。
/// 菜单中明确选择的 shell 不受此限制。
pub(super) fn default_shell_applies(dir: &[u8], windows: bool) -> bool {
    let dir = bytes_path(dir);
    !(windows && winc_editor::wsl::locate(&dir).is_some())
}

/// 把 shell 报告的工作目录 `path` 转换为编辑器使用的路径（见 [`terminal_path`]）。重启标签和
/// 恢复现场都使用该目录。
pub(super) fn local_cwd(root: Option<&std::path::Path>, path: Vec<u8>) -> Vec<u8> {
    match root {
        Some(root) => os_bytes(terminal_path(root, bytes_path(&path)).as_os_str()),
        None => path,
    }
}

/// 把终端中的路径 `path` 转换为编辑器使用的路径：位于 WSL 发行版中时属于同一发行版；其余路径原样返回。
pub(in crate::app) fn terminal_path(root: &std::path::Path, path: PathBuf) -> PathBuf {
    winc_editor::wsl::from_terminal(root, path)
}

/// shell 集成脚本放在哪里（配置目录下的 `shell`）；`terminal.shellIntegration` 关了时不注入。
pub(super) fn integration_dir(config: &winc_config::Config) -> Option<Vec<u8>> {
    if config.terminal_shell_integration == Some(false) {
        return None;
    }
    let config = winc_config::config_path()?;
    Some(os_bytes(config.parent()?.join("shell").as_os_str()))
}

/// 新终端的启动方式：程序、参数和工作目录。
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Launch {
    /// 要运行的程序；`None` 表示用户的默认 shell。
    pub(super) program: Option<Vec<u8>>,
    pub(super) args: Vec<Vec<u8>>,
    pub(super) login: bool,
    pub(super) cwd: Vec<u8>,
}

/// 返回在目录 `cwd` 新建终端时的启动方式。
///
/// 设置了 `shell` 时运行该命令（分词见 [`command_words`]）。在 Windows（`windows`）
/// 上且 `cwd` 位于 WSL 发行版中时，进入对应的发行版和目录：目录由 `wsl.exe` 的参数指定，进程本身
/// 从家目录启动。其余情况运行默认 shell。
pub(super) fn launch(shell: Option<&str>, cwd: Vec<u8>, windows: bool) -> Launch {
    // Windows 上 `/` 开头的目录来自 WSL 中的 shell，不是 Windows 程序可用的工作目录。
    let cwd = if windows && cwd.first() == Some(&b'/') {
        Vec::new()
    } else {
        cwd
    };
    let mut words = shell.map(command_words).unwrap_or_default().into_iter();
    if let Some(program) = words.next() {
        return Launch {
            program: Some(program.into_bytes()),
            args: words.map(String::into_bytes).collect(),
            login: !windows,
            cwd,
        };
    }
    let dir = String::from_utf8_lossy(&cwd);
    let place = windows
        .then(|| winc_editor::wsl::locate(std::path::Path::new(dir.as_ref())))
        .flatten();
    let Some(place) = place else {
        return Launch {
            program: None,
            args: Vec::new(),
            login: true,
            cwd,
        };
    };
    let args = ["-d", place.distro.as_str(), "--cd", place.path.as_str()];
    Launch {
        program: Some(b"wsl.exe".to_vec()),
        args: args.iter().map(|arg| arg.as_bytes().to_vec()).collect(),
        login: false,
        cwd: Vec::new(),
    }
}

/// 返回给终端的环境变量；`prompt`、`python` 为假时分别加上 `WINC_PROMPT=0`、`WINC_PYTHON=0`，
/// 集成脚本据此不替换默认提示符、不加载启动目录中的 Python 环境。
pub(super) fn environment(prompt: bool, python: bool) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut env: Vec<_> = std::env::vars_os()
        .filter(|(k, _)| k != "WINC_PROMPT" && k != "WINC_PYTHON")
        .map(|(k, v)| (os_bytes(&k), os_bytes(&v)))
        .collect();
    if !prompt {
        env.push((b"WINC_PROMPT".to_vec(), b"0".to_vec()));
    }
    if !python {
        env.push((b"WINC_PYTHON".to_vec(), b"0".to_vec()));
    }
    #[cfg(target_os = "macos")]
    if !env
        .iter()
        .any(|(k, _)| matches!(&k[..], b"LANG" | b"LC_ALL" | b"LC_CTYPE"))
        && let Some(name) = winc_i18n::Locale::system_posix()
        && winc_backend::fs::is_dir(
            std::path::Path::new("/usr/share/locale").join(format!("{name}.UTF-8")),
        )
    {
        env.push((b"LANG".to_vec(), format!("{name}.UTF-8").into_bytes()));
    }
    env
}

/// 路径、环境变量按原样的字节传给辅助进程（它们不一定是 UTF-8）。
#[cfg(unix)]
pub(super) fn os_bytes(s: &std::ffi::OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    s.as_bytes().to_vec()
}

/// 路径、环境变量按原样的字节传给辅助进程；非 Unix 平台只能先按 UTF-8 有损转换。
#[cfg(not(unix))]
pub(super) fn os_bytes(s: &std::ffi::OsStr) -> Vec<u8> {
    s.to_string_lossy().into_owned().into_bytes()
}

/// [`os_bytes`] 反过来：辅助进程报的目录换成路径。
#[cfg(unix)]
pub(super) fn bytes_path(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
}

/// [`os_bytes`] 反过来：辅助进程报的目录换成路径；非 Unix 平台按 UTF-8 有损转换。
#[cfg(not(unix))]
pub(super) fn bytes_path(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}

#[cfg(test)]
mod tests;
