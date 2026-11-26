//! 结构化编辑命令与按键意图路由。
//!
//! 本模块中的所有命令均基于只读原文（[`Rope`]）、解析结构（[`Markdown`]）与当前选区计算，
//! 输出纯数据编辑方案（[`EditPlan`]、[`EnterPlan`]、[`BackspaceAction`] 等），不直接依赖宿主应用或
//! 编辑器状态，从而可在无界面环境中独立运行与验证。

use std::ops::Range;

use ropey::Rope;
use winc_core::{Change, EditKind, LineEnding, Selection, Transaction};
use winc_preview::markdown::{
    Align, Block, Markdown,
    cells::{self, Change as TableChange, Step as TableStep},
    enter::{self, NextLine},
    format,
    paragraph::{self, Kind as ParagraphKind},
};

use crate::mapping::LiveLeaf;
use crate::mapping::{
    Fenced, SourceBridge, fenced_lines, is_url, leaf_at_or_before, line_end, line_text,
};

/// 实时编辑预览中可由内核识别的平台无关按键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveKey {
    /// 左方向键。
    ArrowLeft,
    /// 右方向键。
    ArrowRight,
    /// 上方向键。
    ArrowUp,
    /// 下方向键。
    ArrowDown,
    /// 向上翻页。
    PageUp,
    /// 向下翻页。
    PageDown,
    /// 行首键。
    Home,
    /// 行尾键。
    End,
    /// 回车键。
    Enter,
    /// 退格键。
    Backspace,
    /// 向前删除键。
    Delete,
    /// Tab 键。
    Tab,
    /// 空格键。
    Space,
    /// 普通字符键。
    Char(char),
}

/// 实时编辑按键的修饰键与平台上下文。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LiveKeyModifiers {
    /// 主命令键（macOS 为 ⌘，其他平台为 Ctrl）。
    pub cmd: bool,
    /// Shift 键。
    pub shift: bool,
    /// Alt / Option 键。
    pub alt: bool,
    /// 当前是否运行在 macOS 平台（决定 ⌘← / ⌘→ 是否映射为显示行首尾）。
    pub macos: bool,
}

/// 内核对实时编辑按键的语义路由结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveKeyAction {
    /// 移到当前显示行的开头或末尾。
    LineEdge {
        /// 为真时移到行尾，为假时移到行首。
        end: bool,
    },
    /// 在已闭合的围栏代码块或公式块之后另起一段。
    LeaveFenced,
    /// 执行表格结构工具命令（如 `"row"`、`"delete-row"`）。
    TableTool(&'static str),
    /// 切换表格单元格，若不在表格中则回退到列表项缩进调整。
    TabOrListShift {
        /// 是否反向（⇧Tab）。
        reverse: bool,
    },
    /// 正文、表格或围栏中的回车键（不带 Shift 时先尝试表格下行）。
    Enter {
        /// 是否按住 Shift（段内硬换行）。
        shift: bool,
    },
    /// 退格键。
    Backspace,
    /// 向前删除键。
    Delete,
    /// 水平方向移动（跳过隐藏块标记与折叠分隔行）。
    Horizontal {
        /// 为真时向右，为假时向左。
        right: bool,
    },
    /// 空格触发的输入规则。
    SpaceRule,
    /// 反引号或波浪号触发的围栏自动闭合规则。
    FenceChar(char),
    /// 竖直方向按显示行或翻页移动。
    Vertical {
        /// 为真时向下，为假时向上。
        down: bool,
        /// 为真时翻页，为假时逐行移动。
        page: bool,
    },
}

/// 根据按键、修饰键与光标是否位于表格中，路由到统一的实时编辑语义动作。
pub fn route_live_key(
    key: LiveKey,
    mods: LiveKeyModifiers,
    in_table: bool,
) -> Option<LiveKeyAction> {
    if mods.macos && mods.cmd && !mods.alt {
        match key {
            LiveKey::ArrowLeft => return Some(LiveKeyAction::LineEdge { end: false }),
            LiveKey::ArrowRight => return Some(LiveKeyAction::LineEdge { end: true }),
            _ => {}
        }
    }
    if mods.cmd && !mods.alt {
        if !mods.shift && key == LiveKey::Enter {
            return Some(if in_table {
                LiveKeyAction::TableTool("row")
            } else {
                LiveKeyAction::LeaveFenced
            });
        }
        if in_table && mods.shift && key == LiveKey::Backspace {
            return Some(LiveKeyAction::TableTool("delete-row"));
        }
        return None;
    }
    if mods.alt {
        return None;
    }
    match key {
        LiveKey::Tab => Some(LiveKeyAction::TabOrListShift {
            reverse: mods.shift,
        }),
        LiveKey::Enter => Some(LiveKeyAction::Enter { shift: mods.shift }),
        LiveKey::Backspace => Some(LiveKeyAction::Backspace),
        LiveKey::Delete => Some(LiveKeyAction::Delete),
        LiveKey::Home => Some(LiveKeyAction::LineEdge { end: false }),
        LiveKey::End => Some(LiveKeyAction::LineEdge { end: true }),
        LiveKey::ArrowLeft => Some(LiveKeyAction::Horizontal { right: false }),
        LiveKey::ArrowRight => Some(LiveKeyAction::Horizontal { right: true }),
        LiveKey::Space | LiveKey::Char(' ') if !mods.shift => Some(LiveKeyAction::SpaceRule),
        LiveKey::Char(ch @ ('`' | '~')) => Some(LiveKeyAction::FenceChar(ch)),
        LiveKey::ArrowUp => Some(LiveKeyAction::Vertical {
            down: false,
            page: false,
        }),
        LiveKey::ArrowDown => Some(LiveKeyAction::Vertical {
            down: true,
            page: false,
        }),
        LiveKey::PageUp => Some(LiveKeyAction::Vertical {
            down: false,
            page: true,
        }),
        LiveKey::PageDown => Some(LiveKeyAction::Vertical {
            down: true,
            page: true,
        }),
        LiveKey::Space | LiveKey::Char(_) => None,
    }
}

