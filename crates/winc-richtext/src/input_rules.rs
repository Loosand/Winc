//! 可单步撤销的 Markdown 输入规则引擎。
//!
//! 每条输入规则在触发时产出两条连续的原子事务（[`InputRulePlan`]）：
//! 1. [`InputRulePlan::raw`]：记录用户实际键入的原始输入（如空格、第三个围栏字符或换行）；
//! 2. [`InputRulePlan::transform`]：以 [`EditKind::Other`] 记录自动结构转换，使其在撤销栈中
//!    独立于前一步键入，绝不合并。
//!
//! 由此保证按第 1 次撤销只退回自动转换、保留用户原始输入；按第 2 次撤销才撤回原始输入。

use ropey::Rope;
use winc_core::{Change, EditKind, LineEnding, Selection, Transaction};
use winc_preview::markdown::enter::{self, Fence};

use crate::layout::LiveLeaf;
use crate::mapping::line_text;

/// 单条输入规则产出的两段式原子事务：先提交原始输入，再提交可独立撤销的结构转换。
#[derive(Debug, Clone)]
pub struct InputRulePlan {
    /// 用户原始键入对应的事务。
    pub raw: Transaction,
    /// 自动结构转换对应的事务（固定为 [`EditKind::Other`]，不与前一步合并）。
    pub transform: Transaction,
}

impl InputRulePlan {
    /// 检查第一步原始输入涉及的字符范围是否满足宿主编辑限制（如大文件或保护区校验）。
    pub fn raw_range(&self) -> Option<(usize, usize)> {
        let change = self.raw.changes.first()?;
        Some((change.start, change.start + change.removed.chars().count()))
    }
}

/// 围栏开行末尾按回车时的规则判定结果。
#[derive(Debug, Clone)]
pub enum FenceEnterRule {
    /// 下方已存在对应的闭围栏，交由常规换行处理。
    Closed,
    /// 需要自动补全闭围栏（原始换行 + 可独立撤销的闭围栏插入）。
    AutoClose(Box<InputRulePlan>),
}

/// 规划空格触发的行首输入规则（如 `[]` / `-[]` / `- [` + 空格 -> `- [ ] `）。
///
/// 第 1 次撤销还原为用户敲完空格后的原文（如 `[] `），第 2 次撤销再撤回该空格。
pub fn plan_space_input_rule(
    rope: &Rope,
    selection: Selection,
    leaf: Option<&LiveLeaf>,
    line_ending: LineEnding,
) -> Option<InputRulePlan> {
    if !selection.is_empty() || leaf.is_some_and(|l| !l.prose) {
        return None;
    }
    let caret_char = selection.head.min(rope.len_chars());
    let caret = rope.char_to_byte(caret_char);
    let line = rope.byte_to_line(caret);
    let text = line_text(rope, line);
    let at = (caret - rope.line_to_byte(line)).min(text.len());
    let edit = enter::task(&text[..at], &text[at..])?;

    let a_char = rope.byte_to_char(caret - edit.before);
    let b_char = rope.byte_to_char(caret + edit.after);
    let after_raw_caret = caret_char + 1;
    let raw = Transaction::new(
        vec![Change {
            start: caret_char,
            removed: String::new(),
            inserted: " ".to_owned(),
        }],
        selection,
        Selection::point(after_raw_caret),
        EditKind::Typing,
    );

    let mut removed = String::new();
    removed.push_str(&rope.slice(a_char..caret_char).to_string());
    removed.push(' ');
    removed.push_str(&rope.slice(caret_char..b_char).to_string());
    let inserted = edit.text.replace('\n', line_ending.as_str());
    let after_transform_caret = a_char + inserted.chars().count();
    let transform = Transaction::new(
        vec![Change {
            start: a_char,
            removed,
            inserted,
        }],
        Selection::point(after_raw_caret),
        Selection::point(after_transform_caret),
        EditKind::Other,
    );

    Some(InputRulePlan { raw, transform })
}

