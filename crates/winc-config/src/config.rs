//! 用户配置和项目配置：读入、校验、写回，以及配置文件的位置。
//!
//! 每一项设置的键名和读写规则由 [`Config::SECTIONS`] 和 [`Config::TOP_LEVEL`] 表给出。

mod parse;
mod types;

pub use types::*;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::files::{config_dir, write_atomic};
use crate::json::{self, Json};
use parse::*;
use winc_i18n::{LocaleChoice, t};

/// 用户配置。
///
/// 每一项都是可选的，`None`（集合类型为空）表示用户没写过，使用默认值。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Config {
    /// 编辑区等宽字体，可以是字体名（如 `Menlo`），也可以是字体文件路径。
    pub editor_font: Option<String>,
    /// 编辑区字号（点）。
    ///
    /// 可用范围见 [`Config::EDITOR_SIZE_RANGE`]。
    pub editor_size: Option<f32>,
    /// 界面字体（文件树、标签、状态栏）。
    pub ui_font: Option<String>,
    /// 界面字号（点）。
    ///
    /// 可用范围见 [`Config::UI_SIZE_RANGE`]。
    pub ui_size: Option<f32>,
    /// 界面缩放（百分比）：整个窗口（界面、编辑区和终端）按此比例放大或缩小，叠加在系统的
    /// 显示缩放之上。
    ///
    /// 可用范围见 [`Config::ZOOM_RANGE`]。
    pub ui_zoom: Option<u16>,
    /// 全部文字的字重，默认为 400（各字体自身的字重）；范围见 [`Config::FONT_WEIGHT_RANGE`]。
    pub ui_font_weight: Option<u16>,
    /// 窗口背景图片的路径；没写时没有背景图。
    pub ui_background_image: Option<String>,
    /// 背景图片在底色之上的不透明度（百分比），默认为 15；范围见
    /// [`Config::BACKGROUND_OPACITY_RANGE`]。
    pub ui_background_opacity: Option<f32>,
    /// 背景图片的显示方式，默认为填充。
    pub ui_background_fit: Option<BackgroundFit>,
    /// 是否用系统的标题栏（配置中的 `"titleBarStyle": "native"`）。
    ///
    /// 写成 `"custom"` 或没写时，窗口按钮在 winc 的顶栏中绘制。只影响 macOS 以外的平台。
    pub ui_native_title_bar: Option<bool>,
    /// 是否播放界面的过渡动画（面板、弹出层、列表的变化）。
    ///
    /// 没写时跟随系统的「减弱动态效果」设置；系统没有该设置或未读取时为是。
    pub ui_animations: Option<bool>,
    /// 制表符的显示宽度（空格数）；文件用制表符缩进时，也是每级缩进的宽度。
    ///
    /// 空格缩进的宽度不取此值，按文件内容判断，判断不出时用语言的默认缩进。
    ///
    /// 可用范围见 [`Config::TAB_WIDTH_RANGE`]。
    pub tab_width: Option<usize>,
    /// 什么时候自动保存修改，默认不保存。
    pub auto_save: Option<AutoSave>,
    /// 热退出：退出时不逐个询问是否保存，未保存的内容在下次启动时恢复，默认为开。
    pub hot_exit: Option<bool>,
    /// 关闭窗口或退出前是否先确认，默认为否。
    pub confirm_close: Option<bool>,
    /// 启动时打开上次退出时打开的目录和标签，默认为开。
    ///
    /// 关闭后每次启动都显示开始页；命令行指定了目录或文件时仍然打开。
    pub restore_folder: Option<bool>,
    /// 把目录拖进已打开目录的窗口时怎样处理，默认每次询问。
    pub drop_folder: Option<FolderDrop>,
    /// 已经打开了目录的窗口里再打开另一个目录时，`true` 在新窗口打开，`false` 在此窗口打开
    /// （替换当前目录）；没写时每次询问。
    ///
    /// 提示框上勾选「不再询问」后写入当时的选择。
    pub open_folder_in_new_window: Option<bool>,
    /// 滚动时，把已滚出可视区域的函数、类、标题的开头行固定在编辑区顶部，默认为开。
    pub sticky_scroll: Option<bool>,
    /// 自动换行：按编辑区宽度把长行断开显示（不修改文件），默认为否。
    pub word_wrap: Option<bool>,
    /// 超长行显示全部内容，而不是只显示开头的一部分，默认为否。
    pub whole_lines: Option<bool>,
    /// 在编辑区右侧显示缩略图，默认为否。
    pub minimap: Option<bool>,
    /// 鼠标滚轮每滚一格平滑地滑到位，而不是立即跳到，默认为是。
    pub smooth_scrolling: Option<bool>,
    /// 在侧栏文件视图的底部显示大纲（当前文件的函数、类型、标题），默认为是。
    pub outline: Option<bool>,
    /// 何时显示空格和制表符，默认为选中时。
    pub render_whitespace: Option<RenderWhitespace>,
    /// 差异单栏显示：新旧内容上下排列，默认为否（左右并排）。
    pub diff_inline: Option<bool>,
    /// 在行号栏显示行号，默认为是；关闭后折叠箭头、书签和改动标记仍然显示。
    pub line_numbers: Option<bool>,
    /// 显示窗口底部的状态栏，默认为是。
    pub status_bar: Option<bool>,
    /// 新窗口沿用上次关闭时的大小和位置，默认为是。
    pub restore_window: Option<bool>,
    /// 配色：`auto` 跟随系统，`dark` 或 `light` 强制指定。
    pub theme: Option<ThemeChoice>,
    /// 浅色外观时用的主题（主题的名字，如 `paper`）；没写时用默认的。
    pub light_theme: Option<String>,
    /// 深色外观时用的主题（主题的名字，如 `ember`）；没写时用默认的。
    pub dark_theme: Option<String>,
    /// 界面语言：`auto` 跟随系统的首选语言，或语言标签（如 `en-US`）指定一种语言。
    pub language: Option<LocaleChoice>,
    /// Markdown 预览是否联网加载网址的图片（README 里的徽章、截图），默认为是。
    pub markdown_remote_images: Option<bool>,
    /// Markdown 预览正文的最大宽度，默认为 [`TextWidth::Standard`]。
    pub markdown_text_width: Option<TextWidth>,
    /// Markdown 预览是否显示行号，默认为是：可编辑的预览在左侧写原文行号，其余预览在代码块中
    /// 写从 1 起的行号。
    pub markdown_line_numbers: Option<bool>,
    /// Markdown 文件的源代码是否也使用写作快捷键（标题、列表、链接等），默认为是；可编辑的
    /// 预览总是使用。
    pub markdown_shortcuts: Option<bool>,
    /// 编辑 Markdown 时是否淡化光标所在段落以外的文字（专注模式），默认为否。
    pub markdown_focus: Option<bool>,
    /// 编辑 Markdown 时是否让光标所在的行停在编辑区中间（打字机模式），默认为否。
    pub markdown_typewriter: Option<bool>,
    /// 可编辑的预览中用鼠标选中文字后是否显示格式工具栏，默认为是。
    pub markdown_selection_toolbar: Option<bool>,
    /// 粘贴、拖入 Markdown 的图片的目录，相对文档所在的目录，`${filename}` 表示文档主名；未设置
    /// 时为文档所在的目录。
    pub markdown_image_folder: Option<String>,
    /// 粘贴图片的文件名（不含扩展名），`${filename}` 表示文档主名，`${date}` 表示粘贴时刻，
    /// 未设置时为 `image`。
    pub markdown_image_name: Option<String>,
    /// 是否定时检查更新（启动后 5 秒、之后每天一次），默认为是。
    ///
    /// 关闭后仍可从菜单「检查更新…」手动检查。
    pub update_auto_check: Option<bool>,
    /// 在文件树里拖动文件或目录时是否先确认，默认为是。
    ///
    /// 提示框上勾选「不再询问」后写成 `false`。
    pub confirm_drag_and_drop: Option<bool>,
    /// 文件树、Git 改动列表和历史提交的文件列表是否把连续的单子目录合并显示为一行；未设置时
    /// 合并。
    pub compact_folders: Option<bool>,
    /// 在文件树中单击文件时是否在预览标签中打开；未设置时为否。
    pub preview_on_click: Option<bool>,
    /// 「在外部终端中打开」使用的终端程序；未设置时使用系统默认的终端程序。
    pub external_terminal: Option<String>,
    /// 是否开启 Vim 模式，默认为否。
    pub vim_enabled: Option<bool>,
    /// Vim 模式下无名寄存器与系统剪贴板的同步方式；未设置时只有 `"+`、`"*` 使用系统剪贴板。
    pub vim_clipboard: Option<VimClipboard>,
    /// Vim 模式下查找是否忽略大小写，默认为否。
    pub vim_ignore_case: Option<bool>,
    /// Vim 模式下忽略大小写时，模式中有大写字母是否改为区分大小写，默认为否。
    pub vim_smart_case: Option<bool>,
    /// Vim 模式下光标与窗口上下边缘之间保留的行数，默认为 0。
    ///
    /// 可用范围见 [`Config::VIM_SCROLL_OFF_RANGE`]。
    pub vim_scroll_off: Option<usize>,
    /// Vim 模式下按键归属的改写：按键的名字（小写，如 `ctrl+f`）与是否交给 Vim 层处理。
    ///
    /// 按名字排序；没有列出的按键按默认的规则决定归属。
    pub vim_handle_keys: Vec<(String, bool)>,
    /// Vim 模式下是否显示相对行号，默认为否。
    pub vim_relative_number: Option<bool>,
    /// Vim 模式下复制之后高亮被复制范围的毫秒数，默认为 150，0 表示不高亮。
    ///
    /// 可用范围见 [`Config::VIM_YANK_HIGHLIGHT_RANGE`]。
    pub vim_yank_highlight: Option<u32>,
    /// Vim 模式下映射等待后续按键的毫秒数，默认为 1000。
    ///
    /// 可用范围见 [`Config::VIM_TIMEOUT_LEN_RANGE`]。
    pub vim_timeout_len: Option<u32>,
    /// Vim 模式下映射中 `<Leader>` 代表的按键（按键记法），默认为反斜杠。
    pub vim_leader: Option<String>,
    /// Vim 模式下的映射，按配置文件中的顺序。
    pub vim_keys: Vec<VimMapping>,
    /// 是否启用终端模块（编辑区下方的终端面板），默认为否。
    pub terminal_enabled: Option<bool>,
    /// 终端的等宽字体，写法同 [`Config::editor_font`]；未设置时与编辑区相同。
    pub terminal_font: Option<String>,
    /// 终端的字号（点）；未设置时用编辑区的默认字号，不随编辑区字号变化。
    ///
    /// 可用范围同 [`Config::EDITOR_SIZE_RANGE`]。
    pub terminal_size: Option<f32>,
    /// 终端回滚缓冲的行数。
    ///
    /// 可用范围见 [`Config::SCROLLBACK_RANGE`]，默认 10000 行。
    pub terminal_scrollback: Option<u32>,
    /// 终端中视为 Meta（按键前加 `ESC`）的 ⌥ 键，默认为左侧；只在 macOS 上生效。
    pub option_as_meta: Option<OptionAsMeta>,
    /// 终端的对比度保护：字与底色的对比度低于设定值时把字调亮或调暗，0 表示关闭。
    ///
    /// 可用范围见 [`Config::CONTRAST_RANGE`]，默认 3。
    pub terminal_minimum_contrast: Option<f32>,
    /// 终端运行的程序和参数，以空白分隔（如 `wsl.exe -d Ubuntu`）。未设置时运行系统默认的 shell；
    /// 打开的目录位于 WSL 发行版中时进入对应的发行版。
    pub terminal_shell: Option<String>,
    /// 终端是否自动注入 shell 集成（命令块与项目内 Python 环境），默认为是。
    pub terminal_shell_integration: Option<bool>,
    /// 关闭还有程序在运行的终端标签前是否先确认，默认为是。
    pub terminal_confirm_close: Option<bool>,
    /// 用户自己没设提示符时，终端里换成 winc 的提示符（目录、Git 分支、上一条命令的成败），ls 带
    /// 颜色，zsh 的补全用菜单；默认为是。
    pub terminal_prompt: Option<bool>,
    /// 终端里选中文字（拖选、双击、三击）松开时自动复制，默认为否。
    pub terminal_copy_on_select: Option<bool>,
    /// 终端里把粗体的前 8 种颜色画成对应的亮色，供依赖粗体表示亮色的程序使用，默认为否。
    pub terminal_bold_is_bright: Option<bool>,
    /// 终端面板是否放在编辑区右边（配置里的 `"position": "right"`）；否则（`"bottom"` 或
    /// 没写）放在下面。
    pub terminal_right: Option<bool>,
    /// 是否在面板右侧使用竖向列表管理终端，默认为否。
    pub terminal_vertical_tabs: Option<bool>,
    /// 暂停终端内容增强的快捷键，保留终端管理和复制、粘贴；默认关闭。
    pub terminal_application_priority: Option<bool>,
    /// 是否启用远程模块（经 `ssh` 打开其他主机上的目录），默认为否。
    pub remote_enabled: Option<bool>,
    /// 远端目录挂载在本机的位置（SSHFS、Samba 映射的盘符等）：键是远端目录的显示写法
    /// `目标:/路径`（如 `devbox:/home/me`），值是本机上对应的目录（如 `Z:\`）。
    ///
    /// 远端的文件在这些目录之内时，「在文件管理器中显示」「用默认应用打开」改为对本机上
    /// 对应的位置执行；默认没有。键已规范：路径中没有空段、`.` 和 `..`。
    pub remote_local_folders: BTreeMap<String, String>,
    /// 键位方案：`idea` 或 `vscode`；没写时用 `idea`。
    pub keymap: Option<String>,
    /// 改过的快捷键：命令名 → 按键写法的数组，空数组表示不设快捷键。
    pub shortcuts: BTreeMap<String, Vec<String>>,
    /// 是否启用 Git 模块（文件状态、分支、变更视图），默认为否。
    pub git_enabled: Option<bool>,
    /// 发布、推送或同步分支前是否先确认目标远程仓库，默认为是。
    ///
    /// 提示框上勾选「不再询问」后写成 `false`，可在设置中重新打开。
    pub git_confirm_push: Option<bool>,
    /// 是否允许把通过验证的 HTTP(S) 密码保存在系统的凭据存储中，默认为是；关闭时清除已保存的。
    pub git_remember_credentials: Option<bool>,
    /// 联网的命令（`git`、检查更新、网络图片）是否跟随系统的代理设置，默认为是；环境里已有代理
    /// 变量或 `git` 自己配置了代理时以它们为准。
    pub system_proxy: Option<bool>,
    /// 是否在光标所在行的末尾淡淡地显示最后修改这一行的作者、时间和提交说明，默认为是。
    pub git_inline_blame: Option<bool>,
    /// 是否定时在后台获取远程仓库的更新，默认为否：获取会连接远程，需要认证的远程可能每次都
    /// 要求操作。
    pub git_auto_fetch: Option<bool>,
    /// 提交视图是否显示暂存区：分出「已暂存的更改」一组，各项的状态取自暂存区；默认为否，
    /// 用勾选决定提交的内容。
    pub git_staging_area: Option<bool>,
    /// 在打开的目录下查找仓库的层数，默认见 [`Config::REPOSITORY_SCAN_DEPTH`]，可用范围见
    /// [`Config::REPOSITORY_SCAN_DEPTH_RANGE`]。
    pub repository_scan_depth: Option<usize>,
    /// 新建工作树的存放目录，按仓库分开：`<目录>/<仓库名>/<分支名>`。
    ///
    /// 开头的 `~/` 指仓库所在系统（本机或 WSL 发行版）中用户的主目录，相对路径相对于主工作树。
    /// 未设置，或者该目录与仓库不在同一系统中时，存放在仓库旁边的 `<仓库名>.worktrees` 中。
    pub git_worktree_root: Option<String>,
    /// `git` 程序：完整路径，或在 `PATH` 里查找的程序名；未设置时在 `PATH` 里找 `git`。只用于
    /// 本机的仓库。
    pub git_path: Option<String>,
    /// 点击工作树时在哪个窗口打开它，默认每次询问。
    pub git_open_worktree: Option<WorktreeOpen>,
    /// 新建工作树时复制过去的被忽略的文件（gitignore 写法的 glob，如 `.env*`），默认不复制。
    pub git_worktree_copy: Vec<String>,
    /// 保存时先格式化，默认为否。
    pub format_on_save: Option<bool>,
    /// 保存时不格式化的语言（语言的 id）；手动格式化（`⇧⌥F`）仍然生效。
    pub format_on_save_exclude: Vec<String>,
    /// 各语言的格式化命令，键是语言的 id。
    ///
    /// 没写的语言自动选择工具。
    pub format_commands: BTreeMap<String, String>,
    /// 保存时先修复（在格式化之前），默认为否。
    pub fix_on_save: Option<bool>,
    /// 各语言的修复命令，键是语言的 id。
    ///
    /// 没写的语言自动选择工具。
    pub fix_commands: BTreeMap<String, String>,
}

