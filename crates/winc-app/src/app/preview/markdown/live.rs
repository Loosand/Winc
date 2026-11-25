//! Markdown 预览中的实时编辑适配器：将窗口按键与菜单操作转发至 `winc_richtext::commands`。

use winc_richtext::{BackspaceAction, DeleteAction, EditPlan, commands};

use super::*;

impl App {
    /// 可编辑的预览中 ↑↓ 按显示行移动光标（加 ⇧ 扩展选区），`PageUp` / `PageDown` 移动一屏；
    /// 正文里的回车、⇧ 回车见 [`App::live_enter`]。
    pub(super) fn live_key(&mut self, key: &Key, cmd: bool, area: winc_render::Rect) -> bool {
        self.parse_markdown_inline();
        // macOS 上 ⌘← / ⌘→ 与 Home / End 相同，移到显示行的开头、末尾。
        if cfg!(target_os = "macos") && cmd && !self.pointer.modifiers.alt_key() {
            match key {
                Key::Named(NamedKey::ArrowLeft) => return self.live_line_edge(false),
                Key::Named(NamedKey::ArrowRight) => return self.live_line_edge(true),
                _ => {}
            }
        }
        // 代码块、公式中 ⌘↩ 在块之后另起一段。
        if cmd
            && !self.pointer.modifiers.alt_key()
            && !self.pointer.modifiers.shift_key()
            && *key == Key::Named(NamedKey::Enter)
            && self.live_leave_fenced()
        {
            return true;
        }
        // 表格中 ⌘↩ 在下方插入一行，⌘⇧⌫ 删除当前行。
        if cmd && !self.pointer.modifiers.alt_key() && self.live_leaf().is_some_and(|l| l.table) {
            let shift = self.pointer.modifiers.shift_key();
            match key {
                Key::Named(NamedKey::Enter) if !shift => return self.live_table_tool("row"),
                Key::Named(NamedKey::Backspace) if shift => {
                    return self.live_table_tool("delete-row");
                }
                _ => {}
            }
        }
        if cmd || self.pointer.modifiers.alt_key() {
            return false;
        }
        use winc_preview::markdown::cells::Step;
        let shift = self.pointer.modifiers.shift_key();
        match key {
            Key::Named(NamedKey::Tab) => {
                return self.live_table_step(if shift { Step::Previous } else { Step::Next })
                    || self.live_list_shift(shift);
            }
            Key::Named(NamedKey::Enter) if !shift && self.live_table_step(Step::Down) => {
                return true;
            }
            Key::Named(NamedKey::Enter) => return self.live_enter(shift),
            Key::Named(NamedKey::Backspace) => return self.live_backspace(),
            Key::Named(NamedKey::Delete) => return self.live_delete(),
            Key::Named(NamedKey::Home) => return self.live_line_edge(false),
            Key::Named(NamedKey::End) => return self.live_line_edge(true),
            Key::Named(NamedKey::ArrowLeft) => return self.live_horizontal(false),
            Key::Named(NamedKey::ArrowRight) => return self.live_horizontal(true),
            Key::Named(NamedKey::Space) if !shift && self.live_space() => return true,
            Key::Character(c) if c == " " && !shift && self.live_space() => return true,
            Key::Character(c) if (c == "`" || c == "~") && self.live_fence_char(c) => {
                return true;
            }
            _ => {}
        }
        let page = (area.h * 0.9).max(self.fonts.line_height);
        let (down, by) = match key {
            Key::Named(NamedKey::ArrowUp) => (false, None),
            Key::Named(NamedKey::ArrowDown) => (true, None),
            Key::Named(NamedKey::PageUp) => (false, Some(page)),
            Key::Named(NamedKey::PageDown) => (true, Some(page)),
            _ => return false,
        };
        self.live_vertical(down, by);
        true
    }

