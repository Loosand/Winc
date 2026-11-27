//! 窗口内绘制的菜单：没有系统菜单可用时（macOS 以外的平台，以及没有窗口的测试中），
//! 右键菜单和按钮菜单在窗口里画出来，由应用自己处理悬停、点击和按键。
//!
//! 菜单项从 [`MenuEntry`] 生成，编号与系统菜单相同，选中后按编号找回命令，交给
//! [`App::run_menu_command`]；状态栏路径的目录菜单例外，各行是文件和目录
//! （[`PopupRow::Entry`]），目录停留时才读内容。只有命令的菜单可以键入文字筛选（见
//! [`PopupMenu::query`]），语言、编码这类长列表不必逐行移动。

use std::path::{Path, PathBuf};

use winit::event::{KeyEvent, MouseButton};
use winit::keyboard::{Key, NamedKey};

use winc_i18n::t;

use super::*;

/// 标签列表菜单的图标边长（逻辑像素）。
///
/// 文件图标为 12 × 15，无法放入其他菜单 15 × 12 的图标位，因此改用 16 × 16 的图标位，
/// 状态栏路径的目录菜单也使用该尺寸。
pub(super) const TAB_LIST_ICON: f32 = 16.0;

/// 菜单项左边的图标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MenuIcon {
    /// 最近打开的目录：与文件树中的目录相同的图标和颜色。
    Folder,
    NotebookFolder {
        current: bool,
    },
    ICloud,
    Typora,
    /// 最近打开的一组目录：两个叠放的目录。
    Folders,
    /// 标签列表的第几项：标签上的图标（文件类型图标，日志标签为 Git 图标）。
    Tab(usize),
    /// 目录菜单中的文件：文件类型的记号和它在深色、浅色配色下的颜色（同文件树）。
    File(icons::FileGlyph, [u32; 2]),
}

impl MenuIcon {
    /// 图标位的逻辑尺寸（宽，高）。
    pub(crate) fn size(self) -> (f32, f32) {
        match self {
            MenuIcon::Tab(_) | MenuIcon::File(..) | MenuIcon::Typora => {
                (TAB_LIST_ICON, TAB_LIST_ICON)
            }
            MenuIcon::Folder
            | MenuIcon::Folders
            | MenuIcon::NotebookFolder { .. }
            | MenuIcon::ICloud => MENU_ICON,
        }
    }
}

/// 子菜单 `items` 这一行用谁的图标：第一项打开整个目录组时用它的，其余子菜单没有图标。
pub(super) fn submenu_lead(items: &[MenuEntry]) -> Option<MenuCommand> {
    match items.first() {
        Some(&MenuEntry::Item(command @ MenuCommand::OpenGroup(_), _)) => Some(command),
        _ => None,
    }
}

/// 菜单中的一行。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PopupRow {
    Heading(String),
    /// 分隔线。
    Separator,
    /// 一条命令：文字、左边的图标、快捷键的显示文字、是否可用、是否打勾、编号（同系统菜单的
    /// tag）。
    Item {
        label: MenuLabel,
        icon: Option<MenuIcon>,
        key: String,
        enabled: bool,
        checked: bool,
        tag: isize,
    },
    /// 子菜单：标题、图标、是否打勾、子项。
    Submenu {
        label: MenuLabel,
        icon: Option<MenuIcon>,
        checked: bool,
        rows: Vec<PopupRow>,
    },
    /// 目录菜单中的一个文件或目录，含名字、路径、是否目录、是否打勾。
    ///
    /// 打勾表示当前文件及其各级上层目录。目录停留时展开内容，文件选中时打开。
    Entry {
        name: String,
        path: PathBuf,
        dir: bool,
        checked: bool,
    },
    /// 筛选时第一行写出键入的文字，前面是搜索图标；不能选中。
    Query(String),
}

impl PopupRow {
    /// 是否可以悬停和选中；分隔线和不可用的命令返回 `false`。
    pub(crate) fn selectable(&self) -> bool {
        match self {
            PopupRow::Heading(_) | PopupRow::Separator | PopupRow::Query(_) => false,
            PopupRow::Item { enabled, .. } => *enabled,
            PopupRow::Submenu { .. } | PopupRow::Entry { .. } => true,
        }
    }

    /// 是否在停留时展开子菜单：子菜单行和目录行返回 `true`。
    fn expands(&self) -> bool {
        matches!(
            self,
            PopupRow::Submenu { .. } | PopupRow::Entry { dir: true, .. }
        )
    }
}