impl Config {
    /// 编辑区字号的合法范围（点）。
    ///
    /// 超出范围的值在读入时忽略并报告；上下限为经验值。
    pub const EDITOR_SIZE_RANGE: std::ops::RangeInclusive<f32> = 8.0..=32.0;
    /// 界面字号的合法范围（点）。
    ///
    /// 超出范围的值在读入时忽略并报告；上下限为经验值。
    pub const UI_SIZE_RANGE: std::ops::RangeInclusive<f32> = 8.0..=24.0;
    /// 界面缩放的合法范围（百分比）。
    ///
    /// 下限时 13 点的界面文字约为 9 点，仍可辨认；上限时 4K 屏幕在系统缩放为 100% 的情况下，
    /// 界面与 1080p 屏幕上的原始大小相同。超出范围的值在读入时忽略并报告，小数取整。
    pub const ZOOM_RANGE: std::ops::RangeInclusive<f32> = 70.0..=200.0;
    /// 字重的合法范围；超出的值在读入时忽略并报告，小数取整。
    pub const FONT_WEIGHT_RANGE: std::ops::RangeInclusive<f32> = 100.0..=900.0;
    /// 默认的字重。
    pub const FONT_WEIGHT: u16 = 400;
    /// 背景图片不透明度的合法范围（百分比）：0 时不显示图片，100 时图片完全盖住底色。
    ///
    /// 超出范围的值在读入时忽略并报告。
    pub const BACKGROUND_OPACITY_RANGE: std::ops::RangeInclusive<f32> = 0.0..=100.0;
    /// 背景图片不透明度的默认值（百分比）。
    pub const BACKGROUND_OPACITY: f32 = 15.0;
    /// 制表符长度的合法范围（空格数）。
    ///
    /// 超出范围的值在读入时忽略并报告；上下限为经验值。
    pub const TAB_WIDTH_RANGE: std::ops::RangeInclusive<f32> = 1.0..=16.0;
    /// Vim 模式下光标与窗口上下边缘之间保留行数的可用范围（行）。
    ///
    /// 超出范围的值在读入时忽略并报告；上限取自调参，超过窗口高度一半的值效果相同。
    pub const VIM_SCROLL_OFF_RANGE: std::ops::RangeInclusive<f32> = 0.0..=999.0;
    /// Vim 模式下复制之后高亮时间的可用范围（毫秒）。
    ///
    /// 超出范围的值在读入时忽略并报告；上限取自调参，更长的高亮妨碍阅读。
    pub const VIM_YANK_HIGHLIGHT_RANGE: std::ops::RangeInclusive<f32> = 0.0..=5_000.0;
    /// Vim 模式下映射等待后续按键的时间的可用范围（毫秒）。
    ///
    /// 超出范围的值在读入时忽略并报告；上限取自调参，更长的等待与不超时无异。
    pub const VIM_TIMEOUT_LEN_RANGE: std::ops::RangeInclusive<f32> = 0.0..=60_000.0;
    /// 查找仓库的默认层数：覆盖「目录 / 分组 / 项目」这样的常见结构；扫描在后台进行，有目录项总数
    /// 的上限，较深的层数不会拖慢界面。
    pub const REPOSITORY_SCAN_DEPTH: usize = 3;
    /// 查找仓库层数的合法范围。
    ///
    /// 超出范围的值在读入时忽略并报告。
    pub const REPOSITORY_SCAN_DEPTH_RANGE: std::ops::RangeInclusive<f32> = 1.0..=8.0;
    /// 终端回滚缓冲行数的合法范围。
    ///
    /// 超出范围的值在读入时忽略并报告；上限 1000000 行为经验值，避免回滚缓冲占用过多内存。
    pub const SCROLLBACK_RANGE: std::ops::RangeInclusive<f32> = 0.0..=1_000_000.0;
    /// 终端最低对比度的合法范围（WCAG 对比度，1 和 0 都表示不保护）。
    ///
    /// 上限 21 是 WCAG 对比度的最大值，即黑底白字之比。
    pub const CONTRAST_RANGE: std::ops::RangeInclusive<f32> = 0.0..=21.0;

