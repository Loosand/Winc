//! 采用配置文件中的新配置：按变化的项更新本窗口。

use super::*;

impl App {
    /// 改用配置文件中刚写好的配置 `config`，按变化的项更新本窗口。
    ///
    /// `config` 由另一个窗口写入，或者是本窗口写回时文件中带有手动修改的项。更新的内容包括字体和
    /// 字号、Tab 宽度、主题和各开关，效果与在本窗口的设置页中修改相同，只是不再写文件。
    pub(crate) fn adopt_config(&mut self, config: Config) {
        let old = std::mem::replace(&mut self.config, config);
        let new = &self.config;
        if old.keymap != new.keymap || old.shortcuts != new.shortcuts {
            for problem in crate::keymap::install_from(new) {
                eprintln!("winc: {problem}");
            }
            self.keymap_changed();
        }
        let new = &self.config;
        let zoom = old.ui_zoom != new.ui_zoom;
        let fonts = old.editor_font != new.editor_font
            || old.markdown_body_font != new.markdown_body_font
            || old.editor_size != new.editor_size
            || old.markdown_custom_font_size != new.markdown_custom_font_size
            || old.markdown_font_size != new.markdown_font_size
            || old.terminal_font != new.terminal_font
            || old.terminal_size != new.terminal_size;
        let theme = (old.theme != new.theme || old.dark_theme != new.dark_theme)
            || old.light_theme != new.light_theme;
        let tab_width = (old.tab_width != new.tab_width)
            .then_some(new.tab_width)
            .flatten();
        let terminal = old.terminal_minimum_contrast != new.terminal_minimum_contrast
            || old.terminal_bold_is_bright != new.terminal_bold_is_bright;
        let (wrap, sticky) = (
            old.word_wrap != new.word_wrap,
            old.sticky_scroll != new.sticky_scroll,
        );
        let images = old.markdown_remote_images != new.markdown_remote_images;
        let relayout = old.line_numbers != new.line_numbers
            || old.status_bar != new.status_bar
            || old.outline != new.outline;
        let hide_outline = old.outline != new.outline && new.outline == Some(false);
        let stop_preview_tabs =
            old.preview_on_click != new.preview_on_click && new.preview_on_click == Some(false);
        let compact = old.compact_folders != new.compact_folders;
        let diff_layout = old.diff_inline != new.diff_inline;
        let staging_view = old.git_staging_area != new.git_staging_area;
        // 窗口字体同步。
        let glyph_language = old.language != new.language
            && crate::set_locale(new.language.unwrap_or_default().resolve());
        let git = old.git_enabled != new.git_enabled;
        let git_program = old.git_path != new.git_path;
        let forget_credentials = old.git_remember_credentials != Some(false)
            && new.git_remember_credentials == Some(false);
        let title_bar = old.ui_native_title_bar != new.ui_native_title_bar;
        let stop_terminal =
            old.terminal_enabled != new.terminal_enabled && new.terminal_enabled == Some(false);

        if fonts || glyph_language {
            // 别的窗口已经用上了这些字体，这里也用得上；万一用不上，留着原来的字体。
            let _ = self.apply_fonts();
        }
        if zoom {
            self.apply_zoom();
        }
        if let Some(width) = tab_width {
            self.editor.tab_width = width;
            self.ensure_caret_visible();
        }
        if theme {
            self.apply_theme(self.config.theme.unwrap_or(ThemeChoice::Auto));
        } else if terminal {
            self.terminal_theme_changed();
        }
        if sticky {
            self.caches.sticky_cache = None;
        }
        if wrap {
            self.word_wrap_changed();
        }
        if relayout {
            self.relayout();
        }
        if hide_outline && self.focus == Focus::Outline {
            self.focus = Focus::Tree;
        }
        if stop_preview_tabs {
            self.preview_tab = None;
        }
        if compact {
            self.apply_compact_folders();
        }
        if diff_layout {
            self.apply_diff_layout();
        }
        if staging_view {
            self.apply_staging_view();
        }
        winc_editor::proxy::follow_system_proxy(self.config.system_proxy != Some(false));
        if images {
            self.preview.markdown.forget_images();
        }
        if git {
            self.git_folder_changed();
        } else if git_program {
            self.git_program_changed();
        }
        if forget_credentials {
            crate::app::git::question::forget_credentials();
        }
        if stop_terminal {
            self.stop_terminal();
        }
        // 模块关闭后它的那一页不再列出：设置停在该页时改到「模块」页。
        let page = self.settings.selected.page();
        if self.settings.open && !SettingsPage::listed(self.settings_modules()).any(|p| p == page) {
            self.settings_show_page(SettingsPage::Modules);
        }
        self.reload_run_configs();
        if title_bar {
            self.sync_title_bar();
        }
        self.vim_apply_config();
        self.relayout();
        self.request_redraw();
    }