/// 打开着的一级菜单：最外层的菜单，或者展开的子菜单。
#[derive(Debug, Clone, Default)]
pub(crate) struct PopupLevel {
    /// 各行。
    pub rows: Vec<PopupRow>,
    /// 子菜单展开自哪一行（上一级菜单里的下标）；最外层为 `None`。
    pub parent: Option<usize>,
    /// 鼠标或键盘停在的那一行。
    pub hover: Option<usize>,
    /// 内容比菜单高时向下滚动的距离（逻辑像素）。
    pub scroll: f32,
    /// 绘制时记下的外框；还没有绘制过时为空。
    pub rect: Rect,
    /// 绘制时记下的各行位置（已减去滚动距离），与 `rows` 一一对应。
    pub row_rects: Vec<Rect>,
    /// 内容的总高度（逻辑像素），绘制时记下，滚动时限制范围。
    pub content: f32,
}

/// 窗口内绘制的菜单。
#[derive(Debug, Clone)]
pub(crate) struct PopupMenu {
    /// 打开着的各级菜单，第 0 级是弹出的菜单。
    pub levels: Vec<PopupLevel>,
    /// 弹出的位置：菜单的左上角；`right_aligned` 时是右上角。
    pub at: (f32, f32),
    /// 菜单向左展开（拆分按钮、标签列表的菜单）。
    pub right_aligned: bool,
    /// 打勾的那一行对准 `at`（状态栏的缩进、行尾菜单，和弹出式按钮一样）。
    pub at_checked: bool,
    /// 第一级菜单的最小宽度（逻辑像素）；没有要求时为 0。
    pub min_width: f32,
    pub app_icon: Option<std::sync::Arc<winc_preview::image::Pixels>>,
    /// 目录菜单：当前文件的路径，展开目录时据此给它的各级上层目录打勾；其他菜单为 `None`。
    pub current: Option<PathBuf>,
    /// 键入的筛选文字；没有筛选时为空。
    ///
    /// 只在第一级各行全是命令（没有子菜单、目录项）时筛选：按名称不区分大小写地匹配，名称以
    /// 键入的文字开头的排在前面，其余保持原来的顺序。
    pub query: String,
    /// 开始筛选前第一级的全部行；没有筛选时为 `None`。
    all: Option<Vec<PopupRow>>,
}

impl PopupMenu {
    /// 由第一级菜单的各行 `rows` 创建。
    ///
    /// `at` 是弹出位置，`flags` 是 `(at_checked, right_aligned)` 两个标志。
    pub(crate) fn new(rows: Vec<PopupRow>, at: (f32, f32), flags: (bool, bool)) -> PopupMenu {
        PopupMenu {
            levels: vec![PopupLevel {
                rows,
                ..PopupLevel::default()
            }],
            at,
            at_checked: flags.0,
            right_aligned: flags.1,
            min_width: 0.0,
            app_icon: None,
            current: None,
            query: String::new(),
            all: None,
        }
    }

    /// 是否可以键入文字筛选：只展开着第一级，且其中没有子菜单和目录项。
    fn filterable(&self) -> bool {
        let rows = self.all.as_ref().unwrap_or(&self.levels[0].rows);
        self.levels.len() == 1
            && !(rows.iter()).any(|r| {
                matches!(
                    r,
                    PopupRow::Heading(_) | PopupRow::Submenu { .. } | PopupRow::Entry { .. }
                )
            })
    }

    /// 把筛选文字换成 `query`，按它重新列出第一级的各行，停在第一个能选的行上。
    ///
    /// 第一次筛选时把菜单固定在当前的位置和宽度：列出的行变少时菜单不移动、不变窄。
    fn set_query(&mut self, query: String) {
        if self.all.is_none() {
            let rect = self.levels[0].rect;
            if rect.w > 0.0 {
                (self.at, self.min_width) = ((rect.x, rect.y), rect.w);
                (self.right_aligned, self.at_checked) = (false, false);
            }
            self.all = Some(self.levels[0].rows.clone());
        }
        let all = self.all.as_deref().unwrap_or_default();
        let rows = if query.is_empty() {
            let rows = all.to_vec();
            self.all = None;
            rows
        } else {
            filtered(all, &query)
        };
        self.query = query;
        let level = &mut self.levels[0];
        level.rows = rows;
        level.scroll = 0.0;
        level.hover = level.rows.iter().position(PopupRow::selectable);
    }
}