    /// 读取用户级配置，返回配置和读取过程中发现的问题（可能有多条）。
    ///
    /// 配置文件不存在或无法确定配置路径时返回默认值；内容无法理解的部分同样退回默认值，
    /// 原因写入问题列表，由调用方提示用户。
    pub fn load() -> (Config, Vec<String>) {
        match config_path() {
            Some(path) => Config::load_from(&path),
            None => (Config::default(), Vec::new()),
        }
    }

    /// 读取指定文件中的配置，返回值含义同 [`Config::load`]。
    ///
    /// 文件不存在时返回默认值，不报告问题。
    pub fn load_from(path: &Path) -> (Config, Vec<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => Config::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Config::default(), Vec::new()),
            Err(e) => (
                Config::default(),
                vec![t!("config-read-failed", error = e.to_string())],
            ),
        }
    }

    /// 解析配置文件内容，返回设置和所有无法理解的地方；任何输入都不会 panic。
    ///
    /// 内容按 JSONC 解析：允许行注释、块注释、开头的 BOM，以及最后一项后面的逗号。
    pub fn parse(text: &str) -> (Config, Vec<String>) {
        let mut config = Config::default();
        let mut problems = Vec::new();

        let Some((root, _)) = Json::parse_jsonc(text) else {
            problems.push(t!("config-invalid-json").to_owned());
            return (config, problems);
        };

        let Some(object) = root.as_object() else {
            problems.push(t!("config-root-not-object").to_owned());
            return (config, problems);
        };

        for &(name, fields) in Self::SECTIONS {
            if let Some(section) = object.get(name) {
                Self::read_section(&mut config, name, fields, section, &mut problems);
            }
        }
        for field in Self::TOP_LEVEL {
            if let Some(value) = object.get(field.key) {
                (field.read)(&mut config, value, field.key, &mut problems);
            }
        }
        for key in object.keys().filter(|key| !Self::is_known(key)) {
            problems.push(t!("config-unknown-key", key = key.as_str()));
        }

        (config, problems)
    }

    /// 序列化为缩进 2 个空格的 JSON，只写出有值的项。
    ///
    /// 各节和各项按固定顺序写出，按严格 JSON 编码，不写注释。
    pub fn to_json(&self) -> String {
        self.render(None)
    }

    /// 序列化为缩进 2 个空格的 JSON：各节和各项按 [`Config::SECTIONS`]、[`Config::TOP_LEVEL`] 的顺序写出，
    /// 只写出有值的项。
    ///
    /// `original` 是磁盘上原有配置的顶层对象和上次读入或写出该文件时的 `base`。给出时：
    ///
    /// - 相对 `base` 修改过的项写 `self` 中的值，`self` 中没有值的项不写出。
    /// - 没有修改的项和节按原文件写出，包括读入时报告并忽略的值（例如超出范围的值）。
    /// - 不认识的键原样保留：节内的键写在该节已知的项之后，顶层的键写在最后。
    fn render(&self, original: Option<(&BTreeMap<String, Json>, &Config)>) -> String {
        /// 顶层的一项：按固定顺序写出的一节，或单个值。
        enum Entry<'a> {
            Section(Vec<(&'a str, Json)>),
            Value(Json),
        }

        let changed = |field: &Field<Self>| {
            original.is_none_or(|(_, base)| (field.write)(self) != (field.write)(base))
        };
        // 一项要写出的值：修改过的项取 `self` 中的值，没有修改的项取原文件同一对象（`old`）中的值。
        let value = |field: &Field<Self>, old: Option<&BTreeMap<String, Json>>| {
            if changed(field) {
                (field.write)(self)
            } else {
                old?.get(field.key).cloned()
            }
        };
        let root = original.map(|(root, _)| root);
        let mut entries = Vec::new();
        for &(name, fields) in Self::SECTIONS {
            let old = root.and_then(|root| root.get(name));
            if let Some(old) = old.filter(|old| old.as_object().is_none())
                && !fields.iter().any(changed)
            {
                // 原文件中这一节不是对象（读入时已报告并忽略），节内的项也没有修改：原样保留。
                entries.push((name, Entry::Value(old.clone())));
                continue;
            }
            let old = old.and_then(Json::as_object);
            // 没有修改、新位置上也没有的项沿用旧位置上的值。
            let moved = |field: &Field<Self>| {
                let root = root.filter(|_| !changed(field))?;
                Self::moved_from(root, name, field.key).cloned()
            };
            let mut items: Vec<(&str, Json)> = fields
                .iter()
                .filter_map(|field| Some((field.key, value(field, old).or_else(|| moved(field))?)))
                .collect();
            if let Some(old) = old {
                let unknown = old.iter().filter(|(key, _)| {
                    Self::find(fields, key).is_none() && !Self::is_moved(name, key)
                });
                items.extend(unknown.map(|(key, value)| (key.as_str(), value.clone())));
            }
            if !items.is_empty() {
                entries.push((name, Entry::Section(items)));
            }
        }
        for field in Self::TOP_LEVEL {
            if let Some(value) = value(field, root) {
                entries.push((field.key, Entry::Value(value)));
            }
        }
        if let Some(original) = root {
            let unknown = original.iter().filter(|(key, _)| !Self::is_known(key));
            entries.extend(unknown.map(|(key, value)| (key.as_str(), Entry::Value(value.clone()))));
        }

        let mut out = String::new();
        json::write_object(&mut out, 0, entries, |out, entry, indent| match entry {
            Entry::Section(items) => {
                let pairs = items.iter().map(|(key, value)| (*key, value));
                json::write_object(out, indent, pairs, |out, v, indent| {
                    v.write_into(out, indent)
                });
            }
            Entry::Value(value) => value.write_into(out, indent),
        });
        out.push('\n');
        out
    }

    /// 读取项目配置（`root` 下的 `.winc/config.json`），只认 `format` 一节，其他内容报告后忽略。
    ///
    /// 文件不存在时返回 `None`；无法读取或无法理解的部分与用户配置相同，退回默认值并报告。
    /// 远端主机上的目录没有项目配置：这里只读本机的文件，项目配置管的格式化也只在本机运行。
    pub fn load_project(root: &Path) -> Option<(Config, Vec<String>)> {
        if winc_backend::is_remote(root) {
            return None;
        }
        let text = match std::fs::read_to_string(project_config_path(root)) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => {
                let problem = t!("config-project-read-failed", error = e.to_string());
                return Some((Config::default(), vec![problem]));
            }
        };
        Some(Config::parse_project(&text))
    }

    /// 解析项目配置的内容，规则见 [`Config::load_project`]。
    pub(crate) fn parse_project(text: &str) -> (Config, Vec<String>) {
        let mut config = Config::default();
        let mut problems = Vec::new();
        let root = Json::parse_jsonc(text);
        let Some(object) = root.as_ref().and_then(|(root, _)| root.as_object()) else {
            problems.push(t!("config-project-invalid-json").to_owned());
            return (config, problems);
        };
        for (key, value) in object {
            if key == "format" {
                Self::read_section(&mut config, "format", Self::FORMAT, value, &mut problems);
            } else {
                problems.push(t!("config-project-unsupported-key", key = key.as_str()));
            }
        }
        (config, problems)
    }

    /// 把相对 `base`（上次读入或写出该文件时的配置）修改过的项写入指定文件，目录不存在时先创建。
    ///
    /// 其余内容按文件当前的内容保留，其他位置在程序运行期间手动修改的设置不会被覆盖。原文件无法
    /// 解析或含有注释时，先原样另存为同目录下的 `config.json.bak`，再写出 `self` 的全部设置。
    ///
    /// # Errors
    ///
    /// 原文件无法读取、备份失败、目录创建失败、没有写权限或磁盘错误时返回 `std::io::Error`。
    pub fn save_to(&self, path: &Path, base: &Config) -> std::io::Result<Saved> {
        let old = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e),
        };
        let root = if old.trim_ascii().is_empty() {
            Some((Json::Object(BTreeMap::new()), false))
        } else {
            std::str::from_utf8(&old).ok().and_then(Json::parse_jsonc)
        };
        let original = root.as_ref().and_then(|(root, _)| root.as_object());
        let comments = root.as_ref().is_some_and(|&(_, comments)| comments);

        let mut backup = None;
        if original.is_none() || comments {
            let path = path.with_extension("json.bak");
            std::fs::write(&path, &old)?;
            backup = Some(Backup { path, comments });
        }
        let text = self.render(original.map(|root| (root, base)));
        write_atomic(path, &text)?;
        Ok(Saved {
            config: Config::parse(&text).0,
            backup,
        })
    }
}

