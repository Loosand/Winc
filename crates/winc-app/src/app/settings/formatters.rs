//! 设置页弹出菜单的分发与项目上下文同步。

use super::*;

impl App {
    pub(crate) fn set_settings_project(&mut self, project: (Option<PathBuf>, Option<PathBuf>)) {
        self.settings.project = project;
    }

    /// 在 `setting` 所在行的下方弹出该设置项的菜单。
    pub(in crate::app) fn settings_open_menu(&mut self, setting: Setting) {
        let Some(row) = self.settings_layout().row(setting).copied() else {
            return;
        };
        let target = match setting {
            Setting::LightTheme => MenuTarget::LightTheme,
            Setting::DarkTheme => MenuTarget::DarkTheme,
            Setting::BodyFont => MenuTarget::BodyFont,
            Setting::CodeFont => MenuTarget::CodeFont,
            Setting::LineHeight => MenuTarget::LineHeight,
            Setting::ContentWidth => MenuTarget::ContentWidth,
            Setting::ParagraphSpacing => MenuTarget::ParagraphSpacing,
            Setting::RenderWhitespace => MenuTarget::Whitespace,
            Setting::VimClipboard => MenuTarget::VimClipboard,
            Setting::GitOpenWorktree => MenuTarget::OpenWorktree,
            Setting::DropFolder => MenuTarget::DropFolder,
            Setting::AutoSave => MenuTarget::AutoSave,
            Setting::TerminalPosition => MenuTarget::TerminalPosition,
            Setting::TerminalTabPosition => MenuTarget::TerminalTabPosition,
            Setting::TerminalOptionAsMeta => MenuTarget::OptionAsMeta,
            Setting::EditorFont | Setting::TerminalFont => MenuTarget::Font(setting),
            Setting::TerminalShell => {
                self.settings.selected = setting;
                return self.open_context_menu(MenuTarget::DefaultShell, menu_below(row.control));
            }
            Setting::GitPath => return self.settings_start_editing(setting),
            Setting::Updates => return self.settings_check_updates(),
            _ => return,
        };
        self.settings.selected = setting;
        let (_, y) = menu_below(row.control);
        self.open_context_menu(target, (row.control.right(), y));
    }
}
