//! 文件树右键菜单：当前目标的菜单项、文案和命令执行。

use winc_i18n::t;

use super::*;
use crate::app::input::context_menu::{MenuCommand, MenuEntry, tree_menu, tree_selection_menu};

impl App {
    /// 返回右键点中文件树时的菜单项；`path` 为 `None` 时作用于根目录。
    pub(in crate::app) fn tree_menu_entries(&mut self, path: Option<&Path>) -> Vec<MenuEntry> {
        if path.is_some()
            && self
                .editor
                .workspace
                .as_ref()
                .is_some_and(|w| w.multi_selected())
        {
            let targets = self.tree_targets();
            let remote = !targets.is_empty() && targets.iter().all(|p| winc_backend::is_remote(p));
            return tree_selection_menu(self.tree_selected_files().len(), remote);
        }

        // 目录自己的一行和空白处一样：不能改名、剪切、删除。
        let folder =
            path.filter(|p| (self.editor.workspace.as_ref()).is_some_and(|ws| ws.is_folder_row(p)));
        let kind = path.filter(|_| folder.is_none()).and_then(|p| {
            let ws = self.editor.workspace.as_ref()?;
            ws.row_of(p)
                .map(|i| ws.rows()[i].kind)
                .or_else(|| self.note_in_library(p).then_some(EntryKind::File))
        });
        let can_paste = self.can_paste_into_tree();
        let remote = path.is_some_and(winc_backend::is_remote);
        let mut menu = if self.notebook_sidebar() && !remote {
            notebook_tree_menu(kind, can_paste)
        } else {
            tree_menu(kind, can_paste, remote)
        };
        if kind.is_none() && !self.notebook_sidebar() {
            menu.extend(self.folder_items(folder));
        }
        // Git 模块开着、文件在仓库里：最后加「显示文件历史」，有更改的文件再加暂存与回滚。
        if !self.notebook_sidebar()
            && let Some(path) = path
            && kind == Some(EntryKind::File)
            && self.in_repo(path)
        {
            menu.extend([
                MenuEntry::Separator,
                MenuEntry::Item(MenuCommand::FileHistory, true),
            ]);
            menu.extend(self.git_change_items(path));
        }
        menu
    }

    /// 返回文件树菜单里的「复制路径」文案。
    pub(in crate::app) fn tree_copy_path_label(&self) -> &'static str {
        if self.notebook_sidebar() {
            t!("menu-copy-file-path")
        } else {
            MenuCommand::CopyPath.label()
        }
    }

    /// 执行文件树菜单中的命令 `command`；`path` 为右键点中的项，`None` 表示根目录。
    pub(in crate::app) fn run_tree_command(&mut self, command: MenuCommand, path: Option<PathBuf>) {
        let Some(root) = self.editor.root().map(Path::to_path_buf) else {
            return;
        };
        let target = path.clone().unwrap_or_else(|| root.clone());

        // 选中了多项时（菜单只有这几项）作用于选中的各项。
        if path.is_some()
            && self
                .editor
                .workspace
                .as_ref()
                .is_some_and(|w| w.multi_selected())
        {
            let selection = self.editor.workspace.as_ref().map(|w| w.selection());
            let selection = selection.unwrap_or_default();
            match command {
                MenuCommand::OpenFiles => self.open_tree_selection(),
                MenuCommand::CompareFiles => {
                    if let [left, right] = &self.tree_selected_files()[..] {
                        self.compare_files(left, right);
                    }
                }
                MenuCommand::Cut => self.cut_entries(&self.tree_targets()),
                MenuCommand::Copy => {
                    self.copy_entries(&self.tree_targets());
                }
                MenuCommand::Delete => self.ask_delete_all(self.tree_targets(), false),
                MenuCommand::Download => self.pick(Pick::Download(self.tree_targets())),
                MenuCommand::CopyPath => self.copy_paths(&selection, false),
                MenuCommand::CopyRelativePath => self.copy_paths(&selection, true),
                _ => {}
            }
            return;
        }

        match command {
            MenuCommand::FileHistory => {
                if let Some(path) = &path {
                    self.git_file_history(path);
                }
            }
            MenuCommand::CommitCheck(on) => self.stage_paths(&[target], on),
            MenuCommand::GitRollbackFile => self.confirm_rollback(vec![target]),
            MenuCommand::NewFile | MenuCommand::NewFolder => {
                // 右键时已选中了那一行（空白处则取消选中），新建的位置由选中的行决定。
                self.start_new_entry(command == MenuCommand::NewFolder);
            }
            MenuCommand::CloseFolder if target == root => self.close_folder(),
            MenuCommand::AddFolder => self.ask_add_folder(),
            MenuCommand::RemoveFolder => self.remove_folder(&target),
            MenuCommand::Reveal => self.reveal_in_file_manager(&target),
            MenuCommand::OpenInTerminal => self.open_in_terminal(Some(&target)),
            MenuCommand::FindInFolder => self.find_in_folder(&target),
            MenuCommand::CopyPath => self.copy_path(&target, false),
            MenuCommand::CopyRelativePath => self.copy_path(&target, true),
            MenuCommand::Rename => self.start_rename(&target),
            MenuCommand::Delete => self.ask_delete(&target),
            MenuCommand::Cut => self.cut_entries(std::slice::from_ref(&target)),
            MenuCommand::Copy => self.copy_entry(&target),
            // 右键时已选中了那一行（空白处则取消选中），粘贴到的目录由选中的行决定。
            MenuCommand::Paste => self.paste_into_tree(),
            MenuCommand::Download => self.pick(Pick::Download(vec![target])),
            _ => {}
        }
    }
}

/// The notebook sidebar shows only applicable clipboard commands and keeps item actions separate.
fn notebook_tree_menu(kind: Option<EntryKind>, can_paste: bool) -> Vec<MenuEntry> {
    use MenuCommand::*;
    use MenuEntry::{Item, Separator};

    let mut menu = vec![Item(NewFile, true), Item(NewFolder, true), Separator];
    if can_paste {
        menu.extend([Item(Paste, true), Separator]);
    }
    menu.extend([Item(CopyPath, true), Item(Reveal, true)]);
    if kind.is_some() {
        menu.extend([
            Separator,
            Item(Cut, true),
            Item(Copy, true),
            Item(CopyRelativePath, true),
            Item(Rename, true),
            Item(Delete, true),
        ]);
    }
    menu
}