/// [`Config::save_to`] 的结果。
#[derive(Debug, PartialEq)]
pub struct Saved {
    /// 写入后文件中的配置：修改过的项是新值，其余项取自文件，可能刚在其他位置被手动修改过。
    ///
    /// 调用方改用写入后的配置，并以它作为下次写回时的 `base`。
    pub config: Config,
    /// 写入前另存的原文件：位置和原因；没有另存时为 `None`。
    pub backup: Option<Backup>,
}

/// 返回用户级配置文件的路径；无法确定用户目录时返回 `None`。
///
/// - macOS：`~/Library/Application Support/winc/config.json`
/// - Linux：`$XDG_CONFIG_HOME/winc/config.json`，未设置时为 `~/.config/winc/config.json`
/// - Windows：`%APPDATA%\winc\config.json`，未设置时使用 `HOME` 环境变量指向的目录；便携版放在
///   便携数据目录中（见 [`crate::files::portable_dir`]）
///
/// 不读取系统级配置（如 `/etc/winc/config.json`）：只支持用户级配置。
pub fn config_path() -> Option<PathBuf> {
    let dir = config_dir()?;
    Some(dir.join("config.json"))
}

/// 返回项目配置文件的路径：`root`（打开的目录）下的 `.winc/config.json`。
pub(crate) fn project_config_path(root: &Path) -> PathBuf {
    root.join(".winc").join("config.json")
}