    /// 将 `winc-richtext` 产出的 [`EditPlan`] 应用到当前活动文档。
    pub(super) fn apply_edit_plan(&mut self, plan: EditPlan, record_table: bool) -> bool {
        if plan.slide {
            self.preview.markdown.slide = true;
        }
        let previous_revision = self.editor.active().map(|d| d.revision);
        let changed = !plan.replace.is_empty() || !plan.insert.is_empty();
        if let Some(doc) = self.editor.active_mut() {
            doc.collapse_selections();
            doc.selection = winc_core::Selection::new(plan.replace.start, plan.replace.end);
        } else {
            return false;
        }
        if changed {
            let cmd = if plan.paste {
                winc_editor::Command::Paste(plan.insert)
            } else {
                winc_editor::Command::Insert(plan.insert)
            };
            self.run_source_edit(cmd);
        }
        if let Some(doc) = self.editor.active_mut() {
            let chars = doc.text().rope().map_or(0, ropey::Rope::len_chars);
            doc.selection = winc_core::Selection::new(
                plan.selection.anchor.min(chars),
                plan.selection.head.min(chars),
            );
            if record_table && Some(doc.revision) != previous_revision {
                self.preview
                    .markdown
                    .live
                    .record_table_edit(doc.id, doc.revision);
            }
        }
        true
    }

    /// 执行表格工具栏或右键菜单「表格」中的操作 `op`。
    pub(crate) fn live_table_tool(&mut self, op: &str) -> bool {
        let Some(live) = self.live_leaf().filter(|l| l.table) else {
            return false;
        };
        let Some(caret) = self.live_caret_source() else {
            return false;
        };
        let Some(rope) = self.editor.active().and_then(|d| d.text().rope()) else {
            return false;
        };
        let Some(plan) = commands::plan_table_tool(rope, &live, caret, op) else {
            return false;
        };
        self.apply_edit_plan(plan, true);
        self.finish_live_edit();
        true
    }

    /// 返回光标位于表格中时右键菜单的「表格」子菜单；不在表格中时为 `None`。
    pub(in crate::app) fn live_table_menu(
        &self,
    ) -> Option<Vec<crate::app::input::context_menu::MenuEntry>> {
        use crate::app::input::context_menu::{MenuCommand, MenuEntry};
        self.live_leaf().filter(|l| l.table)?;
        let item = |op| MenuEntry::Item(MenuCommand::Table(op), true);
        Some(vec![
            item("row-above"),
            item("row"),
            item("column-left"),
            item("column"),
            MenuEntry::Separator,
            item("left"),
            item("center"),
            item("right"),
            MenuEntry::Separator,
            item("delete-row"),
            item("delete-column"),
            item("delete"),
        ])
    }

    /// 光标在表格的单元格中时，按 Tab、⇧Tab、回车切换单元格。
    pub(super) fn live_table_step(&mut self, step: winc_preview::markdown::cells::Step) -> bool {
        let Some(live) = self.live_leaf().filter(|l| l.table) else {
            return false;
        };
        let Some(caret) = self.live_caret_source() else {
            return false;
        };
        let Some(rope) = self.editor.active().and_then(|d| d.text().rope()) else {
            return false;
        };
        let Some(plan) = commands::plan_table_step(rope, &live, caret, step) else {
            return false;
        };
        let mut select = plan.select_bytes;
        if let Some((pos, row)) = plan.insert {
            let before = rope.len_bytes();
            if let Some(doc) = self.editor.active_mut() {
                doc.collapse_selections();
                doc.selection = winc_core::Selection::point(pos);
            }
            let len = row.len();
            self.run(winc_editor::Command::Insert(row));
            // 文档的行尾为 `\r\n` 时，插入的文字比 `row` 长。
            let after = self
                .editor
                .active()
                .and_then(|d| d.text().rope())
                .map_or(before, |r| r.len_bytes());
            let grown = (after - before).saturating_sub(len);
            select = select.start + grown..select.end + grown;
        }
        if let Some(doc) = self.editor.active_mut()
            && let Some(rope) = doc.text().rope()
        {
            let char_at =
                |b: usize| rope.byte_to_char((plan.leaf_start_byte + b).min(rope.len_bytes()));
            let (a, b) = (char_at(select.start), char_at(select.end));
            doc.goal_cell = None;
            doc.collapse_selections();
            doc.selection = winc_core::Selection::new(a, b);
        }
        self.finish_live_edit();
        self.reset_blink();
        self.request_redraw();
        true
    }

