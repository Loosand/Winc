//! 配置项使用的枚举与结构体类型，不包含 [`super::Config`] 的读写逻辑。

use std::path::PathBuf;

/// 终端里哪一边的 ⌥ 当 Meta 用；另一边仍然输入 `å`、`∑` 这类字符。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionAsMeta {
    /// 左边的 ⌥。
    Left,
    /// 右边的 ⌥。
    Right,
    /// 两边都是。
    Both,
    /// 两边都不当 Meta 用。
    None,
}

impl OptionAsMeta {
    /// 全部选项，顺序与设置中菜单的顺序一致。
    pub const ALL: [OptionAsMeta; 4] = [
        OptionAsMeta::Left,
        OptionAsMeta::Right,
        OptionAsMeta::Both,
        OptionAsMeta::None,
    ];

    /// 返回该变体在配置文件里的写法。
    pub fn name(self) -> &'static str {
        match self {
            OptionAsMeta::Left => "left",
            OptionAsMeta::Right => "right",
            OptionAsMeta::Both => "both",
            OptionAsMeta::None => "none",
        }
    }
}

/// Vim 模式下无名寄存器与系统剪贴板的同步方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VimClipboard {
    /// 只有 `"+`、`"*` 使用系统剪贴板。
    #[default]
    Register,
    /// 复制时同时写入系统剪贴板。
    Yank,
    /// 无名寄存器与系统剪贴板同步。
    Always,
}

impl VimClipboard {
    /// 全部取值，按设置菜单中的顺序排列。
    pub const ALL: [Self; 3] = [Self::Register, Self::Yank, Self::Always];
}

/// 什么时候自动保存有修改的文档。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutoSave {
    /// 不自动保存。
    #[default]
    Off,
    /// 切换标签、编辑区失去焦点（例如切到终端）或窗口失去焦点时保存。
    OnFocusChange,
    /// 停止输入一段时间后保存，切换焦点时也保存。
    AfterDelay,
}

impl AutoSave {
    /// 全部取值，按设置菜单中的顺序排列。
    pub const ALL: [Self; 3] = [Self::Off, Self::OnFocusChange, Self::AfterDelay];
}

/// 把目录拖进已打开目录的窗口时怎样处理。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FolderDrop {
    /// 每次询问。
    #[default]
    Ask,
    /// 加为窗口的附加目录。
    Add,
    /// 不添加，按落点处理：落在编辑区时打开该目录，落在文件树上时复制进目标目录。
    Open,
}

impl FolderDrop {
    /// 全部取值，按设置菜单中的顺序排列。
    pub const ALL: [Self; 3] = [Self::Ask, Self::Add, Self::Open];
}

/// 点击工作树时在哪个窗口打开它。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorktreeOpen {
    /// 每次询问。
    #[default]
    Ask,
    /// 在新窗口中打开，当前窗口不变。
    NewWindow,
    /// 在当前窗口中打开，替换当前打开的目录。
    CurrentWindow,
}

impl WorktreeOpen {
    /// 全部取值，按设置菜单中的顺序排列。
    pub const ALL: [Self; 3] = [Self::Ask, Self::NewWindow, Self::CurrentWindow];
}

/// Vim 模式下映射生效的模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimMapMode {
    /// 普通模式。
    Normal,
    /// 可视模式。
    Visual,
    /// 插入模式。
    Insert,
    /// 已输入操作符、等待动作时。
    Operator,
}

impl VimMapMode {
    /// 各模式及其在配置文件中的写法。
    pub const NAMES: [(Self, &'static str); 4] = [
        (Self::Normal, "normal"),
        (Self::Visual, "visual"),
        (Self::Insert, "insert"),
        (Self::Operator, "operator"),
    ];
}

/// Vim 模式下的一条映射。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VimMapping {
    /// 生效的模式。
    pub mode: VimMapMode,
    /// 左侧的按键，按键记法。
    pub keys: String,
    /// 右侧：按键记法，或命令的英文名。
    pub to: String,
}

/// 何时显示空格和制表符。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RenderWhitespace {
    /// 不显示。
    None,
    /// 只显示选中文字里的。
    #[default]
    Selection,
    /// 只显示行尾的。
    Trailing,
    /// 全部显示。
    All,
}

impl RenderWhitespace {
    /// 全部选项，顺序与设置中菜单的顺序一致。
    pub const ALL: [RenderWhitespace; 4] = [
        RenderWhitespace::None,
        RenderWhitespace::Selection,
        RenderWhitespace::Trailing,
        RenderWhitespace::All,
    ];
}

/// Markdown 预览正文的最大宽度：更宽的预览中正文居中、两侧留白。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextWidth {
    /// 适合连续阅读的宽度。
    #[default]
    Standard,
    /// 比标准宽，适合宽表格和代码较多的文档。
    Wide,
    /// 铺满预览区域。
    Full,
}

impl TextWidth {
    /// 全部选项，顺序与设置中菜单的顺序一致。
    pub const ALL: [TextWidth; 3] = [TextWidth::Standard, TextWidth::Wide, TextWidth::Full];
}

/// 背景图在窗口中的显示方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackgroundFit {
    /// 等比缩放并裁剪，铺满窗口。
    #[default]
    Fill,
    /// 等比缩放到整张可见。
    Fit,
    /// 拉伸到窗口的大小。
    Stretch,
    /// 按原始大小平铺。
    Tile,
}

impl BackgroundFit {
    /// 全部选项，顺序与设置中菜单的顺序一致。
    pub const ALL: [BackgroundFit; 4] = [
        BackgroundFit::Fill,
        BackgroundFit::Fit,
        BackgroundFit::Stretch,
        BackgroundFit::Tile,
    ];
}

/// 配色方案的选择。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeChoice {
    /// 跟随系统的浅色或深色外观。
    Auto,
    /// 强制使用深色配色。
    Dark,
    /// 强制使用浅色配色。
    Light,
}

/// 写回之前另存的原文件（同目录下的 `config.json.bak`）。
#[derive(Debug, PartialEq)]
pub struct Backup {
    /// 另存的位置。
    pub path: PathBuf,
    /// 原文件可以解析但含有注释（写出的文件不保留注释）时为真；原文件无法解析时为假。
    pub comments: bool,
}