/// 一项设置在配置文件里的写法：键名、读入和写回。
///
/// 解析、序列化以及写回时保留不认识的键，都以 [`Config::SECTIONS`] 和 [`Config::TOP_LEVEL`] 两张表
/// 为准；新增一项设置在对应的表里加一行即可。
struct Field<T> {
    /// 键名：节内的键（如 `size`），或顶层的键（如 `theme`）。
    key: &'static str,
    /// 把 `value` 读入 `target`；类型不符、超出范围等问题记入 `problems`，文案中用 `name`（如
    /// `editor.size`）指代这一项。
    read: fn(target: &mut T, value: &Json, name: &str, problems: &mut Vec<String>),
    /// 返回要写出的值；`None` 表示用户没写过，不写出。
    write: fn(target: &T) -> Option<Json>,
}

/// 布尔设置的 [`Field`]：键名和 [`Config`] 中对应的字段。
macro_rules! bool_field {
    ($key:literal, $field:ident) => {
        Field {
            key: $key,
            read: |config, value, name, problems| config.$field = read_bool(value, name, problems),
            write: |config| config.$field.map(Json::Bool),
        }
    };
}

/// 字符串设置的 [`Field`]：键名和 [`Config`] 中对应的字段，读入规则见 [`read_string`]。
macro_rules! string_field {
    ($key:literal, $field:ident) => {
        Field {
            key: $key,
            read: |config, value, name, problems| {
                config.$field = read_string(value, name, problems)
            },
            write: |config| config.$field.clone().map(Json::String),
        }
    };
}

impl Config {
    /// 配置文件中的各节（顶层值为对象的键）和其中的设置，按写出的顺序排列。
    const SECTIONS: &[(&str, &[Field<Self>])] = &[
        ("editor", Self::EDITOR),
        ("ui", Self::UI),
        ("tree", Self::TREE),
        ("markdown", Self::MARKDOWN),
        ("update", Self::UPDATE),
        ("terminal", Self::TERMINAL),
        ("vim", Self::VIM),
        ("git", Self::GIT),
        ("network", Self::NETWORK),
        ("remote", Self::REMOTE),
        ("keyboard", Self::KEYBOARD),
        ("format", Self::FORMAT),
    ];

    /// 顶层的单值设置，写在各节之后。
    const TOP_LEVEL: &[Field<Self>] = &[
        Field {
            key: "theme",
            read: |config, value, name, problems| {
                config.theme = match value.as_str() {
                    Some("auto") => Some(ThemeChoice::Auto),
                    Some("dark") => Some(ThemeChoice::Dark),
                    Some("light") => Some(ThemeChoice::Light),
                    _ => {
                        problems.push(t!("config-theme-invalid", name = name));
                        None
                    }
                }
            },
            write: |config| {
                let theme = match config.theme? {
                    ThemeChoice::Auto => "auto",
                    ThemeChoice::Dark => "dark",
                    ThemeChoice::Light => "light",
                };
                Some(Json::String(theme.to_owned()))
            },
        },
        string_field!("lightTheme", light_theme),
        string_field!("darkTheme", dark_theme),
        Field {
            key: "language",
            read: |config, value, name, problems| {
                config.language = value.as_str().and_then(LocaleChoice::from_key);
                if config.language.is_none() {
                    problems.push(t!("config-language-invalid", name = name));
                }
            },
            write: |config| Some(Json::String(config.language?.key().to_owned())),
        },
    ];

    /// `editor` 一节。
    const EDITOR: &[Field<Self>] = &[
        string_field!("font", editor_font),
        Field {
            key: "size",
            read: |config, value, name, problems| {
                let label = t!("config-label-font-size");
                config.editor_size =
                    read_number(value, name, Config::EDITOR_SIZE_RANGE, label, problems)
            },
            write: |config| config.editor_size.map(number),
        },
        Field {
            key: "tabWidth",
            read: |config, value, name, problems| {
                let label = t!("config-label-tab-width");
                config.tab_width =
                    read_number(value, name, Config::TAB_WIDTH_RANGE, label, problems)
                        .filter(|v| {
                            let whole = v.fract() == 0.0;
                            if !whole {
                                let value = v.to_string();
                                problems.push(t!("config-not-integer", name = name, value = value));
                            }
                            whole
                        })
                        .map(|v| v as usize)
            },
            write: |config| config.tab_width.map(|width| Json::Number(width as f64)),
        },
        Field {
            key: "autoSave",
            // 早先的版本是开关：`true` 即切换焦点时保存。
            read: |config, value, name, problems| {
                config.auto_save = match (value, value.as_str()) {
                    (Json::Bool(false), _) | (_, Some("off")) => Some(AutoSave::Off),
                    (Json::Bool(true), _) | (_, Some("onFocusChange")) => {
                        Some(AutoSave::OnFocusChange)
                    }
                    (_, Some("afterDelay")) => Some(AutoSave::AfterDelay),
                    _ => {
                        problems.push(t!("config-auto-save-invalid", name = name));
                        None
                    }
                };
            },
            write: |config| {
                let value = match config.auto_save? {
                    AutoSave::Off => "off",
                    AutoSave::OnFocusChange => "onFocusChange",
                    AutoSave::AfterDelay => "afterDelay",
                };
                Some(Json::String(value.into()))
            },
        },
        bool_field!("hotExit", hot_exit),
        bool_field!("confirmClose", confirm_close),
        bool_field!("restoreFolder", restore_folder),
        bool_field!("openFolderInNewWindow", open_folder_in_new_window),
        Field {
            key: "dropFolder",
            read: |config, value, name, problems| {
                config.drop_folder = match value.as_str() {
                    Some("ask") => Some(FolderDrop::Ask),
                    Some("add") => Some(FolderDrop::Add),
                    Some("open") => Some(FolderDrop::Open),
                    _ => {
                        problems.push(t!("config-drop-folder-invalid", name = name));
                        None
                    }
                };
            },
            write: |config| {
                let value = match config.drop_folder? {
                    FolderDrop::Ask => "ask",
                    FolderDrop::Add => "add",
                    FolderDrop::Open => "open",
                };
                Some(Json::String(value.into()))
            },
        },
        bool_field!("stickyScroll", sticky_scroll),
        bool_field!("wordWrap", word_wrap),
        bool_field!("wholeLines", whole_lines),
        bool_field!("minimap", minimap),
        bool_field!("smoothScrolling", smooth_scrolling),
        bool_field!("outline", outline),
        Field {
            key: "renderWhitespace",
            read: |config, value, name, problems| {
                config.render_whitespace = match value.as_str() {
                    Some("none") => Some(RenderWhitespace::None),
                    Some("selection") => Some(RenderWhitespace::Selection),
                    Some("trailing") => Some(RenderWhitespace::Trailing),
                    Some("all") => Some(RenderWhitespace::All),
                    _ => {
                        problems.push(t!("config-render-whitespace-invalid", name = name));
                        None
                    }
                }
            },
            write: |config| {
                let value = match config.render_whitespace? {
                    RenderWhitespace::None => "none",
                    RenderWhitespace::Selection => "selection",
                    RenderWhitespace::Trailing => "trailing",
                    RenderWhitespace::All => "all",
                };
                Some(Json::String(value.to_owned()))
            },
        },
        bool_field!("diffInline", diff_inline),
        bool_field!("lineNumbers", line_numbers),
        bool_field!("statusBar", status_bar),
        bool_field!("restoreWindow", restore_window),
    ];