/// `all` 中名称含有 `query`（不区分大小写）的命令，前面加一行写出键入的文字；名称以 `query`
/// 开头的排在前面。没有匹配时在键入的文字下面写一行不可选的说明。
fn filtered(all: &[PopupRow], typed: &str) -> Vec<PopupRow> {
    let query = typed.to_lowercase();
    let mut matched: Vec<(bool, &PopupRow)> = (all.iter())
        .filter_map(|row| match row {
            PopupRow::Item { label, .. } => {
                let title = label.title.to_lowercase();
                title
                    .contains(&query)
                    .then(|| (!title.starts_with(&query), row))
            }
            _ => None,
        })
        .collect();
    matched.sort_by_key(|(later, _)| *later);
    let mut rows = vec![PopupRow::Query(typed.to_owned())];
    if matched.is_empty() {
        rows.push(PopupRow::Item {
            label: MenuLabel::plain(t!("menu-filter-no-match")),
            icon: None,
            key: String::new(),
            enabled: false,
            checked: false,
            tag: 0,
        });
    }
    rows.extend(matched.into_iter().map(|(_, row)| row.clone()));
    rows
}

impl PopupMenu {
    /// 返回点 `(x, y)` 落在第几级菜单的第几行，行号是该级 `rows` 的下标。
    ///
    /// 落在菜单的空白处（上下边距）时行号为 `None`；不在任何一级菜单上时返回 `None`。
    pub(crate) fn hit(&self, x: f32, y: f32) -> Option<(usize, Option<usize>)> {
        // 后展开的子菜单压在上面，先查。
        self.levels.iter().enumerate().rev().find_map(|(l, level)| {
            if !level.rect.contains(x, y) {
                return None;
            }
            let row = level.row_rects.iter().position(|r| {
                r.contains(x, y) && r.y >= level.rect.y && r.bottom() <= level.rect.bottom() + 0.5
            });
            Some((l, row))
        })
    }

    /// 停在第 `level` 级的第 `row` 行：更深的子菜单收起；这一行是子菜单时展开它。
    fn hover(&mut self, level: usize, row: Option<usize>) {
        self.levels.truncate(level + 1);
        let current = &mut self.levels[level];
        current.hover = row.filter(|&r| current.rows[r].selectable());
        let Some(r) = current.hover else {
            return;
        };
        let rows = match &current.rows[r] {
            PopupRow::Submenu { rows, .. } => rows.clone(),
            PopupRow::Entry {
                path, dir: true, ..
            } => folder_rows(path, self.current.as_deref().unwrap_or(path)),
            _ => return,
        };
        self.levels.push(PopupLevel {
            rows,
            parent: Some(r),
            ..PopupLevel::default()
        });
    }

    /// 在最深一级菜单中把停着的行移到上一个或下一个能选的行，越过两端时绕回。
    fn step(&mut self, down: bool) {
        let level = self.levels.len() - 1;
        let current = &self.levels[level];
        let n = current.rows.len();
        if n == 0 {
            return;
        }
        let start = current.hover;
        let mut i = start.unwrap_or(if down { n - 1 } else { 0 });
        for _ in 0..n {
            i = if down { (i + 1) % n } else { (i + n - 1) % n };
            if current.rows[i].selectable() {
                break;
            }
        }
        if current.rows[i].selectable() {
            self.levels[level].hover = Some(i);
            self.reveal(level, i);
        }
    }

    /// 滚动第 `level` 级菜单，使第 `row` 行完整露出来。
    fn reveal(&mut self, level: usize, row: usize) {
        let level = &mut self.levels[level];
        let (Some(rect), true) = (level.row_rects.get(row), level.rect.h > 0.0) else {
            return;
        };
        let top = level.rect.y + POPUP_PAD;
        let bottom = level.rect.bottom() - POPUP_PAD;
        if rect.y < top {
            level.scroll -= top - rect.y;
        } else if rect.bottom() > bottom {
            level.scroll += rect.bottom() - bottom;
        }
        level.scroll = level.scroll.max(0.0);
    }
}

/// 菜单上下的内边距（逻辑像素，经验值）：菜单外框与第一行、最后一行之间各留出的空隙。
pub(crate) const POPUP_PAD: f32 = 5.0;
/// 左边打勾那一列的宽度（逻辑像素）：勾选标记宽 9、中心距列首 13，其余是与菜单外框的空隙。
pub(crate) const CHECK_COLUMN: f32 = 24.0;
/// 有图标的菜单中图标那一列的宽度（逻辑像素）：最宽的图标位 16，加上到文字的间距。
pub(crate) const ICON_COLUMN: f32 = 22.0;
/// 有图标的菜单中文字离菜单左边的距离（逻辑像素）。
pub(crate) const POPUP_TEXT_X: f32 = CHECK_COLUMN + ICON_COLUMN;