    /// 处理正文中的回车：另起一段、列表续一项、空项退出一层；按住 ⇧ 时段内换行。
    pub(super) fn live_enter(&mut self, shift: bool) -> bool {
        if !shift && let Some(handled) = self.live_fence() {
            return handled;
        }
        let leaf = self.live_leaf();
        let operation = self.markdown_operation_range();
        let Some(doc) = self.editor.active() else {
            return false;
        };
        let Some(rope) = doc.text().rope() else {
            return false;
        };
        let Some(plan) = commands::plan_enter(
            rope,
            leaf.as_ref(),
            doc.selection,
            operation,
            shift,
            doc.line_ending,
        ) else {
            return false;
        };
        let previous_revision = doc.revision;
        let start = plan.replace.start;
        let opens_empty = plan.opens_empty_paragraph;
        let trailing = plan.trailing_len;
        if plan.explicit_transaction {
            if !doc.fits_edit(plan.replace.start, plan.replace.end) {
                return true;
            }
            let transaction = plan.into_transaction(rope, doc.selection, doc.line_ending);
            if let Some(doc) = self.editor.active_mut() {
                doc.collapse_selections();
                doc.goal_cell = None;
                doc.apply(transaction);
            }
            self.relayout();
            self.ensure_caret_visible();
            self.reset_blink();
            self.request_redraw();
        } else {
            if let Some(doc) = self.editor.active_mut() {
                doc.collapse_selections();
                doc.selection = winc_core::Selection::new(plan.replace.start, plan.replace.end);
            }
            self.run_source_edit(winc_editor::Command::Insert(plan.text));
        }
        if opens_empty {
            self.remember_empty_paragraph(previous_revision, start, trailing);
        }
        self.finish_live_edit();
        true
    }

    /// 补全闭围栏：在围栏行末尾按回车时，光标移到两行围栏之间的空行。
    fn live_fence(&mut self) -> Option<bool> {
        let leaf = self.live_leaf();
        let doc = self.editor.active()?;
        let rope = doc.text().rope()?;
        if !doc.selection.is_empty() || !doc.extra_selections().is_empty() {
            return None;
        }
        let (head, tail) =
            match commands::plan_fence_enter(rope, leaf.as_ref(), doc.selection.head)? {
                Some(pair) => pair,
                None => return Some(false),
            };
        self.run(winc_editor::Command::Insert(head));
        let at = self.editor.active().map(|d| d.selection.head);
        self.run(winc_editor::Command::Insert(tail));
        if let Some(doc) = self.editor.active_mut()
            && let Some(at) = at
        {
            doc.selection = winc_core::Selection::point(at);
        }
        self.finish_live_edit();
        Some(true)
    }

    /// 按 Tab、⇧Tab 调整列表项的层级：缩进为上一项的子项，或退出一层。
    pub(super) fn live_list_shift(&mut self, outdent: bool) -> bool {
        let leaf = self.live_leaf();
        let Some(doc) = self.editor.active() else {
            return false;
        };
        if !doc.extra_selections().is_empty() {
            return false;
        }
        let Some(rope) = doc.text().rope() else {
            return false;
        };
        let Some(plan) = commands::plan_list_shift(rope, doc.selection, leaf.as_ref(), outdent)
        else {
            return false;
        };
        if let Some(plan) = plan {
            self.apply_edit_plan(plan, false);
            self.finish_live_edit();
        }
        true
    }

    /// 处理正文中的删除键。
    pub(super) fn live_backspace(&mut self) -> bool {
        if let Some(start) = self.property_body_start()
            && self
                .editor
                .active()
                .is_some_and(|doc| doc.selection.is_empty() && doc.selection.head <= start)
        {
            return true;
        }
        if self.backspace_empty_paragraph() {
            return true;
        }
        if let Some(done) = self.live_fenced_backspace() {
            return done;
        }
        let leaf = self.live_leaf();
        let Some(doc) = self.editor.active() else {
            return false;
        };
        let Some(rope) = doc.text().rope() else {
            return false;
        };
        if !doc.selection.is_empty() || !doc.extra_selections().is_empty() {
            return false;
        }
        let checked = self
            .preview
            .markdown
            .checked
            .filter(|_| self.preview.markdown.doc == Some(doc.id));
        let Some(action) = commands::plan_backspace(
            rope,
            self.preview.markdown.parsed.as_deref(),
            leaf.as_ref(),
            doc.selection.head,
            doc.revision,
            checked,
        ) else {
            return false;
        };
        self.apply_backspace_action(action);
        true
    }