    /// `ui` 一节。
    const UI: &[Field<Self>] = &[
        string_field!("font", ui_font),
        bool_field!("animations", ui_animations),
        Field {
            key: "size",
            read: |config, value, name, problems| {
                let label = t!("config-label-font-size");
                config.ui_size = read_number(value, name, Config::UI_SIZE_RANGE, label, problems)
            },
            write: |config| config.ui_size.map(number),
        },
        Field {
            key: "zoom",
            read: |config, value, name, problems| {
                let label = t!("config-label-zoom");
                config.ui_zoom = read_number(value, name, Config::ZOOM_RANGE, label, problems)
                    .map(|zoom| zoom.round() as u16);
            },
            write: |config| config.ui_zoom.map(|zoom| number(f32::from(zoom))),
        },
        Field {
            key: "fontWeight",
            read: |config, value, name, problems| {
                let label = t!("config-label-font-weight");
                let range = Config::FONT_WEIGHT_RANGE;
                config.ui_font_weight =
                    read_number(value, name, range, label, problems).map(|w| w.round() as u16);
            },
            write: |config| {
                config
                    .ui_font_weight
                    .map(|weight| number(f32::from(weight)))
            },
        },
        string_field!("backgroundImage", ui_background_image),
        Field {
            key: "backgroundOpacity",
            read: |config, value, name, problems| {
                let label = t!("config-label-background-opacity");
                let range = Config::BACKGROUND_OPACITY_RANGE;
                config.ui_background_opacity = read_number(value, name, range, label, problems);
            },
            write: |config| config.ui_background_opacity.map(number),
        },
        Field {
            key: "backgroundFit",
            read: |config, value, name, problems| {
                config.ui_background_fit = match value.as_str() {
                    Some("fill") => Some(BackgroundFit::Fill),
                    Some("fit") => Some(BackgroundFit::Fit),
                    Some("stretch") => Some(BackgroundFit::Stretch),
                    Some("tile") => Some(BackgroundFit::Tile),
                    _ => {
                        problems.push(t!("config-background-fit-invalid", name = name));
                        None
                    }
                };
            },
            write: |config| {
                let value = match config.ui_background_fit? {
                    BackgroundFit::Fill => "fill",
                    BackgroundFit::Fit => "fit",
                    BackgroundFit::Stretch => "stretch",
                    BackgroundFit::Tile => "tile",
                };
                Some(Json::String(value.into()))
            },
        },
        Field {
            // `titleBarStyle` 在配置中是字符串，读入后只保留是否用系统的标题栏。
            key: "titleBarStyle",
            read: |config, value, name, problems| match value.as_str() {
                Some("custom") => config.ui_native_title_bar = Some(false),
                Some("native") => config.ui_native_title_bar = Some(true),
                _ => problems.push(t!("config-title-bar-style-invalid", name = name)),
            },
            write: |config| {
                let native = config.ui_native_title_bar?;
                Some(Json::String(
                    if native { "native" } else { "custom" }.into(),
                ))
            },
        },
    ];

    /// `tree` 一节。
    const TREE: &[Field<Self>] = &[
        bool_field!("confirmDragAndDrop", confirm_drag_and_drop),
        bool_field!("compactFolders", compact_folders),
        bool_field!("previewOnClick", preview_on_click),
    ];

    /// `markdown` 一节。
    const MARKDOWN: &[Field<Self>] = &[
        bool_field!("remoteImages", markdown_remote_images),
        bool_field!("lineNumbers", markdown_line_numbers),
        bool_field!("shortcuts", markdown_shortcuts),
        bool_field!("focusMode", markdown_focus),
        bool_field!("typewriterMode", markdown_typewriter),
        bool_field!("selectionToolbar", markdown_selection_toolbar),
        string_field!("imageFolder", markdown_image_folder),
        string_field!("imageName", markdown_image_name),
        Field {
            key: "textWidth",
            read: |config, value, name, problems| {
                config.markdown_text_width = match value.as_str() {
                    Some("standard") => Some(TextWidth::Standard),
                    Some("wide") => Some(TextWidth::Wide),
                    Some("full") => Some(TextWidth::Full),
                    _ => {
                        problems.push(t!("config-text-width-invalid", name = name));
                        None
                    }
                };
            },
            write: |config| {
                let value = match config.markdown_text_width? {
                    TextWidth::Standard => "standard",
                    TextWidth::Wide => "wide",
                    TextWidth::Full => "full",
                };
                Some(Json::String(value.into()))
            },
        },
    ];

    /// `update` 一节。
    const UPDATE: &[Field<Self>] = &[bool_field!("autoCheck", update_auto_check)];

    /// `vim` 一节。
    const VIM: &[Field<Self>] = &[
        bool_field!("enabled", vim_enabled),
        Field {
            key: "clipboard",
            read: |config, value, name, problems| {
                config.vim_clipboard = match value.as_str() {
                    Some("register") => Some(VimClipboard::Register),
                    Some("yank") => Some(VimClipboard::Yank),
                    Some("always") => Some(VimClipboard::Always),
                    _ => {
                        problems.push(t!("config-vim-clipboard-invalid", name = name));
                        None
                    }
                }
            },
            write: |config| {
                let value = match config.vim_clipboard? {
                    VimClipboard::Register => "register",
                    VimClipboard::Yank => "yank",
                    VimClipboard::Always => "always",
                };
                Some(Json::String(value.to_owned()))
            },
        },
        bool_field!("ignoreCase", vim_ignore_case),
        bool_field!("smartCase", vim_smart_case),
        Field {
            key: "scrollOff",
            read: |config, value, name, problems| {
                let label = t!("config-label-vim-scroll-off");
                let range = Config::VIM_SCROLL_OFF_RANGE;
                config.vim_scroll_off = read_number(value, name, range, label, problems)
                    .filter(|v| {
                        let whole = v.fract() == 0.0;
                        if !whole {
                            let value = v.to_string();
                            problems.push(t!("config-not-integer", name = name, value = value));
                        }
                        whole
                    })
                    .map(|v| v as usize)
            },
            write: |config| {
                config
                    .vim_scroll_off
                    .map(|lines| Json::Number(lines as f64))
            },
        },
        Field {
            // 对象的每一项是按键的名字与 `vim` 或 `app`；写法有误的项逐个报告并忽略。
            key: "handleKeys",
            read: |config, value, name, problems| {
                let Some(keys) = value.as_object() else {
                    problems.push(t!("config-vim-handle-keys-invalid", name = name));
                    return;
                };
                config.vim_handle_keys.clear();
                for (key, owner) in keys {
                    let key = key.trim().to_lowercase();
                    let vim = match owner.as_str() {
                        Some("vim") => true,
                        Some("app") => false,
                        _ => {
                            let name = format!("{name}.{key}");
                            problems.push(t!("config-vim-handle-keys-invalid", name = name));
                            continue;
                        }
                    };
                    config.vim_handle_keys.push((key, vim));
                }
                config.vim_handle_keys.sort();
            },
            write: |config| {
                if config.vim_handle_keys.is_empty() {
                    return None;
                }
                let owner = |vim: &bool| Json::String(if *vim { "vim" } else { "app" }.to_owned());
                let keys = config.vim_handle_keys.iter();
                Some(Json::object(
                    keys.map(|(key, vim)| (key.clone(), owner(vim))),
                ))
            },
        },
        bool_field!("relativeNumber", vim_relative_number),
        Field {
            key: "yankHighlight",
            read: |config, value, name, problems| {
                let label = t!("config-label-vim-yank-highlight");
                let range = Config::VIM_YANK_HIGHLIGHT_RANGE;
                config.vim_yank_highlight = read_number(value, name, range, label, problems)
                    .filter(|v| {
                        let whole = v.fract() == 0.0;
                        if !whole {
                            let value = v.to_string();
                            problems.push(t!("config-not-integer", name = name, value = value));
                        }
                        whole
                    })
                    .map(|v| v as u32)
            },
            write: |config| {
                config
                    .vim_yank_highlight
                    .map(|ms| Json::Number(f64::from(ms)))
            },
        },
        Field {
            key: "timeoutLen",
            read: |config, value, name, problems| {
                let label = t!("config-label-vim-timeout-len");
                let range = Config::VIM_TIMEOUT_LEN_RANGE;
                config.vim_timeout_len = read_number(value, name, range, label, problems)
                    .filter(|v| {
                        let whole = v.fract() == 0.0;
                        if !whole {
                            let value = v.to_string();
                            problems.push(t!("config-not-integer", name = name, value = value));
                        }
                        whole
                    })
                    .map(|v| v as u32)
            },
            write: |config| config.vim_timeout_len.map(|ms| Json::Number(f64::from(ms))),
        },
        Field {
            key: "leader",
            read: |config, value, name, problems| {
                config.vim_leader = match value.as_str() {
                    Some(key) if !key.is_empty() => Some(key.to_owned()),
                    _ => {
                        problems.push(t!("config-vim-leader-invalid", name = name));
                        None
                    }
                }
            },
            write: |config| config.vim_leader.clone().map(Json::String),
        },
        Field {
            // 数组的每一项是一条映射；写法有误的项逐个报告并忽略。
            key: "keys",
            read: |config, value, name, problems| {
                config.vim_keys.clear();
                let Some(items) = value.as_array() else {
                    problems.push(t!("config-vim-keys-invalid", name = name));
                    return;
                };
                for (index, item) in items.iter().enumerate() {
                    let text = |key: &str| {
                        let value = item.as_object()?.iter().find(|(name, _)| *name == key)?.1;
                        value.as_str().filter(|text| !text.is_empty())
                    };
                    let mode = text("mode").and_then(|mode| {
                        let mut names = VimMapMode::NAMES.iter();
                        names.find(|(_, name)| *name == mode).map(|(mode, _)| *mode)
                    });
                    match (mode, text("keys"), text("to")) {
                        (Some(mode), Some(keys), Some(to)) => config.vim_keys.push(VimMapping {
                            mode,
                            keys: keys.to_owned(),
                            to: to.to_owned(),
                        }),
                        _ => {
                            let name = format!("{name}[{index}]");
                            problems.push(t!("config-vim-keys-invalid", name = name));
                        }
                    }
                }
            },
            write: |config| {
                if config.vim_keys.is_empty() {
                    return None;
                }
                let item = |mapping: &VimMapping| {
                    let mut names = VimMapMode::NAMES.iter();
                    let mode = names
                        .find(|(mode, _)| *mode == mapping.mode)
                        .map_or("normal", |(_, name)| *name);
                    Json::object([
                        ("mode".to_owned(), Json::String(mode.to_owned())),
                        ("keys".to_owned(), Json::String(mapping.keys.clone())),
                        ("to".to_owned(), Json::String(mapping.to.clone())),
                    ])
                };
                Some(Json::Array(config.vim_keys.iter().map(item).collect()))
            },
        },
    ];

