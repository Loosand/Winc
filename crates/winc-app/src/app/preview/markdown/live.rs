//! Markdown 预览中的实时编辑适配器：将窗口按键与菜单操作转发至 `winc_richtext`。

use winc_richtext::{
    BackspaceAction, DeleteAction, FenceEnterRule, InputRulePlan, LiveKey, LiveKeyAction,
    LiveKeyModifiers, commands, input_rules,
};

use super::*;

/// 将 `winit` 按键转换为内核平台无关按键表示。
fn to_live_key(key: &Key) -> Option<LiveKey> {
    match key {
        Key::Named(NamedKey::ArrowLeft) => Some(LiveKey::ArrowLeft),
        Key::Named(NamedKey::ArrowRight) => Some(LiveKey::ArrowRight),
        Key::Named(NamedKey::ArrowUp) => Some(LiveKey::ArrowUp),
        Key::Named(NamedKey::ArrowDown) => Some(LiveKey::ArrowDown),
        Key::Named(NamedKey::PageUp) => Some(LiveKey::PageUp),
        Key::Named(NamedKey::PageDown) => Some(LiveKey::PageDown),
        Key::Named(NamedKey::Home) => Some(LiveKey::Home),
        Key::Named(NamedKey::End) => Some(LiveKey::End),
        Key::Named(NamedKey::Enter) => Some(LiveKey::Enter),
        Key::Named(NamedKey::Backspace) => Some(LiveKey::Backspace),
        Key::Named(NamedKey::Delete) => Some(LiveKey::Delete),
        Key::Named(NamedKey::Tab) => Some(LiveKey::Tab),
        Key::Named(NamedKey::Space) => Some(LiveKey::Space),
        Key::Character(s) => {
            let mut chars = s.chars();
            let ch = chars.next()?;
            chars.next().is_none().then_some(LiveKey::Char(ch))
        }
        _ => None,
    }
}

impl App {
    /// 可编辑的预览中 ↑↓ 按显示行移动光标（加 ⇧ 扩展选区），`PageUp` / `PageDown` 移动一屏；
    /// 正文里的回车、⇧ 回车见 [`App::live_enter`]。
    pub(super) fn live_key(&mut self, key: &Key, cmd: bool, area: winc_render::Rect) -> bool {
        self.parse_markdown_inline();
        let Some(live_key) = to_live_key(key) else {
            return false;
        };
        let mods = LiveKeyModifiers {
            cmd,
            shift: self.pointer.modifiers.shift_key(),
            alt: self.pointer.modifiers.alt_key(),
            macos: cfg!(target_os = "macos"),
        };
        let in_table = self.live_leaf().is_some_and(|l| l.table);
        let Some(action) = commands::route_live_key(live_key, mods, in_table) else {
            return false;
        };
        use winc_preview::markdown::cells::Step;
        match action {
            LiveKeyAction::LineEdge { end } => self.live_line_edge(end),
            LiveKeyAction::LeaveFenced => self.live_leave_fenced(),
            LiveKeyAction::TableTool(op) => self.live_table_tool(op),
            LiveKeyAction::TabOrListShift { reverse } => {
                self.live_table_step(if reverse { Step::Previous } else { Step::Next })
                    || self.live_list_shift(reverse)
            }
            LiveKeyAction::Enter { shift } => {
                (!shift && self.live_table_step(Step::Down)) || self.live_enter(shift)
            }
            LiveKeyAction::Backspace => self.live_backspace(),
            LiveKeyAction::Delete => self.live_delete(),
            LiveKeyAction::Horizontal { right } => self.live_horizontal(right),
            LiveKeyAction::SpaceRule => self.live_space(),
            LiveKeyAction::FenceChar(ch) => {
                let mut buf = [0u8; 4];
                self.live_fence_char(ch.encode_utf8(&mut buf))
            }
            LiveKeyAction::Vertical { down, page } => {
                let step = page.then(|| (area.h * 0.9).max(self.fonts.line_height));
                self.live_vertical(down, step);
                true
            }
        }
    }

    /// 按两段式事务应用可单步撤销的输入规则（先原输入，再独立的自动结构转换事务）。
    fn apply_input_rule(&mut self, rule: InputRulePlan) -> bool {
        let Some((start, end)) = rule.raw_range() else {
            return false;
        };
        let Some(doc) = self.editor.active_mut() else {
            return false;
        };
        if !doc.fits_edit(start, end) {
            return true;
        }
        doc.collapse_selections();
        doc.goal_cell = None;
        doc.apply(rule.raw);
        doc.apply(rule.transform);
        self.relayout();
        self.ensure_caret_visible();
        self.finish_live_edit();
        self.reset_blink();
        self.request_redraw();
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

    /// 补全闭围栏：在围栏行末尾按回车时，先提交换行再提交可独立撤销的闭围栏事务。
    fn live_fence(&mut self) -> Option<bool> {
        let leaf = self.live_leaf();
        let doc = self.editor.active()?;
        let rope = doc.text().rope()?;
        if !doc.selection.is_empty() || !doc.extra_selections().is_empty() {
            return None;
        }
        match input_rules::plan_fence_enter_rule(
            rope,
            leaf.as_ref(),
            doc.selection,
            doc.line_ending,
        )? {
            FenceEnterRule::Closed => Some(false),
            FenceEnterRule::AutoClose(rule) => Some(self.apply_input_rule(*rule)),
        }
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

    /// 处理正文中输入的空格（任务框简写转换为 `- [ ] `，转换本身可单步撤销）。
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
        let Some(rule) =
            input_rules::plan_space_input_rule(rope, doc.selection, leaf.as_ref(), doc.line_ending)
        else {
            return false;
        };
        self.apply_input_rule(rule)
    }

    /// 处理正文中输入的 `` ` ``、`~`：自动补全闭围栏（自动补全可单步撤销）。
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
        let Some(rule) = input_rules::plan_fence_char_rule(
            rope,
            doc.selection,
            leaf.as_ref(),
            mark,
            doc.line_ending,
        ) else {
            return false;
        };
        self.apply_input_rule(rule)
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
        let Some(fenced) = self.live_fenced() else {
            return false;
        };
        let Some(doc) = self.editor.active() else {
            return false;
        };
        let Some(rope) = doc.text().rope() else {
            return false;
        };
        let crlf = doc.line_ending == winc_core::LineEnding::CrLf;
        let Some(plan) = commands::plan_leave_fenced(rope, &fenced, crlf) else {
            return false;
        };
        self.apply_edit_plan(plan, false);
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