    fn apply_backspace_action(&mut self, action: BackspaceAction) {
        match action {
            BackspaceAction::Consume => {}
            BackspaceAction::MoveCaret(pos) => {
                if let Some(doc) = self.editor.active_mut() {
                    doc.selection = winc_core::Selection::point(pos);
                }
                self.finish_live_edit();
                self.request_redraw();
            }
            BackspaceAction::Replace {
                range,
                insert,
                caret,
            } => {
                if let Some(doc) = self.editor.active_mut() {
                    doc.collapse_selections();
                    doc.selection = winc_core::Selection::new(range.start, range.end);
                }
                self.run_source_edit(winc_editor::Command::Insert(insert));
                if let Some(pos) = caret
                    && let Some(doc) = self.editor.active_mut()
                {
                    doc.selection = winc_core::Selection::point(pos);
                }
                self.finish_live_edit();
                self.request_redraw();
            }
        }
    }

    fn backspace_empty_paragraph(&mut self) -> bool {
        let Some(doc) = self.editor.active_mut() else {
            return false;
        };
        let Some((start, end, caret)) = self.preview.markdown.live.matching_empty_paragraph(
            doc.id,
            doc.revision,
            doc.selection,
            doc.has_cursors(),
        ) else {
            return false;
        };
        let Some(removed) = doc.text_range(start, end) else {
            return false;
        };
        if !doc.apply_changes(
            vec![winc_core::Change {
                start,
                removed,
                inserted: String::new(),
            }],
            caret,
            start,
        ) {
            return false;
        }
        let doc_id = doc.id;
        self.preview
            .markdown
            .live
            .finish_empty_paragraph_backspace(Some(doc_id));
        self.relayout();
        self.ensure_caret_visible();
        self.reset_blink();
        self.request_redraw();
        true
    }

    /// 处理正文末尾的向前删除键。
    pub(super) fn live_delete(&mut self) -> bool {
        if self.live_fenced_delete() {
            return true;
        }
        let leaf = self.live_leaf();
        let Some(doc) = self.editor.active() else {
            return false;
        };
        let Some(rope) = doc.text().rope() else {
            return false;
        };
        if !doc.selection.is_empty() || !doc.extra_selections().is_empty() {
            return false;
        }
        let checked = self
            .preview
            .markdown
            .checked
            .filter(|_| self.preview.markdown.doc == Some(doc.id));
        let Some(action) = commands::plan_delete(
            rope,
            self.preview.markdown.parsed.as_deref(),
            leaf.as_ref(),
            doc.selection.head,
            doc.revision,
            checked,
        ) else {
            return false;
        };
        match action {
            DeleteAction::Consume => {}
            DeleteAction::MoveCaret(pos) => {
                if let Some(doc) = self.editor.active_mut() {
                    doc.selection = winc_core::Selection::point(pos);
                }
                self.finish_live_edit();
                self.request_redraw();
            }
            DeleteAction::Delete(range) => {
                if let Some(doc) = self.editor.active_mut() {
                    doc.selection = winc_core::Selection::new(range.start, range.end);
                }
                self.run_source_edit(winc_editor::Command::Insert(String::new()));
                self.finish_live_edit();
                self.request_redraw();
            }
        }
        true
    }

    /// 处理正文中输入的空格（任务框简写转换为 `- [ ] `）。
    pub(super) fn live_space(&mut self) -> bool {
        if !self.preedit_at(Focus::Editor).is_empty() {
            return false;
        }
        let leaf = self.live_leaf();
        let Some(doc) = self.editor.active() else {
            return false;
        };
        if !doc.extra_selections().is_empty() {
            return false;
        }
        let Some(rope) = doc.text().rope() else {
            return false;
        };
        let Some(plan) = commands::plan_space_rule(rope, doc.selection, leaf.as_ref()) else {
            return false;
        };
        self.apply_edit_plan(plan, false);
        self.finish_live_edit();
        true
    }