    /// `terminal` 一节。
    const TERMINAL: &[Field<Self>] = &[
        Field {
            key: "external",
            // 只有空白视为没写，使用系统默认的终端程序，不报告。
            read: |config, value, name, problems| match value.as_str() {
                Some(app) => {
                    let app = app.trim();
                    config.external_terminal = (!app.is_empty()).then(|| app.to_owned());
                }
                None => problems.push(t!("config-not-string", name = name)),
            },
            write: |config| config.external_terminal.clone().map(Json::String),
        },
        bool_field!("enabled", terminal_enabled),
        string_field!("font", terminal_font),
        Field {
            key: "size",
            read: |config, value, name, problems| {
                let label = t!("config-label-font-size");
                config.terminal_size =
                    read_number(value, name, Config::EDITOR_SIZE_RANGE, label, problems)
            },
            write: |config| config.terminal_size.map(number),
        },
        string_field!("shell", terminal_shell),
        bool_field!("shellIntegration", terminal_shell_integration),
        bool_field!("confirmClose", terminal_confirm_close),
        bool_field!("prompt", terminal_prompt),
        bool_field!("copyOnSelect", terminal_copy_on_select),
        bool_field!("boldIsBright", terminal_bold_is_bright),
        bool_field!("verticalTabs", terminal_vertical_tabs),
        bool_field!("applicationPriority", terminal_application_priority),
        Field {
            // `position` 在配置里是字符串，读入后只保留是否在右边。
            key: "position",
            read: |config, value, name, problems| match value.as_str() {
                Some("bottom") => config.terminal_right = Some(false),
                Some("right") => config.terminal_right = Some(true),
                _ => problems.push(t!("config-terminal-position-invalid", name = name)),
            },
            write: |config| {
                let right = config.terminal_right?;
                Some(Json::String(if right { "right" } else { "bottom" }.into()))
            },
        },
        Field {
            key: "scrollback",
            read: |config, value, name, problems| {
                let label = t!("config-label-lines");
                config.terminal_scrollback =
                    read_number(value, name, Config::SCROLLBACK_RANGE, label, problems)
                        .map(|n| n.round() as u32);
            },
            write: |config| config.terminal_scrollback.map(|n| number(n as f32)),
        },
        Field {
            key: "minimumContrast",
            read: |config, value, name, problems| {
                let label = t!("config-label-contrast");
                config.terminal_minimum_contrast =
                    read_number(value, name, Config::CONTRAST_RANGE, label, problems);
            },
            write: |config| config.terminal_minimum_contrast.map(number),
        },
        Field {
            key: "optionAsMeta",
            read: |config, value, name, problems| {
                config.option_as_meta = match value.as_str() {
                    Some("left") => Some(OptionAsMeta::Left),
                    Some("right") => Some(OptionAsMeta::Right),
                    Some("both") => Some(OptionAsMeta::Both),
                    Some("none") => Some(OptionAsMeta::None),
                    _ => {
                        problems.push(t!("config-option-as-meta-invalid", name = name));
                        None
                    }
                }
            },
            write: |config| {
                config
                    .option_as_meta
                    .map(|o| Json::String(o.name().to_owned()))
            },
        },
    ];

    /// `git` 一节。
    const GIT: &[Field<Self>] = &[
        bool_field!("enabled", git_enabled),
        bool_field!("confirmPush", git_confirm_push),
        bool_field!("rememberCredentials", git_remember_credentials),
        bool_field!("inlineBlame", git_inline_blame),
        bool_field!("autoFetch", git_auto_fetch),
        bool_field!("stagingArea", git_staging_area),
        Field {
            key: "repositoryScanDepth",
            read: |config, value, name, problems| {
                let label = t!("config-label-repository-scan-depth");
                let range = Config::REPOSITORY_SCAN_DEPTH_RANGE;
                config.repository_scan_depth = read_number(value, name, range, label, problems)
                    .filter(|v| {
                        let whole = v.fract() == 0.0;
                        if !whole {
                            let value = v.to_string();
                            problems.push(t!("config-not-integer", name = name, value = value));
                        }
                        whole
                    })
                    .map(|v| v as usize)
            },
            write: |config| {
                config
                    .repository_scan_depth
                    .map(|depth| Json::Number(depth as f64))
            },
        },
        string_field!("path", git_path),
        string_field!("worktreeRoot", git_worktree_root),
        Field {
            key: "openWorktree",
            read: |config, value, name, problems| {
                config.git_open_worktree = match value.as_str() {
                    Some("ask") => Some(WorktreeOpen::Ask),
                    Some("newWindow") => Some(WorktreeOpen::NewWindow),
                    Some("currentWindow") => Some(WorktreeOpen::CurrentWindow),
                    _ => {
                        problems.push(t!("config-open-worktree-invalid", name = name));
                        None
                    }
                };
            },
            write: |config| {
                let value = match config.git_open_worktree? {
                    WorktreeOpen::Ask => "ask",
                    WorktreeOpen::NewWindow => "newWindow",
                    WorktreeOpen::CurrentWindow => "currentWindow",
                };
                Some(Json::String(value.into()))
            },
        },
        Field {
            key: "worktreeCopy",
            read: |config, value, name, problems| {
                let Some(items) = value.as_array() else {
                    problems.push(t!("config-not-array", name = name));
                    return;
                };
                for item in items {
                    match item.as_str() {
                        Some(glob) if !glob.trim().is_empty() => {
                            config.git_worktree_copy.push(glob.to_owned());
                        }
                        _ => problems.push(t!("config-item-not-non-empty-string", name = name)),
                    }
                }
            },
            write: |config| {
                let globs = &config.git_worktree_copy;
                (!globs.is_empty())
                    .then(|| Json::Array(globs.iter().cloned().map(Json::String).collect()))
            },
        },
    ];