/// 规划行首输入第三个 `` ` `` 或 `~` 时自动补全闭围栏的两段式事务。
///
/// 第 1 次撤销移除自动补全的闭围栏，保留用户输入的三个围栏字符；第 2 次撤销撤回输入字符。
pub fn plan_fence_char_rule(
    rope: &Rope,
    selection: Selection,
    leaf: Option<&LiveLeaf>,
    mark: &str,
    line_ending: LineEnding,
) -> Option<InputRulePlan> {
    if !selection.is_empty() || leaf.is_some_and(|l| !l.prose) {
        return None;
    }
    let caret_char = selection.head.min(rope.len_chars());
    let caret = rope.char_to_byte(caret_char);
    let line = rope.byte_to_line(caret);
    let current = line_text(rope, line);
    let at = (caret - rope.line_to_byte(line)).min(current.len());
    let typed = format!("{}{mark}", &current[..at]);
    if typed.len() - typed.trim_end_matches(mark).len() != 3 * mark.len() {
        return None;
    }
    let below: Vec<String> = (line + 1..rope.len_lines())
        .map(|l| line_text(rope, l))
        .collect();
    let tail = match enter::fence(&typed, &current[at..], below.iter().map(String::as_str))? {
        Fence::Open { tail, .. } => tail,
        Fence::Closed => return None,
    };

    let after_mark = caret_char + mark.chars().count();
    let raw = Transaction::new(
        vec![Change {
            start: caret_char,
            removed: String::new(),
            inserted: mark.to_owned(),
        }],
        selection,
        Selection::point(after_mark),
        EditKind::Typing,
    );
    let tail_inserted = tail.replace('\n', line_ending.as_str());
    let transform = Transaction::new(
        vec![Change {
            start: after_mark,
            removed: String::new(),
            inserted: tail_inserted,
        }],
        Selection::point(after_mark),
        Selection::point(after_mark),
        EditKind::Other,
    );

    Some(InputRulePlan { raw, transform })
}