/// 返回目录菜单中目录 `dir` 的各行：文件和目录，目录与文件之间一条分隔线。
///
/// 读取失败、空目录或项数过多时有一行灰色的说明（`folder_entries`）。`current` 是当前文件，
/// 它和它的各级上层目录打勾。
pub(crate) fn folder_rows(dir: &Path, current: &Path) -> Vec<PopupRow> {
    use crate::app::status::breadcrumb::{FolderEntry, folder_entries};
    use winc_editor::EntryKind;
    folder_entries(dir, current)
        .into_iter()
        .map(|entry| match entry {
            FolderEntry::Item {
                name,
                path,
                kind,
                checked,
            } => PopupRow::Entry {
                name,
                path,
                dir: kind == EntryKind::Dir,
                checked,
            },
            FolderEntry::Separator => PopupRow::Separator,
            FolderEntry::Note(note) => PopupRow::Item {
                label: MenuLabel::plain(&note),
                icon: None,
                key: String::new(),
                enabled: false,
                checked: false,
                tag: 0,
            },
        })
        .collect()
}

impl App {
    /// 把菜单项 `entries` 变成窗口内菜单的各行；`numbered` 是已经编到的号。
    ///
    /// 编号方式与 [`MenuEntry::command_at`] 相同，因此选中后按编号就能找回命令。
    pub(in crate::app) fn popup_rows(
        &mut self,
        entries: &[MenuEntry],
        context: Context,
        numbered: &mut isize,
    ) -> Vec<PopupRow> {
        let mut rows = Vec::new();
        for entry in entries {
            match entry {
                MenuEntry::Item(command, enabled) => {
                    *numbered += 1;
                    let key = command
                        .shortcut(context)
                        .map(|s| s.label(keymap::Os::CURRENT))
                        .unwrap_or_default();
                    rows.push(PopupRow::Item {
                        label: self.menu_label(*command),
                        icon: self.menu_icon_kind(*command),
                        key,
                        enabled: *enabled,
                        checked: self.menu_checked(*command),
                        tag: *numbered,
                    });
                }
                // 按住 `⌥` 才出现的备选项不显示，只占一个编号。
                MenuEntry::Alternate(..) => *numbered += 1,
                MenuEntry::Submenu(title, items) => {
                    let children = self.popup_rows(items, context, numbered);
                    rows.push(PopupRow::Submenu {
                        label: MenuLabel::plain(title),
                        icon: submenu_lead(items).and_then(|c| self.menu_icon_kind(c)),
                        checked: false,
                        rows: children,
                    });
                }
                MenuEntry::Separator => {
                    // 开头、结尾和连着的分隔线不画。
                    if rows.last().is_some_and(|r| *r != PopupRow::Separator) {
                        rows.push(PopupRow::Separator);
                    }
                }
            }
        }
        if rows.last() == Some(&PopupRow::Separator) {
            rows.pop();
        }
        rows
    }

    /// 窗口内的菜单。
    pub(crate) fn popup(&self) -> Option<&PopupMenu> {
        self.overlays.context_menu.as_ref()?.popup.as_ref()
    }

    fn popup_mut(&mut self) -> Option<&mut PopupMenu> {
        self.overlays.context_menu.as_mut()?.popup.as_mut()
    }

    /// 关掉窗口内的菜单，不执行任何命令。
    pub(in crate::app) fn dismiss_popup(&mut self) {
        if self.popup().is_some() {
            self.overlays.context_menu = None;
            self.overlays.menu_button = None;
            self.refresh_pointer_hover();
            self.request_redraw();
        }
    }

    /// 选中编号为 `tag` 的命令：关掉菜单，执行它。
    fn choose_popup(&mut self, tag: isize) {
        let command = self
            .overlays
            .context_menu
            .as_ref()
            .and_then(|menu| MenuEntry::command_at(&menu.entries, tag));
        if let Some(menu) = &mut self.overlays.context_menu {
            menu.popup = None;
        }
        if let Some(command) = command {
            self.run_menu_command(command);
        }
        self.overlays.context_menu = None;
        self.overlays.menu_button = None;
        self.request_redraw();
    }

    /// 展开包含第三方应用图标的子菜单时，才按需加载缓存好的 32×32 图标，避免打开主菜单时阻塞。
    fn ensure_popup_app_icon(&mut self) {
        #[cfg(target_os = "macos")]
        {
            if self.window.handle.is_none() {
                return;
            }
            let Some(menu) = &mut self.overlays.context_menu else {
                return;
            };
            let (
                MenuTarget::NotebookWorkspaces {
                    typora: Some(path), ..
                },
                Some(popup),
            ) = (&menu.target, &mut menu.popup)
            else {
                return;
            };
            if popup.app_icon.is_some() {
                return;
            }
            let needs_typora = popup.levels.iter().any(|level| {
                level.rows.iter().any(|row| {
                    matches!(
                        row,
                        PopupRow::Item {
                            icon: Some(MenuIcon::Typora),
                            ..
                        }
                    )
                })
            });
            if needs_typora {
                popup.app_icon = crate::macos::application_icon_pixels(path);
            }
        }
    }