/// 统一的 Markdown 写作命令，覆盖块级样式切换、插入块结构与链接/图片生成。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WritingCommand<'a> {
    /// 切换选中各行的段落/标题/列表/引用类型。
    Paragraph(ParagraphKind),
    /// 将选中行包裹进围栏代码块或数学公式块。
    Fenced(&'a str),
    /// 插入两列 Markdown 表格。
    Table,
    /// 将单行选区做成链接或图片；可选带入剪贴板中的网址。
    Link {
        /// 剪贴板当前文本（有效网址时自动填入）。
        clipboard_url: Option<&'a str>,
        /// 是否插入图片语法。
        image: bool,
    },
    /// 替换第 `first..=last` 行并按字节偏移设定新选区。
    ReplaceLines {
        /// 起始行号（含）。
        first: usize,
        /// 结束行号（含）。
        last: usize,
        /// 替换后的文本。
        text: &'a str,
        /// 替换后选中的字节范围（相对 `text`）。
        select: Range<usize>,
    },
    /// 在第 `line` 行下方空一行插入块，光标落于 `caret_byte`。
    InsertBelow {
        /// 基准行号。
        line: usize,
        /// 插入的块文本。
        block: &'a str,
        /// 光标相对 `block` 起点的字节偏移。
        caret_byte: usize,
    },
}

/// 按统一入口规划写作命令，产出原子 [`EditPlan`]。
pub fn plan_writing(
    rope: &Rope,
    selection: Selection,
    command: WritingCommand<'_>,
    crlf: bool,
) -> Option<EditPlan> {
    match command {
        WritingCommand::Paragraph(kind) => Some(plan_paragraph_kind(rope, selection, kind, crlf)),
        WritingCommand::Fenced(fence) => Some(plan_insert_fenced(rope, selection, fence, crlf)),
        WritingCommand::Table => Some(plan_insert_table(rope, selection, crlf)),
        WritingCommand::Link {
            clipboard_url,
            image,
        } => plan_insert_link(rope, selection, clipboard_url, image, crlf),
        WritingCommand::ReplaceLines {
            first,
            last,
            text,
            select,
        } => Some(plan_replace_lines(
            rope,
            first,
            last,
            text.to_owned(),
            select,
            crlf,
        )),
        WritingCommand::InsertBelow {
            line,
            block,
            caret_byte,
        } => Some(plan_insert_below(rope, line, block, caret_byte, crlf)),
    }
}

/// 一次富文本编辑命令产出的原子文本替换方案。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditPlan {
    /// 需要替换的文档字符范围。
    pub replace: Range<usize>,
    /// 插入的新文本（统一以 `\n` 表示换行，由宿主按文档行尾风格写入）。
    pub insert: String,
    /// 替换完成后的目标选区（字符偏移）。
    pub selection: Selection,
    /// 是否按整段粘贴方式写入（避免触发单字符括号/引号自动配对）。
    pub paste: bool,
    /// 是否在重排时触发块滑动过渡动画。
    pub slide: bool,
}

/// 表格单元格导航（Tab / ⇧Tab / Enter）的计算结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableStepPlan {
    /// 需要追加新行时的插入位置（文档字符偏移）与行文本。
    pub insert: Option<(usize, String)>,
    /// 目标单元格在表格叶子块内的字节范围（相对 `leaf_start_byte`）。
    pub select_bytes: Range<usize>,
    /// 表格叶子块起点在原文中的字节偏移。
    pub leaf_start_byte: usize,
}

/// 正文回车的编辑规划结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnterPlan {
    /// 需要替换的文档字符范围。
    pub replace: Range<usize>,
    /// 替换文本：前 `generated_len` 字节为新生成的文本（需按文档行尾风格转换 `\n`），
    /// 之后的部分为后续列表重编号期间复制的原文（保持原有换行）。
    pub text: String,
    /// `text` 中需要转换换行符的前缀字节长度。
    pub generated_len: usize,
    /// `text` 中实际主编辑部分的字节长度（不含额外尾随内容与重编号复制段）。
    pub primary_len: usize,
    /// 完成后光标所在的文档字符偏移（已按 `line_ending` 计算）。
    pub caret: usize,
    /// 本次回车是否打开了一个可按退格整段收回的空段。
    pub opens_empty_paragraph: bool,
    /// 尾随保留的换行符个数（用于记录空段收回范围）。
    pub trailing_len: usize,
    /// 是否需要直接构造显式 [`Transaction`]（当光标落在插入文本中间或操作范围扩选了隐藏标记时）。
    pub explicit_transaction: bool,
}

impl EnterPlan {
    /// 按给定文档行尾风格将规划结果物化为单个原子 [`Transaction`]。
    pub fn into_transaction(
        self,
        rope: &Rope,
        before: Selection,
        line_ending: LineEnding,
    ) -> Transaction {
        let ending = line_ending.as_str();
        let mut inserted = self.text[..self.generated_len].replace('\n', ending);
        inserted.push_str(&self.text[self.generated_len..]);
        let change = Change {
            start: self.replace.start,
            removed: rope.slice(self.replace.clone()).to_string(),
            inserted,
        };
        Transaction::new(
            vec![change],
            before,
            Selection::point(self.caret),
            EditKind::Other,
        )
    }
}

/// 退格键在可编辑 Markdown 中的决策结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackspaceAction {
    /// 消费按键但不修改文本（例如光标已在属性区后正文起点或空块边界）。
    Consume,
    /// 仅移动光标（例如前一块是代码块或表格时，移到其内容末尾而不合并）。
    MoveCaret(usize),
    /// 用指定文本替换给定字符范围，并将光标落于替换后末尾（若 `caret` 指定则落于指定位置）。
    Replace {
        /// 要替换的文档字符范围。
        range: Range<usize>,
        /// 替换文本。
        insert: String,
        /// 替换后显式指定的光标字符位置；为 `None` 时留在插入文本末尾。
        caret: Option<usize>,
    },
}