/// 规划围栏开行末尾回车时的闭围栏自动补全。
///
/// - 返回 `None`：当前位置不是围栏开行，交由普通正文回车处理；
/// - 返回 `Some(FenceEnterRule::Closed)`：下方已闭合，按普通换行处理；
/// - 返回 `Some(FenceEnterRule::AutoClose(plan))`：先提交换行与续行前缀，再提交可独立撤销的闭围栏。
pub fn plan_fence_enter_rule(
    rope: &Rope,
    leaf: Option<&LiveLeaf>,
    selection: Selection,
    line_ending: LineEnding,
) -> Option<FenceEnterRule> {
    if !selection.is_empty() {
        return None;
    }
    let caret_char = selection.head.min(rope.len_chars());
    let line = rope.char_to_line(caret_char);
    if leaf.is_some_and(|l| !l.prose && rope.byte_to_line(l.leaf.start) != line) {
        return None;
    }
    let byte = rope.char_to_byte(caret_char);
    let start = rope.line_to_byte(line);
    let before = rope.byte_slice(start..byte).to_string();
    if !before.contains(['`', '~', '$']) {
        return None;
    }
    let after = line_text(rope, line)[byte - start..].to_owned();
    let below: Vec<String> = (line + 1..rope.len_lines())
        .map(|l| line_text(rope, l))
        .collect();
    let fence = enter::fence(&before, &after, below.iter().map(String::as_str))?;
    let (head, tail) = match fence {
        Fence::Open { head, tail } => (head, tail),
        Fence::Closed => return Some(FenceEnterRule::Closed),
    };

    let ending = line_ending.as_str();
    let head_inserted = head.replace('\n', ending);
    let tail_inserted = tail.replace('\n', ending);
    let after_head = caret_char + head_inserted.chars().count();
    let raw = Transaction::new(
        vec![Change {
            start: caret_char,
            removed: String::new(),
            inserted: head_inserted,
        }],
        selection,
        Selection::point(after_head),
        EditKind::Other,
    );
    let transform = Transaction::new(
        vec![Change {
            start: after_head,
            removed: String::new(),
            inserted: tail_inserted,
        }],
        Selection::point(after_head),
        Selection::point(after_head),
        EditKind::Other,
    );

    Some(FenceEnterRule::AutoClose(Box::new(InputRulePlan {
        raw,
        transform,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use winc_core::History;
    use winc_preview::markdown;

    use crate::mapping::SourceBridge;

    fn run_rule(text: &str, plan: InputRulePlan) -> (Rope, History, Selection) {
        let mut rope = Rope::from_str(text);
        let mut history = History::default();
        plan.raw.apply(&mut rope);
        let after_raw = plan.raw.after;
        history.push(plan.raw);
        plan.transform.apply(&mut rope);
        let after_transform = plan.transform.after;
        history.push(plan.transform);
        assert_ne!(after_raw, Selection::point(usize::MAX));
        (rope, history, after_transform)
    }

    #[test]
    fn space_task_rule_undoes_transform_first_then_space() {
        for (source, caret, expected_after, expected_undo1) in [
            ("[]", 2, "- [ ] ", "[] "),
            ("-[]", 3, "- [ ] ", "-[] "),
            ("- []", 4, "- [ ] ", "- [] "),
            ("- []", 3, "- [ ] ", "- [ ]"),
        ] {
            let rope = Rope::from_str(source);
            let parsed = markdown::parse(source);
            let leaf =
                SourceBridge::new(&rope, &parsed, 1, Some(1)).leaf_at(rope.char_to_byte(caret));
            let plan = plan_space_input_rule(
                &rope,
                Selection::point(caret),
                leaf.as_ref(),
                LineEnding::Lf,
            )
            .unwrap_or_else(|| panic!("应命中空格任务规则: {source:?}"));

            let (mut buf, mut history, sel) = run_rule(source, plan);
            assert_eq!(buf.to_string(), expected_after, "转换结果: {source:?}");
            assert_eq!(sel, Selection::point(expected_after.chars().count()));

            // 第 1 次撤销：只撤销自动转换，保留用户敲入的空格。
            let r1 = history.undo(&mut buf).expect("应可单步撤销转换");
            assert_eq!(buf.to_string(), expected_undo1, "第 1 次撤销: {source:?}");
            assert_eq!(r1.selection, Selection::point(caret + 1));

            // 第 2 次撤销：撤销空格，恢复原始文本。
            let r2 = history.undo(&mut buf).expect("应可撤销原始空格");
            assert_eq!(buf.to_string(), source, "第 2 次撤销: {source:?}");
            assert_eq!(r2.selection, Selection::point(caret));

            // 两次重做回到转换后的状态。
            assert!(history.redo(&mut buf).is_some());
            assert_eq!(buf.to_string(), expected_undo1);
            assert!(history.redo(&mut buf).is_some());
            assert_eq!(buf.to_string(), expected_after);
        }
    }

    #[test]
    fn fence_char_rule_undoes_closing_fence_in_single_step_for_lf_and_crlf() {
        for (ending, eol) in [(LineEnding::Lf, "\n"), (LineEnding::CrLf, "\r\n")] {
            let source = "``";
            let rope = Rope::from_str(source);
            let parsed = markdown::parse(source);
            let leaf = SourceBridge::new(&rope, &parsed, 1, Some(1)).leaf_at(2);
            let plan = plan_fence_char_rule(&rope, Selection::point(2), leaf.as_ref(), "`", ending)
                .expect("输入第三个反引号应触发自动闭合");

            let (mut buf, mut history, sel) = run_rule(source, plan);
            assert_eq!(buf.to_string(), format!("```{eol}```"));
            assert_eq!(sel, Selection::point(3));

            let r1 = history.undo(&mut buf).expect("第 1 次撤销应仅撤回闭围栏");
            assert_eq!(buf.to_string(), "```");
            assert_eq!(r1.selection, Selection::point(3));

            let r2 = history
                .undo(&mut buf)
                .expect("第 2 次撤销应撤回第三个反引号");
            assert_eq!(buf.to_string(), "``");
            assert_eq!(r2.selection, Selection::point(2));
        }
    }

    #[test]
    fn fence_enter_rule_undoes_closing_fence_and_preserves_newline() {
        for (ending, eol) in [(LineEnding::Lf, "\n"), (LineEnding::CrLf, "\r\n")] {
            let source = "```rust";
            let rope = Rope::from_str(source);
            let parsed = markdown::parse(source);
            let leaf = SourceBridge::new(&rope, &parsed, 1, Some(1)).leaf_at(source.len());
            let plan = match plan_fence_enter_rule(
                &rope,
                leaf.as_ref(),
                Selection::point(source.len()),
                ending,
            ) {
                Some(FenceEnterRule::AutoClose(plan)) => *plan,
                other => panic!("期望自动闭合围栏: {other:?}"),
            };

            let (mut buf, mut history, sel) = run_rule(source, plan);
            assert_eq!(buf.to_string(), format!("```rust{eol}{eol}```"));
            assert_eq!(sel, Selection::point(source.len() + eol.len()));

            let r1 = history.undo(&mut buf).expect("第 1 次撤销应只撤回闭围栏");
            assert_eq!(buf.to_string(), format!("```rust{eol}"));
            assert_eq!(r1.selection, Selection::point(source.len() + eol.len()));

            let r2 = history.undo(&mut buf).expect("第 2 次撤销应撤回回车换行");
            assert_eq!(buf.to_string(), source);
            assert_eq!(r2.selection, Selection::point(source.len()));
        }
    }
}