    /// 设置界面语言（`language`）。
    ///
    /// 界面语言对整个进程生效，各窗口在下一帧使用新语言的文案；菜单栏在下次启动时更新。
    pub(in crate::app) fn set_locale_choice(&mut self, choice: winc_i18n::LocaleChoice) {
        self.config.language = Some(choice);
        if crate::set_locale(choice.resolve()) {
            // 汉字改按新的语言选字形，字体要重新加载；新字体总能加载，失败时留着原来的字体。
            let _ = self.apply_fonts();
        }
        self.save_config();
        self.request_redraw();
    }

    /// 设置 `git` 程序的路径；`None` 表示在 `PATH` 里查找。随后重新查找 `git` 并刷新。
    pub(in crate::app) fn set_git_path(&mut self, path: Option<String>) {
        if self.config.git_path == path {
            return;
        }
        self.config.git_path = path;
        self.save_config();
        self.git_program_changed();
        self.request_redraw();
    }

    /// 设置点击工作树时在哪个窗口打开它。
    pub(in crate::app) fn set_open_worktree(&mut self, choice: winc_config::WorktreeOpen) {
        self.config.git_open_worktree = Some(choice);
        self.save_config();
        self.request_redraw();
    }

    /// 设置 Vim 模式下无名寄存器与系统剪贴板的同步方式。
    pub(in crate::app) fn set_vim_clipboard(&mut self, choice: winc_config::VimClipboard) {
        self.config.vim_clipboard = Some(choice);
        self.vim_apply_config();
        self.save_config();
        self.request_redraw();
    }

    /// 设置何时显示空格和制表符。
    pub(in crate::app) fn set_render_whitespace(&mut self, choice: winc_config::RenderWhitespace) {
        self.vim_forget_option(Setting::RenderWhitespace);
        self.config.render_whitespace = Some(choice);
        self.save_config();
        self.request_redraw();
    }

    /// 设置新建终端时运行的命令（`terminal.shell`）；`None` 表示系统默认。已打开的终端不受影响。
    pub(in crate::app) fn set_terminal_shell(&mut self, command: Option<String>) {
        self.settings.selected = Setting::TerminalShell;
        self.config.terminal_shell = command;
        self.save_config();
        self.request_redraw();
    }

    /// 设置终端面板的位置：`right` 为编辑区右侧，否则为下方。
    pub(in crate::app) fn set_terminal_position(&mut self, right: bool) {
        if right != self.terminal_right() {
            self.toggle_terminal_position();
        }
    }

    /// 设置终端标签的位置：`right` 为右侧，否则为顶部。
    pub(in crate::app) fn set_terminal_tab_position(&mut self, right: bool) {
        self.settings.selected = Setting::TerminalTabPosition;
        if right == (self.config.terminal_vertical_tabs == Some(true)) {
            return;
        }
        self.config.terminal_vertical_tabs = Some(right);
        self.relayout();
        self.save_config();
        self.request_redraw();
    }

    /// 设置终端中用作 Meta 键的 ⌥。
    pub(in crate::app) fn set_option_as_meta(&mut self, choice: winc_config::OptionAsMeta) {
        self.config.option_as_meta = Some(choice);
        self.save_config();
        self.request_redraw();
    }
}