/// 向前删除键（Delete）在可编辑 Markdown 中的决策结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteAction {
    /// 消费按键但不修改文本（例如围栏边界保护）。
    Consume,
    /// 仅移动光标到下一非正文块起点。
    MoveCaret(usize),
    /// 删除给定字符范围以合并下一正文块。
    Delete(Range<usize>),
}

/// 规划围栏代码块或公式块之后另起一段（⌘↩ 退出围栏）。
pub fn plan_leave_fenced(rope: &Rope, fenced: &Fenced, crlf: bool) -> Option<EditPlan> {
    if !fenced.closed {
        return None;
    }
    let end = line_end(rope, fenced.last);
    let added = if crlf { 4 } else { 2 };
    Some(EditPlan {
        replace: end..end,
        insert: "\n\n".to_owned(),
        selection: Selection::point(end + added),
        paste: false,
        slide: false,
    })
}

/// 规划在文档末尾非正文块（代码块、表格、分隔线等）之后按 ↓ 时另起一段。
///
/// 文档为空或已以换行符结束时返回 `None`（光标可直接移到末尾空行）。
pub fn plan_append_paragraph(
    rope: &Rope,
    end_prose: Option<bool>,
    open_fence: bool,
    crlf: bool,
) -> Option<EditPlan> {
    let chars = rope.len_chars();
    if chars == 0 || rope.char(chars - 1) == '\n' || end_prose != Some(false) || open_fence {
        return None;
    }
    let added = if crlf { 4 } else { 2 };
    Some(EditPlan {
        replace: chars..chars,
        insert: "\n\n".to_owned(),
        selection: Selection::point(chars + added),
        paste: false,
        slide: false,
    })
}

/// 规划单击正文下方空白时在文末打开一个可编辑空段；无需插入时返回 `None`。
///
/// 返回 `(插入起点字符偏移, 要插入的换行文本)`。
pub fn plan_open_paragraph_below_body(
    rope: &Rope,
    selection: Selection,
) -> Option<(usize, String)> {
    let chars = rope.len_chars();
    if selection != Selection::point(chars) {
        return None;
    }
    let last = rope.len_lines().checked_sub(1)?;
    let blank = |line| rope.line(line).chars().all(char::is_whitespace);
    let ending = if blank(last) {
        if last == 0 || blank(last - 1) {
            return None;
        }
        "\n"
    } else {
        "\n\n"
    };
    Some((chars, ending.to_owned()))
}