    /// 鼠标移到 `(x, y)`；窗口内的菜单打开时返回 `true`，此时只有菜单响应悬停。
    pub(in crate::app) fn popup_mouse_move(&mut self, x: f32, y: f32) -> bool {
        let Some(popup) = self.popup_mut() else {
            return false;
        };
        let before_len = popup.levels.len();
        let before_hover: Vec<(Option<usize>, Option<usize>)> =
            popup.levels.iter().map(|l| (l.parent, l.hover)).collect();
        if let Some((level, row)) = popup.hit(x, y) {
            let same = popup.levels.len() > level + 1
                && popup.levels[level + 1].parent == row
                && row.is_some();
            // 停在已经展开的子菜单的那一行上：子菜单不动。
            if !same {
                popup.hover(level, row);
            }
        } else if let Some(last) = popup.levels.last_mut() {
            // 移出菜单：最深一级的停留取消，已经展开的子菜单留着。
            last.hover = None;
        }
        let changed = popup.levels.len() != before_len
            || popup
                .levels
                .iter()
                .map(|l| (l.parent, l.hover))
                .ne(before_hover);
        if changed {
            self.ensure_popup_app_icon();
            self.request_overlay_redraw();
        }
        true
    }

    /// 按下鼠标按键 `button`；这次按下由菜单处理时返回 `true`。
    ///
    /// 点在可用的命令上执行它，点在子菜单上展开它；左键点在菜单外面关掉菜单，这次按下不再
    /// 交给下面；右键点在菜单外面时先关掉菜单，再把事件交给下面，在新的位置弹出那里的菜单。
    pub(in crate::app) fn popup_mouse_down(&mut self, button: MouseButton) -> bool {
        let (x, y) = self.pointer.mouse;
        let Some(popup) = self.popup_mut() else {
            return false;
        };
        if button == MouseButton::Right && popup.hit(x, y).is_none() {
            self.dismiss_popup();
            return false;
        }
        let outcome = match popup.hit(x, y) {
            Some((level, Some(row))) => match &popup.levels[level].rows[row] {
                PopupRow::Item {
                    enabled: true, tag, ..
                } => Outcome::Choose(*tag),
                PopupRow::Entry {
                    path, dir: false, ..
                } => Outcome::Open(path.clone()),
                r if r.expands() => {
                    popup.hover(level, Some(row));
                    Outcome::Redraw
                }
                _ => Outcome::Redraw,
            },
            Some((_, None)) => Outcome::Redraw,
            None => Outcome::Dismiss,
        };
        self.popup_outcome(outcome);
        true
    }