    /// `network` 一节。
    const NETWORK: &[Field<Self>] = &[bool_field!("systemProxy", system_proxy)];

    /// 挪过位置的设置：（旧的节，旧的键，新的节，新的键）。旧的位置照样读入，写回时只写新的位置。
    const MOVED: &[(&str, &str, &str, &str)] = &[("git", "systemProxy", "network", "systemProxy")];

    /// 节 `name` 中的 `key` 挪去的位置的那一项。
    fn moved_to(name: &str, key: &str) -> Option<&'static Field<Self>> {
        let &(_, _, section, key) = Self::MOVED.iter().find(|m| (m.0, m.1) == (name, key))?;
        let &(_, fields) = Self::SECTIONS.iter().find(|&&(n, _)| n == section)?;
        Self::find(fields, key)
    }

    /// 原文件 `root` 中，挪到节 `name` 的 `key` 那一项在旧位置上的值。
    fn moved_from<'a>(root: &'a BTreeMap<String, Json>, name: &str, key: &str) -> Option<&'a Json> {
        let &(section, old, ..) = Self::MOVED.iter().find(|m| (m.2, m.3) == (name, key))?;
        root.get(section)?.as_object()?.get(old)
    }

    /// 节 `name` 中的 `key` 是否已经挪走（见 [`Config::MOVED`]）。
    fn is_moved(name: &str, key: &str) -> bool {
        Self::MOVED.iter().any(|m| (m.0, m.1) == (name, key))
    }

    /// `remote` 一节。
    const REMOTE: &[Field<Self>] = &[
        bool_field!("enabled", remote_enabled),
        Field {
            key: "localFolders",
            read: |config, value, name, problems| {
                read_local_folders(value, name, &mut config.remote_local_folders, problems)
            },
            write: |config| {
                let folders = &config.remote_local_folders;
                let pairs = (folders.iter())
                    .map(|(remote, local)| (remote.as_str(), Json::String(local.clone())));
                (!folders.is_empty()).then(|| Json::object(pairs))
            },
        },
    ];

    /// `keyboard` 一节：键位方案和改过的快捷键；命令名和按键写法由应用检查。
    const KEYBOARD: &[Field<Self>] = &[
        Field {
            key: "keymap",
            read: |config, value, name, problems| {
                config.keymap = match value.as_str() {
                    Some(key @ ("idea" | "vscode")) => Some(key.to_owned()),
                    _ => {
                        problems.push(t!("config-keymap-invalid", name = name));
                        None
                    }
                }
            },
            write: |config| config.keymap.clone().map(Json::String),
        },
        Field {
            key: "shortcuts",
            read: |config, value, name, problems| {
                let entries = value.as_object().and_then(|object| {
                    (object.iter())
                        .map(|(command, keys)| {
                            let keys = keys.as_array()?.iter().map(|key| key.as_str());
                            let keys: Option<Vec<&str>> = keys.collect();
                            Some((
                                command.clone(),
                                keys?.into_iter().map(str::to_owned).collect(),
                            ))
                        })
                        .collect::<Option<BTreeMap<String, Vec<String>>>>()
                });
                match entries {
                    Some(entries) => config.shortcuts = entries,
                    None => problems.push(t!("config-shortcuts-invalid", name = name)),
                }
            },
            write: |config| {
                (!config.shortcuts.is_empty()).then(|| {
                    let keys = |keys: &Vec<String>| {
                        Json::Array(keys.iter().cloned().map(Json::String).collect())
                    };
                    let object = config.shortcuts.iter().map(|(k, v)| (k.clone(), keys(v)));
                    Json::Object(object.collect())
                })
            },
        },
    ];

    /// `format` 一节，用户配置和项目配置共用。
    ///
    /// 语言可以写 id 或状态栏上显示的名字（如 `json`、`JSON`），统一存成 id。
    const FORMAT: &[Field<Self>] = &[
        bool_field!("onSave", format_on_save),
        Field {
            key: "onSaveExclude",
            read: |config, value, name, problems| {
                let Some(items) = value.as_array() else {
                    problems.push(t!("config-not-array", name = name));
                    return;
                };
                for item in items {
                    let Some(language) = item.as_str() else {
                        problems.push(t!("config-item-not-string", name = name));
                        continue;
                    };
                    if let Some(id) = language_id(language, name, problems)
                        && !config.format_on_save_exclude.contains(&id)
                    {
                        config.format_on_save_exclude.push(id);
                    }
                }
            },
            write: |config| {
                let ids = &config.format_on_save_exclude;
                (!ids.is_empty())
                    .then(|| Json::Array(ids.iter().cloned().map(Json::String).collect()))
            },
        },
        Field {
            key: "commands",
            read: |config, value, name, problems| {
                read_commands(value, name, &mut config.format_commands, problems)
            },
            write: |config| commands_json(&config.format_commands),
        },
        bool_field!("fixOnSave", fix_on_save),
        Field {
            key: "fixCommands",
            read: |config, value, name, problems| {
                read_commands(value, name, &mut config.fix_commands, problems)
            },
            write: |config| commands_json(&config.fix_commands),
        },
    ];

    /// 在 `fields` 中找键名为 `key` 的一项。
    fn find<'a>(fields: &'a [Field<Self>], key: &str) -> Option<&'a Field<Self>> {
        fields.iter().find(|field| field.key == key)
    }

    /// 顶层的 `key` 是否是认识的设置：[`Config::SECTIONS`] 中某一节的名字，或 [`Config::TOP_LEVEL`] 中的一项。
    fn is_known(key: &str) -> bool {
        Self::SECTIONS.iter().any(|&(name, _)| name == key)
            || Self::find(Self::TOP_LEVEL, key).is_some()
    }

    /// 把名为 `name` 的一节读入 `config`：不是对象，或其中有 `fields` 以外的键时，记入 `problems`。
    fn read_section(
        config: &mut Self,
        name: &str,
        fields: &[Field<Self>],
        section: &Json,
        problems: &mut Vec<String>,
    ) {
        let Some(values) = section.as_object() else {
            problems.push(t!("config-not-object", name = name));
            return;
        };
        for (key, value) in values {
            match Self::find(fields, key).or_else(|| Self::moved_to(name, key)) {
                Some(field) => (field.read)(config, value, &format!("{name}.{key}"), problems),
                None => problems.push(t!("config-unknown-key", key = format!("{name}.{key}"))),
            }
        }
    }
}

/// 把 `f32` 转成 JSON 数字，按 `f32` 的最短十进制写法输出：`12.3` 不写成 `12.300000190734863`。
fn number(value: f32) -> Json {
    Json::Number(value.to_string().parse().unwrap_or(f64::from(value)))
}

#[cfg(test)]
mod tests;