    /// 处理正文中输入的 `` ` ``、`~`：自动补全闭围栏。
    pub(super) fn live_fence_char(&mut self, mark: &str) -> bool {
        if !self.preedit_at(Focus::Editor).is_empty() {
            return false;
        }
        let leaf = self.live_leaf();
        let Some(doc) = self.editor.active() else {
            return false;
        };
        if !doc.extra_selections().is_empty() {
            return false;
        }
        let Some(rope) = doc.text().rope() else {
            return false;
        };
        let Some(tail) = commands::plan_fence_char(rope, doc.selection, leaf.as_ref(), mark) else {
            return false;
        };
        self.run(winc_editor::Command::Insert(mark.to_owned()));
        let at = self.editor.active().map(|d| d.selection.head);
        self.run(winc_editor::Command::Insert(tail));
        if let Some(doc) = self.editor.active_mut()
            && let Some(at) = at
        {
            doc.selection = winc_core::Selection::point(at);
        }
        self.finish_live_edit();
        true
    }

    /// 处理围栏代码块、公式中的删除键。
    pub(super) fn live_fenced_backspace(&mut self) -> Option<bool> {
        let fenced = self.live_fenced()?;
        let doc = self.editor.active()?;
        let rope = doc.text().rope()?;
        if !doc.selection.is_empty() || !doc.extra_selections().is_empty() {
            return None;
        }
        let action = commands::plan_fenced_backspace(rope, &fenced, doc.selection.head)?;
        self.apply_backspace_action(action);
        Some(true)
    }

    /// 处理围栏代码块、公式中的向前删除键。
    pub(super) fn live_fenced_delete(&mut self) -> bool {
        let Some(fenced) = self.live_fenced() else {
            return false;
        };
        let Some(doc) = self.editor.active() else {
            return false;
        };
        let Some(rope) = doc.text().rope() else {
            return false;
        };
        if !doc.selection.is_empty() || !doc.extra_selections().is_empty() {
            return false;
        }
        commands::fenced_delete_blocked(rope, &fenced, doc.selection.head)
    }

    /// 在光标所在的围栏代码块、公式之后另起一段。
    pub(super) fn live_leave_fenced(&mut self) -> bool {
        let Some(fenced) = self.live_fenced().filter(|f| f.closed) else {
            return false;
        };
        let Some(rope) = self.editor.active().and_then(|d| d.text().rope()) else {
            return false;
        };
        let end = line_end(rope, fenced.last);
        if let Some(doc) = self.editor.active_mut() {
            doc.collapse_selections();
            doc.selection = winc_core::Selection::point(end);
        }
        self.run(winc_editor::Command::Insert("\n\n".to_owned()));
        self.finish_live_edit();
        true
    }

    /// 行内格式只作用于单行正文或表格的单个单元格内，不改动代码块等非正文块。
    pub(in crate::app) fn inline_format_available(&self) -> bool {
        let Some(doc) = self
            .editor
            .active()
            .filter(|d| !d.has_cursors() && d.readonly_reason().is_none())
        else {
            return false;
        };
        let Some(rope) = doc.text().rope() else {
            return false;
        };
        let leaf = self.markdown_live().then(|| self.live_leaf()).flatten();
        commands::inline_format_available(rope, doc.selection, leaf.as_ref())
    }

    /// 对选区切换行内格式的标记 `mark`（`**`、`*`、`` ` ``、`~~`、`==`）。
    pub(in crate::app) fn live_format(&mut self, mark: &str) {
        self.parse_markdown_inline();
        if !self.inline_format_available() {
            return;
        }
        let leaf = self.markdown_live().then(|| self.live_leaf()).flatten();
        let Some(doc) = self.editor.active() else {
            return;
        };
        if !doc.extra_selections().is_empty() {
            return;
        }
        let Some(rope) = doc.text().rope() else {
            return;
        };
        let Some(plan) = commands::plan_inline_format(rope, doc.selection, leaf.as_ref(), mark)
        else {
            return;
        };
        self.apply_edit_plan(plan, false);
        self.finish_live_edit_reset_line();
        self.reset_blink();
        self.request_redraw();
    }
}