    /// 执行菜单上一次操作的结果。
    fn popup_outcome(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Redraw => {
                self.ensure_popup_app_icon();
                self.request_overlay_redraw();
            }
            Outcome::Choose(tag) => self.choose_popup(tag),
            Outcome::Open(path) => {
                self.dismiss_popup();
                self.open_path(&path);
            }
            Outcome::Dismiss => self.dismiss_popup(),
        }
    }

    /// 滚轮：窗口内的菜单打开时返回 `true`，滚动鼠标下的那一级菜单。
    pub(in crate::app) fn popup_wheel(&mut self, dy: f32) -> bool {
        let (x, y) = self.pointer.mouse;
        let Some(popup) = self.popup_mut() else {
            return false;
        };
        if let Some((level, _)) = popup.hit(x, y) {
            let level = &mut popup.levels[level];
            let max = (level.content - level.rect.h).max(0.0);
            let next = (level.scroll - dy).clamp(0.0, max);
            if (next - level.scroll).abs() > 0.01 {
                level.scroll = next;
                self.request_overlay_redraw();
            }
        }
        true
    }

    /// 按键：窗口内的菜单打开时返回 `true`，菜单之外的快捷键都不处理。
    ///
    /// `↑` `↓` 移动，`→` 展开子菜单，`←` 收起，`↩` 或空格选中，`Esc` 先收起子菜单、
    /// 再关掉菜单。只有命令的菜单中键入文字筛选（见 [`PopupMenu::query`]）：筛选时空格是文字，
    /// `⌫` 删去最后一个字，`Esc` 先清空筛选。
    pub(in crate::app) fn popup_key(&mut self, event: &KeyEvent) -> bool {
        self.popup_logical_key(&event.logical_key)
    }

    /// 同 [`App::popup_key`]，给的是按键的逻辑键。
    pub(in crate::app) fn popup_logical_key(&mut self, key: &Key) -> bool {
        let mods = self.pointer.modifiers;
        let shortcut = mods.control_key() || mods.super_key();
        let Some(popup) = self.popup_mut() else {
            return false;
        };
        let typed = match key {
            Key::Character(text) if !shortcut && popup.filterable() => {
                Some(format!("{}{text}", popup.query))
            }
            Key::Named(NamedKey::Space) if !popup.query.is_empty() => {
                Some(format!("{} ", popup.query))
            }
            Key::Named(NamedKey::Backspace) if !popup.query.is_empty() => {
                let mut query = popup.query.clone();
                query.pop();
                Some(query)
            }
            Key::Named(NamedKey::Escape) if !popup.query.is_empty() => Some(String::new()),
            _ => None,
        };
        if let Some(query) = typed.filter(|q| !q.chars().any(char::is_control)) {
            popup.set_query(query);
            self.request_overlay_redraw();
            return true;
        }
        let level = popup.levels.len() - 1;
        let hovered = popup.levels[level].hover;
        let row = hovered.map(|r| &popup.levels[level].rows[r]);
        let outcome = match key {
            Key::Named(NamedKey::ArrowDown) => {
                popup.step(true);
                Outcome::Redraw
            }
            Key::Named(NamedKey::ArrowUp) => {
                popup.step(false);
                Outcome::Redraw
            }
            Key::Named(NamedKey::ArrowRight | NamedKey::Enter | NamedKey::Space)
                if row.is_some_and(PopupRow::expands) =>
            {
                popup.hover(level, hovered);
                popup.step(true);
                Outcome::Redraw
            }
            Key::Named(NamedKey::Enter | NamedKey::Space) => match row {
                Some(PopupRow::Item {
                    enabled: true, tag, ..
                }) => Outcome::Choose(*tag),
                Some(PopupRow::Entry {
                    path, dir: false, ..
                }) => Outcome::Open(path.clone()),
                _ => Outcome::Redraw,
            },
            Key::Named(NamedKey::ArrowLeft | NamedKey::Escape) if level > 0 => {
                popup.levels.pop();
                Outcome::Redraw
            }
            Key::Named(NamedKey::Escape) => Outcome::Dismiss,
            _ => Outcome::Redraw,
        };
        self.popup_outcome(outcome);
        true
    }

    /// 状态栏缩进、行尾、编码菜单中当前使用的那一项打勾；「排除的语言」菜单中已被排除的语言打勾；
    /// 格式化工具菜单中当前使用的那一项打勾。
    pub(in crate::app) fn menu_checked(&self, command: MenuCommand) -> bool {
        if let MenuCommand::NotebookNameOrder(reverse) = command {
            return self.panels.sidebar.reverse_name_order == reverse;
        }
        if command == MenuCommand::NotebookShowHidden {
            return self.panels.sidebar.show_hidden_files;
        }
        if let MenuCommand::PropertyOption(i) = command
            && let Some(MenuTarget::NoteProperty(menu)) =
                self.overlays.context_menu.as_ref().map(|m| &m.target)
        {
            return menu
                .options
                .get(i)
                .is_some_and(|option| menu.chosen.contains(option));
        }
        if let MenuCommand::ShowTab(i) = command {
            return self.editor.active == Some(i);
        }
        if let MenuCommand::RunSelect(i) = command {
            let current = self.run.current();
            return current.is_some_and(|c| self.run.list.get(i) == Some(c));
        }
        if command == MenuCommand::ToggleBlame {
            return self
                .editor
                .active()
                .is_some_and(|d| self.git.blame.is_on(d.id));
        }
        if let MenuCommand::LogColumn(column) = command {
            return self.log_column_shown(column);
        }
        if let Some(on) = self.log_filter_checked(command) {
            return on;
        }
        if command == MenuCommand::CommitAmend {
            return self.git.commit.amend;
        }
        if command == MenuCommand::CommitTree {
            return !self.git.commit.flat();
        }
        if command == MenuCommand::CommitList {
            return self.git.commit.flat();
        }
        if command == MenuCommand::CommitChangeList {
            return !self.git.commit.staging();
        }
        if let MenuCommand::CommitSortBy(sort) = command {
            return self.git.commit.sort() == sort;
        }
        if command == MenuCommand::CommitAllRepositories {
            return self.commit_all_repositories();
        }
        if command == MenuCommand::CommitStagingArea {
            return self.git.commit.staging();
        }
        if command == MenuCommand::CommitSignOff {
            return self.commit_signs_off();
        }
        if command == MenuCommand::CommitNoVerify {
            return self.git.commit.no_verify;
        }
        if command == MenuCommand::CommitCheckTodo {
            return self.state.commit_check_todo;
        }
        if command == MenuCommand::TerminalMaximize {
            return self.terminal.maximized;
        }
        if command == MenuCommand::MarkdownLineNumbers {
            return self.markdown_line_numbers_on();
        }
        // 日志的作者筛选：现在用着的那一项。
        let query = self
            .editor
            .active()
            .and_then(|d| d.log.as_deref())
            .map(|v| &v.query);
        if let MenuCommand::FormBase(i) = command {
            return self.form_base_chosen(i);
        }
        if let MenuCommand::FormRemote(i) = command {
            return self.form_remote_chosen(i);
        }
        if let MenuCommand::StatusPlace(i) = command {
            return self.location_places().get(i).is_some_and(|p| p.current);
        }
        if let (MenuCommand::LogAuthor(i), Some(q)) = (command, query) {
            return match i.checked_sub(1) {
                None => q.authors.is_empty(),
                Some(i) => (self.git.log.authors.get(i)).is_some_and(|a| q.authors.contains(a)),
            };
        }

        // 状态栏的缩进、行尾、编码、语言菜单：当前使用的那一种。
        let status_doc = match &self.overlays.context_menu {
            Some(ContextMenu {
                target:
                    MenuTarget::Indent(id)
                    | MenuTarget::LineEnding(id)
                    | MenuTarget::Encoding(id)
                    | MenuTarget::Language(id),
                ..
            }) => self.editor.document(*id),
            _ => None,
        };
        if let Some(doc) = status_doc {
            let tab_width = self.editor.tab_width;
            return match command {
                MenuCommand::IndentUsingSpaces => matches!(doc.indent(), Indent::Spaces(_)),
                MenuCommand::IndentUsingTabs => doc.indent() == Indent::Tabs,
                MenuCommand::IndentSize(n) => doc.indent_size(tab_width) == n,
                MenuCommand::LineEnding(eol) => doc.line_ending == eol,
                MenuCommand::ReopenEncoding(i) | MenuCommand::SaveEncoding(i) => {
                    CHOICES.get(i).is_some_and(|(e, _)| *e == doc.encoding)
                }
                MenuCommand::UseLanguage(i) => (winc_syntax::languages().get(i))
                    .is_some_and(|l| std::ptr::eq(*l, doc.highlighter.language())),
                MenuCommand::UsePlainText => doc.highlighter.language().id == "text",
                _ => false,
            };
        }

        match command {
            MenuCommand::Locale(i) => {
                winc_i18n::LocaleChoice::ALL.get(i)
                    == Some(&self.config.language.unwrap_or_default())
            }
            MenuCommand::KeymapPreset(preset) => crate::keymap::current_preset() == preset,
            MenuCommand::Whitespace(i) => {
                winc_config::RenderWhitespace::ALL.get(i)
                    == Some(&self.config.render_whitespace.unwrap_or_default())
            }
            MenuCommand::VimClipboard(i) => {
                winc_config::VimClipboard::ALL.get(i)
                    == Some(&self.config.vim_clipboard.unwrap_or_default())
            }
            MenuCommand::OpenWorktree(i) => {
                winc_config::WorktreeOpen::ALL.get(i)
                    == Some(&self.config.git_open_worktree.unwrap_or_default())
            }
            MenuCommand::DropFolder(i) => {
                winc_config::FolderDrop::ALL.get(i)
                    == Some(&self.config.drop_folder.unwrap_or_default())
            }
            MenuCommand::AutoSave(i) => {
                winc_config::AutoSave::ALL.get(i)
                    == Some(&self.config.auto_save.unwrap_or_default())
            }
            MenuCommand::DefaultShell(i) => {
                let configured = self.configured_shell().map(|shell| shell.command);
                let listed = i.and_then(|i| self.terminal.shells.get(i));
                configured == listed.map(|shell| shell.command.clone())
            }
            MenuCommand::DefaultShellCustom => self.configured_shell().is_some_and(|configured| {
                (self.terminal.shells.iter()).all(|shell| shell.command != configured.command)
            }),
            MenuCommand::TerminalPosition(right) => self.terminal_right() == right,
            MenuCommand::TerminalTabPosition(right) => {
                (self.config.terminal_vertical_tabs == Some(true)) == right
            }
            MenuCommand::Zoom(i) => {
                crate::app::settings::ZOOM_STEPS.get(i) == Some(&self.config.ui_zoom.unwrap_or(100))
            }
            MenuCommand::BackgroundNone => self.config.ui_background_image.is_none(),
            MenuCommand::TextWidth(i) => {
                winc_config::TextWidth::ALL.get(i)
                    == Some(&self.config.markdown_text_width.unwrap_or_default())
            }
            MenuCommand::BackgroundFit(i) => {
                winc_config::BackgroundFit::ALL.get(i)
                    == Some(&self.config.ui_background_fit.unwrap_or_default())
            }
            MenuCommand::OptionAsMeta(i) => {
                let current =
                    (self.config.option_as_meta).unwrap_or(winc_config::OptionAsMeta::Left);
                winc_config::OptionAsMeta::ALL.get(i) == Some(&current)
            }
            MenuCommand::ThemeDefault => true,
            MenuCommand::BodyFontDefault => self.config.markdown_body_font.is_none(),
            MenuCommand::BodyFont(i) => {
                let current = winc_config::font_list(self.config.markdown_body_font.as_deref())
                    .first()
                    .copied();
                winc_render::installed_fonts()
                    .get(i)
                    .is_some_and(|f| current == Some(f.name.as_str()))
            }
            MenuCommand::CodeFontDefault => self.config.editor_font.is_none(),
            MenuCommand::CodeFont(i) => {
                let current = winc_config::font_list(self.config.editor_font.as_deref())
                    .first()
                    .copied();
                winc_render::installed_fonts()
                    .get(i)
                    .is_some_and(|f| current == Some(f.name.as_str()))
            }
            MenuCommand::LineHeight(None) => self.config.markdown_line_height.is_none(),
            MenuCommand::LineHeight(Some(i)) => {
                crate::layout::LINE_HEIGHT_OPTIONS.get(i).is_some_and(|&v| {
                    self.config
                        .markdown_line_height
                        .is_some_and(|cur| (cur - v).abs() < 0.005)
                })
            }
            MenuCommand::ContentWidth(None) => self.config.markdown_content_width.is_none(),
            MenuCommand::ContentWidth(Some(i)) => {
                crate::layout::CONTENT_WIDTH_OPTIONS.get(i)
                    == self.config.markdown_content_width.as_ref()
            }
            MenuCommand::ParagraphSpacing(None) => self.config.markdown_paragraph_spacing.is_none(),
            MenuCommand::ParagraphSpacing(Some(i)) => crate::layout::PARAGRAPH_SPACING_OPTIONS
                .get(i)
                .is_some_and(|&v| {
                    self.config
                        .markdown_paragraph_spacing
                        .is_some_and(|cur| (cur - v).abs() < 0.005)
                }),
            MenuCommand::FontDefault | MenuCommand::Font(_) | MenuCommand::FallbackFont(_) => {
                let Some(ContextMenu {
                    target: MenuTarget::Font(setting),
                    ..
                }) = &self.overlays.context_menu
                else {
                    return false;
                };
                // 字体菜单只看列表中的第一个字体，「后备字体」看其余的字体。
                let list = winc_config::font_list(self.font_value(*setting).as_deref());
                let name = |i: usize| winc_render::installed_fonts().get(i).map(|f| &*f.name);
                match command {
                    MenuCommand::Font(i) => name(i).is_some_and(|n| list.first() == Some(&n)),
                    MenuCommand::FallbackFont(i) => {
                        name(i).is_some_and(|n| list.iter().skip(1).any(|&m| m == n))
                    }
                    _ => list.is_empty(),
                }
            }
            _ => false,
        }
    }

    /// 返回菜单项左边的图标：最近的目录用目录图标，工作树用分支图标，标签列表用标签上的图标；
    /// 其他命令没有图标。
    pub(in crate::app) fn menu_icon_kind(&self, command: MenuCommand) -> Option<MenuIcon> {
        match command {
            MenuCommand::OpenRecent(i) | MenuCommand::OpenRecentInNewWindow(i) => {
                let group = (self.state.recent.get(i))
                    .is_some_and(|folder| self.recent_group(folder).is_some());
                Some(if group {
                    MenuIcon::Folders
                } else {
                    MenuIcon::Folder
                })
            }
            MenuCommand::OpenGroup(_) => Some(MenuIcon::Folders),
            MenuCommand::GroupFolder(..) => Some(MenuIcon::Folder),
            MenuCommand::SwitchNotebookWorkspace(i) => {
                Some(MenuIcon::NotebookFolder { current: i == 0 })
            }
            MenuCommand::NotebookCloud => Some(MenuIcon::ICloud),
            MenuCommand::NotebookTypora => Some(MenuIcon::Typora),
            MenuCommand::ShowTab(i) => self.editor.documents.get(i).map(|_| MenuIcon::Tab(i)),
            _ => None,
        }
    }
}

/// 菜单上一次操作的结果。
///
/// 执行命令前要先关掉菜单，因此把结果先记下，关掉菜单之后再做。
enum Outcome {
    /// 只需重画。
    Redraw,
    /// 选中了此编号的命令。
    Choose(isize),
    /// 目录菜单中选中了该文件。
    Open(PathBuf),
    /// 关掉菜单。
    Dismiss,
}