/// 将 `old` 替换为 `new` 时，只切出首尾不同的最小变更段，生成对应的 [`EditPlan`]。
pub fn diff_replace_plan(
    rope: &Rope,
    start_byte: usize,
    old: &str,
    new: &str,
    caret_byte: usize,
    slide: bool,
) -> EditPlan {
    let prefix = old
        .char_indices()
        .zip(new.chars())
        .find(|((_, a), b)| a != b)
        .map_or(old.len().min(new.len()), |((i, _), _)| i);
    let suffix = old[prefix..]
        .chars()
        .rev()
        .zip(new[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum::<usize>();
    let a = rope.byte_to_char(start_byte + prefix);
    let b = rope.byte_to_char(start_byte + old.len() - suffix);
    let insert = new[prefix..new.len() - suffix].to_owned();
    let caret_char =
        rope.byte_to_char(start_byte) + new[..caret_byte.min(new.len())].chars().count();
    EditPlan {
        replace: a..b,
        insert,
        selection: Selection::point(caret_char),
        paste: false,
        slide,
    }
}

/// 规划表格工具栏操作（增删行列、对齐、删除整表）。
pub fn plan_table_tool(
    rope: &Rope,
    leaf: &LiveLeaf,
    caret_byte: usize,
    op: &str,
) -> Option<EditPlan> {
    if !leaf.table {
        return None;
    }
    let range = leaf.leaf.start..leaf.leaf.end.min(rope.len_bytes());
    let text = rope.byte_slice(range.clone()).to_string();
    let change = match op {
        "row-above" => TableChange::RowAbove,
        "row" => TableChange::RowBelow,
        "column-left" => TableChange::ColumnLeft,
        "column" => TableChange::ColumnRight,
        "delete-row" => TableChange::DeleteRow,
        "delete-column" => TableChange::DeleteColumn,
        "left" => TableChange::Align(Align::Left),
        "center" => TableChange::Align(Align::Center),
        "right" => TableChange::Align(Align::Right),
        "none" => TableChange::Align(Align::None),
        "delete" => {
            let end = text.trim_end().len();
            let new = text[end..].trim_start_matches('\n').to_owned();
            return Some(diff_replace_plan(rope, range.start, &text, &new, 0, true));
        }
        _ => return None,
    };
    let (new, at) = cells::change(&text, caret_byte.saturating_sub(range.start), change)?;
    Some(diff_replace_plan(rope, range.start, &text, &new, at, true))
}

/// 规划表格单元格切换（Tab / ⇧Tab / Enter）。
pub fn plan_table_step(
    rope: &Rope,
    leaf: &LiveLeaf,
    caret_byte: usize,
    step: TableStep,
) -> Option<TableStepPlan> {
    if !leaf.table || !(leaf.leaf.start..=leaf.leaf.end).contains(&caret_byte) {
        return None;
    }
    let text = rope.byte_slice(leaf.leaf.clone()).to_string();
    let target = cells::step(&text, caret_byte - leaf.leaf.start, step)?;
    let insert = target.insert.map(|(at, row)| {
        (
            rope.byte_to_char((leaf.leaf.start + at).min(rope.len_bytes())),
            row,
        )
    });
    Some(TableStepPlan {
        insert,
        select_bytes: target.select,
        leaf_start_byte: leaf.leaf.start,
    })
}

/// 规划正文中的回车（另起一段、列表续项、空项退层、⇧↩ 硬换行、有序列表顺延重编号）。
pub fn plan_enter(
    rope: &Rope,
    leaf: Option<&LiveLeaf>,
    selection: Selection,
    operation_range: Option<Range<usize>>,
    shift: bool,
    line_ending: LineEnding,
) -> Option<EnterPlan> {
    if leaf.is_some_and(|l| !l.prose) {
        return None;
    }
    let chars = rope.len_chars();
    let op_range = operation_range.unwrap_or_else(|| selection.range());
    let (from, to) = (op_range.start.min(chars), op_range.end.min(chars));
    let (from, to) = (rope.char_to_byte(from), rope.char_to_byte(to));
    let from_line = rope.byte_to_line(from);
    let to_line = rope.byte_to_line(to);
    let start = rope.line_to_byte(from_line);
    let before = rope.byte_slice(start..from).to_string();
    let rest = line_text(rope, to_line);
    let after = rest[(to - rope.line_to_byte(to_line)).min(rest.len())..].to_owned();

    let (edit, trailing) = if shift {
        (enter::hard_break(&before), "")
    } else {
        let next_line = to_line + 1;
        let next = if next_line >= rope.len_lines()
            || rope.line(next_line).chars().all(char::is_whitespace)
        {
            NextLine::BlankOrEnd
        } else if leaf.is_some_and(|leaf| {
            leaf.leaf.contains(&to) && rope.line_to_byte(next_line) < leaf.leaf.end
        }) {
            NextLine::Continuation
        } else {
            NextLine::Block
        };
        let above: Vec<String> = (from_line.saturating_sub(200)..from_line)
            .rev()
            .map(|l| line_text(rope, l))
            .collect();
        enter::enter(&before, &after, next, above.iter().map(String::as_str))
    };

    let a_byte = from - edit.before;
    let b_byte = if edit.after > after.len() && to_line + 1 < rope.len_lines() {
        rope.line_to_byte(to_line + 1)
    } else {
        (to + edit.after).min(rope.len_bytes())
    };
    let item = edit.text.rsplit('\n').next().unwrap_or_default();
    let below = (to_line + 1..rope.len_lines()).map(|l| line_text(rope, l));
    let changes = if shift || !edit.text.contains('\n') {
        Vec::new()
    } else {
        enter::renumber(item, below)
    };
    let mut text = edit.text.clone();
    text.push_str(trailing);
    let mut end_byte = b_byte;
    for (i, range, number) in changes {
        let line_start = rope.line_to_byte(to_line + 1 + i);
        text.push_str(
            &rope
                .byte_slice(end_byte..line_start + range.start)
                .to_string(),
        );
        text.push_str(&number.to_string());
        end_byte = line_start + range.end;
    }
    let (a, b) = (rope.byte_to_char(a_byte), rope.byte_to_char(end_byte));
    let opens_empty_paragraph =
        a == b && selection.is_empty() && after.is_empty() && edit.text == "\n\n";
    let ending = line_ending.as_str();
    let caret = a + edit.text.replace('\n', ending).chars().count();
    let generated_len = edit.text.len() + trailing.len();
    let explicit_transaction = text.len() > edit.text.len() || op_range != selection.range();
    Some(EnterPlan {
        replace: a..b,
        text,
        generated_len,
        primary_len: edit.text.len(),
        caret,
        opens_empty_paragraph,
        trailing_len: trailing.len(),
        explicit_transaction,
    })
}

/// 规划 Tab / ⇧Tab 调整列表项缩进（连同子项与续行一起缩进）。
///
/// - `None`：当前行不是列表项或跨行选区，交由普通缩进处理；
/// - `Some(None)`：已经是目标缩进，消费按键即可；
/// - `Some(Some(plan))`：应用替换方案。
pub fn plan_list_shift(
    rope: &Rope,
    selection: Selection,
    leaf: Option<&LiveLeaf>,
    outdent: bool,
) -> Option<Option<EditPlan>> {
    if leaf.is_some_and(|l| !l.prose) {
        return None;
    }
    let chars = rope.len_chars();
    let line = rope.char_to_line(selection.head.min(chars));
    if rope.char_to_line(selection.anchor.min(chars)) != line {
        return None;
    }
    let above: Vec<String> = (line.saturating_sub(200)..line)
        .rev()
        .map(|l| line_text(rope, l))
        .collect();
    let text = line_text(rope, line);
    let (range, indent) = enter::shift(&text, above.iter().map(String::as_str), outdent)?;
    if text[range.clone()] == indent {
        return Some(None);
    }
    let start = rope.line_to_char(line);
    let (a, b) = (start + range.start, start + range.end);
    let moved = |c: usize| {
        if c >= b {
            (c + indent.len()).saturating_sub(range.len())
        } else {
            c.min(a + indent.len())
        }
    };
    let after = Selection::new(moved(selection.anchor), moved(selection.head));
    let (quote, old) = enter::margins(&text);
    let new = indent.len() - indent.trim_start_matches([' ', '\t']).len();
    let mut lines = vec![format!(
        "{}{indent}{}",
        &text[..range.start],
        &text[range.end..]
    )];
    let (mut last, mut blanks) = (line, 0);
    for l in line + 1..rope.len_lines() {
        let child = line_text(rope, l);
        if child.trim().is_empty() {
            blanks += 1;
            continue;
        }
        let (q, i) = enter::margins(&child);
        if q != quote || i <= old {
            break;
        }
        lines.extend(std::iter::repeat_n(String::new(), blanks));
        blanks = 0;
        let (head, tail) = child.split_at(q);
        lines.push(if new >= old {
            format!("{head}{}{tail}", " ".repeat(new - old))
        } else {
            format!("{head}{}", &tail[(old - new).min(i)..])
        });
        last = l;
    }
    let (replace, insert) = if last == line {
        (a..b, indent)
    } else {
        (start..line_end(rope, last), lines.join("\n"))
    };
    Some(Some(EditPlan {
        replace,
        insert,
        selection: after,
        paste: false,
        slide: false,
    }))
}

/// 规划围栏代码块或公式块中的退格键保护与拆块逻辑。
pub fn plan_fenced_backspace(
    rope: &Rope,
    fenced: &Fenced,
    caret_char: usize,
) -> Option<BackspaceAction> {
    let head = caret_char.min(rope.len_chars());
    let line = rope.char_to_line(head);
    let col = head - rope.line_to_char(line);
    let on_close = fenced.closed && line == fenced.last;
    let on_open = line == fenced.first && fenced.bare && col > 0;
    let at_start = line == fenced.first + 1 && col == 0;
    if !on_close && !on_open && !at_start {
        return None;
    }
    if fenced.empty {
        let range = rope.line_to_char(fenced.first)..line_end(rope, fenced.last);
        Some(BackspaceAction::Replace {
            range: range.clone(),
            insert: String::new(),
            caret: Some(range.start),
        })
    } else if on_close {
        Some(BackspaceAction::MoveCaret(line_end(rope, fenced.last - 1)))
    } else if on_open {
        let lines: Vec<String> = fenced.content.clone().map(|l| line_text(rope, l)).collect();
        let range = rope.line_to_char(fenced.first)..line_end(rope, fenced.last);
        Some(BackspaceAction::Replace {
            range: range.clone(),
            insert: lines.join("\n"),
            caret: Some(range.start),
        })
    } else {
        Some(BackspaceAction::Consume)
    }
}

/// 判断围栏代码块或公式块中的向前删除键（Delete）是否应被阻止以保护围栏结构。
pub fn fenced_delete_blocked(rope: &Rope, fenced: &Fenced, caret_char: usize) -> bool {
    let head = caret_char.min(rope.len_chars());
    let line = rope.char_to_line(head);
    let before_close = fenced.closed && line + 1 == fenced.last;
    head == line_end(rope, line) && (line == fenced.first || before_close)
}

/// 规划正文中的退格键（移除块标记、跨空行合并上一段、遇代码块/表格时仅移光标）。
pub fn plan_backspace(
    rope: &Rope,
    parsed: Option<&Markdown>,
    leaf: Option<&LiveLeaf>,
    caret_char: usize,
    revision: u64,
    checked: Option<u64>,
) -> Option<BackspaceAction> {
    if leaf.is_some_and(|l| !l.prose) {
        return None;
    }
    let caret = rope.char_to_byte(caret_char.min(rope.len_chars()));
    let line = rope.byte_to_line(caret);
    let start = rope.line_to_byte(line);
    let before = rope.byte_slice(start..caret).to_string();
    if let Some(edit) = enter::backspace(&before) {
        let a = rope.byte_to_char(caret - edit.before);
        let b = rope.byte_to_char(caret);
        return Some(BackspaceAction::Replace {
            range: a..b,
            insert: edit.text,
            caret: None,
        });
    }
    if line == 0 || !before.trim().is_empty() {
        return None;
    }
    let blank = |l: usize| rope.line(l).chars().all(char::is_whitespace);
    if before.is_empty() && blank(line) {
        if blank(line - 1) {
            let prev_is_content = line >= 2 && !blank(line - 2);
            // 刚好剩 1 个空段（块间 3 个空行 `A\n\n|\n\nB` 或文末 2 个换行 `A\n\n|` / `A\n\n|\n`）：
            // 退格一次删掉整个空段（2 个换行）并退回上一段末尾，避免光标落进段落分隔空行产生伪删除。
            let empty_para_between = prev_is_content
                && line + 2 < rope.len_lines()
                && blank(line + 1)
                && !blank(line + 2);
            let empty_para_at_end = prev_is_content
                && (line + 1 == rope.len_lines()
                    || (line + 2 == rope.len_lines() && rope.line(line + 1).len_chars() == 0));
            if empty_para_between || empty_para_at_end {
                let delete_from = line_end(rope, line - 2);
                let delete_to = line_end(rope, line);
                if delete_from < delete_to {
                    return Some(BackspaceAction::Replace {
                        range: delete_from..delete_to,
                        insert: String::new(),
                        caret: Some(delete_from),
                    });
                }
            }
            // 连续空行恰好剩 2 个（即 1 个额外空行 + 1 个常规段落分隔空行）且前后均有块时：
            // 删除这个额外空行并直接把光标退回上一段末尾，避免光标悬停在折叠分隔行上、
            // 再次退格时把两段误粘成同一行。
            let next_is_content = line + 1 < rope.len_lines() && !blank(line + 1);
            if prev_is_content && next_is_content {
                let delete_from = line_end(rope, line - 1);
                let delete_to = rope.line_to_char(line);
                let sep_is_collapsed = parsed.is_none_or(|md| {
                    let prev_end = rope.char_to_byte(line_end(rope, line - 2));
                    let block = md
                        .ranges
                        .partition_point(|r| r.start <= prev_end)
                        .checked_sub(1);
                    block
                        .is_none_or(|b| !matches!(md.blocks.get(b), Some(Block::Heading(1 | 2, _))))
                });
                if sep_is_collapsed && delete_from < delete_to {
                    return Some(BackspaceAction::Replace {
                        range: delete_from..delete_to,
                        insert: String::new(),
                        caret: Some(line_end(rope, line - 2)),
                    });
                }
            }
            // 多余空行仍有更多时，普通退格逐行删除。
            return None;
        }
        // 光标落在两段之间唯一的分隔空行上时，退格只将光标移回上一段末尾，不把两段强行合并为一行。
        if line + 1 < rope.len_lines() && !blank(line + 1) {
            return Some(BackspaceAction::MoveCaret(line_end(rope, line - 1)));
        }
        return None;
    }
    let to = rope.byte_to_char(caret);
    let from = if !blank(line - 1) {
        if before.is_empty() {
            return None;
        }
        line_end(rope, line - 1)
    } else {
        let above = (0..line).rev().find(|&l| !blank(l))?;
        let from = line_end(rope, above);
        let prose = parsed.and_then(|md| {
            SourceBridge::new(rope, md, revision, checked).prose_at(rope.char_to_byte(from))
        })?;
        if !prose && !blank(line) {
            let last = rope.line(above).to_string();
            let last = last.trim_end();
            let closed = parsed.and_then(|parsed| {
                let end = rope.char_to_byte(from);
                let leaf = leaf_at_or_before(parsed, end)?;
                if leaf.prose || leaf.table {
                    return None;
                }
                fenced_lines(rope, leaf.range.clone()).filter(|f| f.closed && f.last == above)
            });
            let target = if let Some(fence) = closed {
                line_end(rope, fence.last - 1)
            } else if let Some(cell) = last.strip_suffix('|') {
                from - (last.chars().count() - cell.trim_end().chars().count())
            } else {
                from
            };
            return Some(BackspaceAction::MoveCaret(target));
        }
        from
    };
    Some(BackspaceAction::Replace {
        range: from..to,
        insert: String::new(),
        caret: None,
    })
}

/// 规划正文末尾的向前删除键（Delete）：跨空行合并下一正文块，或移到下一非正文块开头。
pub fn plan_delete(
    rope: &Rope,
    parsed: Option<&Markdown>,
    leaf: Option<&LiveLeaf>,
    caret_char: usize,
    revision: u64,
    checked: Option<u64>,
) -> Option<DeleteAction> {
    if leaf.is_none_or(|l| !l.prose) {
        return None;
    }
    let head = caret_char.min(rope.len_chars());
    let line = rope.char_to_line(head);
    let blank = |l: usize| rope.line(l).chars().all(char::is_whitespace);
    if head != line_end(rope, line) || line + 1 >= rope.len_lines() || !blank(line + 1) {
        return None;
    }
    let below = (line + 1..rope.len_lines()).find(|&l| !blank(l))?;
    let bridge = parsed.map(|md| SourceBridge::new(rope, md, revision, checked));
    let hidden = bridge
        .as_ref()
        .map_or(0, |src| src.hidden_prefix_chars(below));
    let to = rope.line_to_char(below) + hidden;
    let prose = bridge?.prose_at(rope.char_to_byte(to))?;
    Some(if prose {
        DeleteAction::Delete(head..to)
    } else {
        DeleteAction::MoveCaret(to)
    })
}

/// 判断当前选区是否允许执行行内格式切换（仅限单行正文或表格单个单元格内）。
pub fn inline_format_available(
    rope: &Rope,
    selection: Selection,
    live_leaf: Option<&LiveLeaf>,
) -> bool {
    let chars = rope.len_chars();
    let (a, b) = (selection.from().min(chars), selection.to().min(chars));
    let line = rope.char_to_line(a);
    if rope.char_to_line(b) != line {
        return false;
    }
    match live_leaf {
        Some(leaf) if leaf.table => {
            let start = rope.line_to_byte(line);
            let (a, b) = (rope.char_to_byte(a) - start, rope.char_to_byte(b) - start);
            let text = line_text(rope, line);
            !cells::is_delimiter(&text)
                && cells::cells(&text)
                    .into_iter()
                    .any(|c| c.segment.start <= a && b <= c.segment.end)
        }
        Some(leaf) => leaf.prose,
        None => true,
    }
}

/// 规划行内格式切换（`**`、`*`、`` ` ``、`~~`、`==`）。
pub fn plan_inline_format(
    rope: &Rope,
    selection: Selection,
    live_leaf: Option<&LiveLeaf>,
    mark: &str,
) -> Option<EditPlan> {
    if !inline_format_available(rope, selection, live_leaf) {
        return None;
    }
    let chars = rope.len_chars();
    let (from, to) = (selection.from().min(chars), selection.to().min(chars));
    let line = rope.char_to_line(from);
    if rope.char_to_line(to) != line {
        return None;
    }
    let start = rope.line_to_byte(line);
    let text = line_text(rope, line);
    let local = |c: usize| (rope.char_to_byte(c) - start).min(text.len());
    let toggled = format::toggle(&text, local(from)..local(to), mark);
    let first = rope.line_to_char(line);
    let chars_to = |s: &str, byte: usize| first + s[..byte].chars().count();
    let (a, b) = (
        chars_to(&text, toggled.range.start),
        chars_to(&text, toggled.range.end),
    );
    let edited = format!(
        "{}{}{}",
        &text[..toggled.range.start],
        toggled.text,
        &text[toggled.range.end..]
    );
    let select = Selection::new(
        chars_to(&edited, toggled.select.start),
        chars_to(&edited, toggled.select.end),
    );
    Some(EditPlan {
        replace: a..b,
        insert: toggled.text,
        selection: select,
        paste: true,
        slide: false,
    })
}

/// 计算单选区覆盖的起止行号；无选区时另返回光标在行中的字节偏移。
pub fn selected_lines(rope: &Rope, selection: Selection) -> (usize, usize, Option<usize>) {
    let chars = rope.len_chars();
    let (from, to) = (selection.from().min(chars), selection.to().min(chars));
    let first = rope.char_to_line(from);
    let mut last = rope.char_to_line(to);
    if last > first && to == rope.line_to_char(last) {
        last -= 1;
    }
    let caret = (from == to).then(|| rope.char_to_byte(from) - rope.line_to_byte(first));
    (first, last, caret)
}

/// `text` 前 `byte` 个字节在文档里占的字符数（CRLF 下每个换行多占 1 个字符）。
fn chars_in(text: &str, byte: usize, crlf: bool) -> usize {
    let head = &text[..byte];
    head.chars().count() + if crlf { head.matches('\n').count() } else { 0 }
}

/// 构造替换第 `first..=last` 行（不含末行换行符）的 [`EditPlan`]。
pub fn plan_replace_lines(
    rope: &Rope,
    first: usize,
    last: usize,
    text: String,
    select_bytes: Range<usize>,
    crlf: bool,
) -> EditPlan {
    let start = rope.line_to_char(first);
    let end = line_end(rope, last);
    let at = |byte: usize| start + chars_in(&text, byte, crlf);
    let selection = Selection::new(at(select_bytes.start), at(select_bytes.end));
    EditPlan {
        replace: start..end,
        insert: text,
        selection,
        paste: true,
        slide: false,
    }
}

/// 构造在第 `line` 行下方空一行插入块 `block` 的 [`EditPlan`]。
pub fn plan_insert_below(
    rope: &Rope,
    line: usize,
    block: &str,
    caret_byte: usize,
    crlf: bool,
) -> EditPlan {
    let text = format!("{}\n\n{block}", line_text(rope, line));
    let head = text.len() - block.len();
    plan_replace_lines(
        rope,
        line,
        line,
        text,
        head + caret_byte..head + caret_byte,
        crlf,
    )
}

/// 规划段落类型切换（标题、引用、无序/有序/任务列表、普通段落）。
pub fn plan_paragraph_kind(
    rope: &Rope,
    selection: Selection,
    kind: ParagraphKind,
    crlf: bool,
) -> EditPlan {
    let (first, last, caret) = selected_lines(rope, selection);
    let lines: Vec<String> = (first..=last).map(|l| line_text(rope, l)).collect();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let changed = paragraph::change(&refs, kind);
    let text = changed
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let select = match caret {
        Some(at) => {
            let line = &changed[0];
            let at = if at >= line.old_start {
                at - line.old_start + line.new_start
            } else {
                line.new_start
            };
            let at = at.min(line.text.len());
            at..at
        }
        None => 0..text.len(),
    };
    plan_replace_lines(rope, first, last, text, select, crlf)
}

/// 规划插入代码块或数学公式块。
pub fn plan_insert_fenced(rope: &Rope, selection: Selection, fence: &str, crlf: bool) -> EditPlan {
    let (first, last, caret) = selected_lines(rope, selection);
    let content = (first..=last)
        .map(|l| line_text(rope, l))
        .collect::<Vec<_>>()
        .join("\n");
    if caret.is_some() && !content.trim().is_empty() {
        let (block, at) = paragraph::fenced(fence, "");
        return plan_insert_below(rope, first, &block, at, crlf);
    }
    let body = if content.trim().is_empty() {
        ""
    } else {
        &content
    };
    let (text, at) = paragraph::fenced(fence, body);
    plan_replace_lines(rope, first, last, text, at..at, crlf)
}

/// 规划插入两列 Markdown 表格。
pub fn plan_insert_table(rope: &Rope, selection: Selection, crlf: bool) -> EditPlan {
    let (first, _, _) = selected_lines(rope, selection);
    let (table, at) = paragraph::table();
    if line_text(rope, first).trim().is_empty() {
        plan_replace_lines(rope, first, first, table, at..at, crlf)
    } else {
        plan_insert_below(rope, first, &table, at, crlf)
    }
}

/// 规划插入链接或图片标记（若 `clipboard_url` 是有效网址则自动填入）。
pub fn plan_insert_link(
    rope: &Rope,
    selection: Selection,
    clipboard_url: Option<&str>,
    image: bool,
    crlf: bool,
) -> Option<EditPlan> {
    let chars = rope.len_chars();
    let (from, to) = (selection.from().min(chars), selection.to().min(chars));
    let text = rope.slice(from..to).to_string();
    if text.contains('\n') {
        return None;
    }
    let url = clipboard_url.map(str::trim).filter(|u| is_url(u));
    let (link, select) = paragraph::link(&text, url, image);
    let at = |byte: usize| from + chars_in(&link, byte, crlf);
    let selection = Selection::new(at(select.start), at(select.end));
    Some(EditPlan {
        replace: from..to,
        insert: link,
        selection,
        paste: true,
        slide: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use winc_preview::markdown;

    fn apply_plan(text: &str, plan: &EditPlan) -> String {
        let mut rope = Rope::from_str(text);
        rope.remove(plan.replace.clone());
        rope.insert(plan.replace.start, &plan.insert);
        rope.to_string()
    }

    #[test]
    fn inline_format_toggles_bold_and_respects_code_blocks() {
        let text = "hello world\n\n```rust\nlet x = 1;\n```\n";
        let rope = Rope::from_str(text);
        let parsed = markdown::parse(text);
        let bridge = SourceBridge::new(&rope, &parsed, 1, Some(1));

        let prose_leaf = bridge.leaf_at(6).unwrap();
        let plan = plan_inline_format(&rope, Selection::new(6, 11), Some(&prose_leaf), "**")
            .expect("正文内应允许加粗");
        assert_eq!(
            apply_plan(text, &plan),
            "hello **world**\n\n```rust\nlet x = 1;\n```\n"
        );
        assert_eq!(plan.selection, Selection::new(8, 13));

        let code_at = text.find("let x").unwrap();
        let code_leaf = bridge.leaf_at(code_at).unwrap();
        let code_char = rope.byte_to_char(code_at);
        assert!(
            plan_inline_format(
                &rope,
                Selection::new(code_char, code_char + 3),
                Some(&code_leaf),
                "**"
            )
            .is_none()
        );
    }

    #[test]
    fn route_live_key_dispatches_table_fence_and_navigation_intents() {
        let plain = LiveKeyModifiers::default();
        let cmd_mac = LiveKeyModifiers {
            cmd: true,
            macos: true,
            ..Default::default()
        };
        let cmd_shift = LiveKeyModifiers {
            cmd: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(
            route_live_key(LiveKey::ArrowLeft, cmd_mac, false),
            Some(LiveKeyAction::LineEdge { end: false })
        );
        assert_eq!(
            route_live_key(LiveKey::Enter, cmd_mac, false),
            Some(LiveKeyAction::LeaveFenced)
        );
        assert_eq!(
            route_live_key(LiveKey::Enter, cmd_mac, true),
            Some(LiveKeyAction::TableTool("row"))
        );
        assert_eq!(
            route_live_key(LiveKey::Backspace, cmd_shift, true),
            Some(LiveKeyAction::TableTool("delete-row"))
        );
        assert_eq!(
            route_live_key(LiveKey::Space, plain, false),
            Some(LiveKeyAction::SpaceRule)
        );
        assert_eq!(
            route_live_key(LiveKey::Char('`'), plain, false),
            Some(LiveKeyAction::FenceChar('`'))
        );
    }

    #[test]
    fn enter_renumbers_ordered_list_in_single_transaction() {
        let text = "1. 第一项\n2. 第二项\n";
        let rope = Rope::from_str(text);
        let parsed = markdown::parse(text);
        let end_first = line_end(&rope, 0);
        let leaf =
            SourceBridge::new(&rope, &parsed, 1, Some(1)).leaf_at(rope.char_to_byte(end_first));
        let plan = plan_enter(
            &rope,
            leaf.as_ref(),
            Selection::point(end_first),
            None,
            false,
            LineEnding::Lf,
        )
        .expect("有序列表回车应续出下一项");
        assert!(plan.explicit_transaction);
        let tx = plan.into_transaction(&rope, Selection::point(end_first), LineEnding::Lf);
        let mut buf = rope.clone();
        for c in &tx.changes {
            let end = c.start + c.removed.chars().count();
            buf.remove(c.start..end);
            buf.insert(c.start, &c.inserted);
        }
        assert_eq!(buf.to_string(), "1. 第一项\n2. \n3. 第二项\n");
    }

    #[test]
    fn writing_commands_and_fenced_exit_produce_atomic_plans() {
        let rope = Rope::from_str("普通段落");
        let plan = plan_writing(
            &rope,
            Selection::point(2),
            WritingCommand::Paragraph(ParagraphKind::Heading(2)),
            false,
        )
        .expect("应能切换为二级标题");
        assert_eq!(apply_plan("普通段落", &plan), "## 普通段落");
        assert_eq!(plan.selection, Selection::point(5));

        let fenced_doc = "```rust\nlet x = 1;\n```";
        let f_rope = Rope::from_str(fenced_doc);
        let f_parsed = markdown::parse(fenced_doc);
        let fence = SourceBridge::new(&f_rope, &f_parsed, 1, Some(1))
            .fenced_at(10)
            .expect("应识别围栏");
        let leave = plan_leave_fenced(&f_rope, &fence, false).expect("已闭合围栏应支持 ⌘↩ 退出");
        assert_eq!(
            apply_plan(fenced_doc, &leave),
            "```rust\nlet x = 1;\n```\n\n"
        );

        let append = plan_append_paragraph(&f_rope, Some(false), false, false)
            .expect("文末非正文块按 ↓ 应追加段落");
        assert_eq!(
            apply_plan(fenced_doc, &append),
            "```rust\nlet x = 1;\n```\n\n"
        );

        let below = plan_open_paragraph_below_body(&rope, Selection::point(4))
            .expect("点击正文下方应插入双换行");
        assert_eq!(below, (4, "\n\n".to_owned()));

        let md = "| A | B |\n| --- | --- |\n| 1 | 2 |\n";
        let table_rope = Rope::from_str(md);
        let parsed = markdown::parse(md);
        let leaf = SourceBridge::new(&table_rope, &parsed, 1, Some(1))
            .leaf_at(2)
            .unwrap();
        let added = plan_table_tool(&table_rope, &leaf, 2, "row").expect("应能向表格下方插入一行");
        let updated = apply_plan(md, &added);
        assert_eq!(updated.lines().count(), 4);
    }

    #[test]
    fn backspace_on_blank_line_between_paragraphs_never_fuses_blocks() {
        // 两段之间有 1 个额外空行（共 2 个空行）：在空行上按退格应删掉多余空行并把光标退到上一段末尾。
        let text = "适用读者\n\n\n**摘要**\n";
        let rope = Rope::from_str(text);
        let parsed = markdown::parse(text);
        let blank2 = rope.line_to_char(2);
        let leaf = SourceBridge::new(&rope, &parsed, 1, Some(1)).leaf_at(rope.char_to_byte(blank2));
        let action = plan_backspace(&rope, Some(&parsed), leaf.as_ref(), blank2, 1, Some(1))
            .expect("空行退格应由内核接管");
        assert_eq!(
            action,
            BackspaceAction::Replace {
                range: rope.line_to_char(1)..rope.line_to_char(2),
                insert: String::new(),
                caret: Some(line_end(&rope, 0)),
            }
        );

        // 两段之间仅剩 1 个分隔空行时：退格只把光标退回上一段末尾，不能把两段粘成一行。
        let sep_text = "适用读者\n\n**摘要**\n";
        let sep_rope = Rope::from_str(sep_text);
        let sep_parsed = markdown::parse(sep_text);
        let sep_line = sep_rope.line_to_char(1);
        let sep_leaf = SourceBridge::new(&sep_rope, &sep_parsed, 1, Some(1))
            .leaf_at(sep_rope.char_to_byte(sep_line));
        assert_eq!(
            plan_backspace(
                &sep_rope,
                Some(&sep_parsed),
                sep_leaf.as_ref(),
                sep_line,
                1,
                Some(1),
            ),
            Some(BackspaceAction::MoveCaret(line_end(&sep_rope, 0)))
        );
    }
}
