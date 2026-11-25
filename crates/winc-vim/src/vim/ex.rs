//! 命令行（`:`）命令的执行：范围的求值，删除、复制、移动、排序，以及 `:g`、`:v` 与 `:normal`。
//!
//! 一条命令行的全部修改属于一个撤销单元。替换见 `substitute`，`:set` 见 `options`。

use std::cmp::Ordering;

use winc_core::{Change, Lines};

use super::macros::text_keys;
use super::{Failed, Shared, Span, Vim};
use crate::command::{Action, Mode};
use crate::ex::{self, Address, Base, Cmd, Line};
use crate::host::{AppCommand, Host, Listing, Message};
use crate::key::{Key, Named};
use crate::operator::{self, Range, Region};
use crate::parser::{Body, Command};
use crate::pattern;
use crate::register::Source;
use crate::text;

/// 返回寄存器名为 `register` 的命令，供复用按键命令的实现。
pub(super) fn with_register(register: Option<char>) -> Command {
    Command {
        register,
        count: None,
        body: Body::Action {
            action: Action::CommandLine,
            arg: None,
        },
    }
}

/// 返回覆盖第 `first` 至 `last` 行的按行范围。
pub(super) fn line_range<H: Host>(host: &H, first: usize, last: usize) -> Range {
    Range {
        region: Region::Lines { first, last },
        first,
        last,
        start: text::first_non_blank(host.text(), first),
        numbered: true,
        empty: false,
    }
}

/// 解析参数开头的寄存器名与行数：`[x] [count]`；写法有误时返回 `None`。
fn register_count(args: &str) -> Option<(Option<char>, Option<usize>)> {
    let args = args.trim();
    let mut chars = args.chars();
    let (register, rest) = match chars.next() {
        Some(c) if !c.is_ascii_digit() => (Some(c), chars.as_str().trim_start()),
        _ => (None, args),
    };
    if register.is_some_and(|c| !(c.is_ascii_alphanumeric() || "\"-+*_".contains(c))) {
        return None;
    }
    match ex::number(rest) {
        Some((count, "")) if count > 0 => Some((register, Some(count))),
        None if rest.is_empty() => Some((register, None)),
        _ => None,
    }
}

impl Vim {
    /// 执行命令行 `text`：全部修改属于一个撤销单元，行数变化较大时提示。
    pub(super) fn ex_line(&mut self, shared: &mut Shared, host: &mut impl Host, text: &str) {
        if let Some(visual) = self.visual {
            self.end_visual(host);
            self.settle(host, visual.cursor);
        }
        let before = text::line_count(host.text());
        // 撤销与重做不能位于分组之内。
        let grouped = !matches!(
            ex::parse(text).map(|line| line.cmd),
            Ok(Cmd::Undo | Cmd::Redo)
        );
        if grouped {
            host.begin_group();
        }
        self.ex_run(shared, host, text);
        self.check_context(host);
        // 逐处确认的替换与未结束的插入各自持有分组，结束时一并关闭。
        if grouped {
            host.end_group();
        }
        let after = text::line_count(host.text());
        let delta = isize::try_from(after).unwrap_or(0) - isize::try_from(before).unwrap_or(0);
        if delta.abs() > 2 && !self.failed {
            host.message(Message::LineDelta(delta));
        }
    }

    /// 依次执行 `text` 中以 `|` 分隔的命令；某条命令失败时停止并返回假。
    pub(super) fn ex_run(&mut self, shared: &mut Shared, host: &mut impl Host, text: &str) -> bool {
        let mut rest = text.to_owned();
        loop {
            let next = {
                let line = match ex::parse(&rest) {
                    Ok(line) => line,
                    Err(message) => {
                        self.ex_last = true;
                        self.error(host, message);
                        return false;
                    }
                };
                let done = if line.cmd.takes_bar() {
                    self.ex_last = true;
                    self.ex_command(shared, host, &line, line.args)
                } else {
                    let (args, next) = ex::split_bar(line.args);
                    self.ex_last = next.is_none();
                    self.ex_command(shared, host, &line, &args)
                        .map(|_| next.map(str::to_owned))
                };
                match done {
                    Ok(next) => next,
                    Err(Failed) => return false,
                }
            };
            match next {
                Some(next) => rest = next,
                None => return true,
            }
        }
    }

    /// 求值范围中的地址；没有写范围时返回 `None`。
    pub(super) fn ex_span(
        &mut self,
        shared: &mut Shared,
        host: &mut impl Host,
        addresses: &[Address],
    ) -> Result<Option<Span>, Failed> {
        let mut current = text::line_of(host.text(), self.caret(host)) + 1;
        let mut values = Vec::with_capacity(addresses.len());
        for address in addresses {
            let last = text::last_line(host.text()) + 1;
            let base = match &address.base {
                Base::Current => current,
                Base::Last => last,
                Base::Number(number) => *number,
                Base::Mark(name) => {
                    let found = match name {
                        '\'' | '`' => Some(host.mark('\'').unwrap_or(0)),
                        name => host.mark(*name),
                    };
                    match found {
                        Some(pos) => text::line_of(host.text(), pos) + 1,
                        None => return self.fail(host, Message::NoMark(*name)),
                    }
                }
                Base::Forward(pattern) | Base::Backward(pattern) => {
                    let forward = matches!(address.base, Base::Forward(_));
                    let from = current.clamp(1, last) - 1;
                    match self.search_line(shared, host, pattern, from, forward) {
                        Some(line) => line + 1,
                        None => return Err(Failed),
                    }
                }
            };
            let value = isize::try_from(base)
                .unwrap_or(isize::MAX)
                .saturating_add(address.offset);
            let Ok(value) = usize::try_from(value) else {
                return self.fail(host, Message::BadRange);
            };
            if value > last {
                return self.fail(host, Message::BadRange);
            }
            // `;` 之后的地址从该行算起，光标同时移到该行。
            if address.moves {
                current = value.max(1);
                self.cursor_to_line(host, current - 1);
            }
            values.push(value);
        }
        Ok(match values.as_slice() {
            [] => None,
            [only] => Some(Span {
                first: *only,
                last: *only,
                given: 1,
            }),
            // 首行在末行之后的范围不执行。
            [.., first, last] if first > last => return self.fail(host, Message::BadRange),
            [.., first, last] => Some(Span {
                first: *first,
                last: *last,
                given: 2,
            }),
        })
    }

    /// 把光标移到第 `line` 行：列按字节数保持不变，落在字符中间时取该字符，超出行尾时退到
    /// 最后一个字符。
    fn cursor_to_line(&mut self, host: &mut impl Host, line: usize) {
        let text = host.text();
        let (from, col) = text.pos_to_line_col(self.caret(host));
        let bytes: usize = (text.line_content(from).slice().chars())
            .take(col)
            .map(char::len_utf8)
            .sum();
        let mut seen = 0;
        let target = (text.line_content(line).slice().chars())
            .take_while(|c| {
                seen += c.len_utf8();
                seen <= bytes
            })
            .count();
        let pos = (text.line_start(line) + target).min(text::last_char(text, line));
        let pos = winc_core::movement::snap_to_grapheme(text, pos);
        self.settle(host, pos);
    }

    /// 返回当前行构成的范围。
    fn current_span(&self, host: &impl Host) -> Span {
        let line = text::line_of(host.text(), self.caret(host)) + 1;
        Span {
            first: line,
            last: line,
            given: 0,
        }
    }

    /// 执行一条命令；`args` 是参数。成功时返回参数之后余下的下一条命令。
    fn ex_command(
        &mut self,
        shared: &mut Shared,
        host: &mut impl Host,
        line: &Line<'_>,
        args: &str,
    ) -> Result<Option<String>, Failed> {
        // 切换文档的命令之前的数是次数或序号，不是行号。
        let counted = matches!(
            line.cmd,
            Cmd::BufferNext | Cmd::BufferPrevious | Cmd::Buffer | Cmd::TabNext | Cmd::TabPrevious
        );
        let given = match line.addresses.as_slice() {
            [address] if counted => match (&address.base, address.offset) {
                (Base::Number(number), 0) => Some(Span {
                    first: *number,
                    last: *number,
                    given: 1,
                }),
                _ => return self.fail(host, Message::BadRange),
            },
            addresses => self.ex_span(shared, host, addresses)?,
        };
        let span = given.unwrap_or_else(|| self.current_span(host));
        let modifies = matches!(
            line.cmd,
            Cmd::Delete
                | Cmd::Put
                | Cmd::Move
                | Cmd::Copy
                | Cmd::Join
                | Cmd::ShiftRight(_)
                | Cmd::ShiftLeft(_)
                | Cmd::Sort
                | Cmd::Substitute
                | Cmd::RepeatSubstitute
        );
        if modifies && self.blocked(host) {
            return Err(Failed);
        }
        match line.cmd {
            Cmd::Goto => {
                if let Some(span) = given {
                    let from = self.caret(host);
                    self.set_context(host, from);
                    let text = host.text();
                    let pos = text::first_non_blank(text, span.lines().1);
                    self.settle(host, pos);
                }
            }
            Cmd::Delete | Cmd::Yank => self.ex_delete(shared, host, line.cmd, span, args)?,
            Cmd::Put => self.ex_put(shared, host, span, line.bang, args)?,
            Cmd::Move | Cmd::Copy => self.ex_move(shared, host, line.cmd, span, args)?,
            Cmd::Join => self.ex_join(host, span, line.bang, args)?,
            Cmd::ShiftRight(amount) | Cmd::ShiftLeft(amount) => {
                let left = matches!(line.cmd, Cmd::ShiftLeft(_));
                self.ex_shift(host, span, amount, left, args)?;
            }
            Cmd::Normal => self.ex_normal(shared, host, given, !line.bang, args)?,
            Cmd::Sort => self.ex_sort(shared, host, given, line.bang, args)?,
            Cmd::Mark => {
                let mut chars = args.trim().chars();
                let (Some(name), None) = (chars.next(), chars.next()) else {
                    return self.fail(host, Message::BadArgument);
                };
                let text = host.text();
                let pos = text::first_non_blank(text, span.lines().1);
                host.set_mark(if name == '`' { '\'' } else { name }, pos);
            }
            Cmd::Set => self.ex_set(shared, host, args)?,
            Cmd::Substitute | Cmd::RepeatSubstitute => {
                let repeat = line.cmd == Cmd::RepeatSubstitute;
                return self.ex_substitute(shared, host, span, repeat, args);
            }
            Cmd::Global | Cmd::GlobalInverse => {
                let inverse = line.bang != (line.cmd == Cmd::GlobalInverse);
                self.ex_global(shared, host, given, inverse, args)?;
            }
            Cmd::Print => {
                let pos = text::first_non_blank(host.text(), span.lines().1);
                self.settle(host, pos);
            }
            Cmd::Undo | Cmd::Redo => {
                let action = if line.cmd == Cmd::Undo {
                    Action::Undo
                } else {
                    Action::Redo
                };
                self.act(shared, host, with_register(None), action, None);
            }
            _ => self.ex_app(host, line, given, args)?,
        }
        Ok(None)
    }

    /// `:d`、`:y`：删除或复制范围中的行。
    fn ex_delete(
        &mut self,
        shared: &mut Shared,
        host: &mut impl Host,
        cmd: Cmd,
        span: Span,
        args: &str,
    ) -> Result<(), Failed> {
        let Some((register, count)) = register_count(args) else {
            return self.fail(host, Message::BadArgument);
        };
        let (first, last) = counted(host, span, count);
        let range = line_range(host, first, last);
        let command = with_register(register);
        let done = if cmd == Cmd::Delete {
            self.delete(shared, host, command, range)
        } else {
            let cursor = self.caret(host);
            let done = self.yank(shared, host, command, range);
            // 复制不移动光标。
            self.settle(host, cursor);
            done
        };
        if done { Ok(()) } else { Err(Failed) }
    }

    /// `:pu`：把寄存器的内容按行放到范围末行之后；`above` 为真时放到其之前。
    fn ex_put(
        &mut self,
        shared: &mut Shared,
        host: &mut impl Host,
        span: Span,
        above: bool,
        args: &str,
    ) -> Result<(), Failed> {
        let mut chars = args.trim().chars();
        let register = chars.next();
        if chars.next().is_some() {
            return self.fail(host, Message::BadArgument);
        }
        // 光标先移到范围的末行，列不变。
        self.cursor_to_line(host, span.lines().1);
        let Some(content) = Self::read_register(shared, host, register) else {
            return self.fail(host, Message::EmptyRegister(register.unwrap_or('"')));
        };
        let lines: Vec<&str> = content.lines().collect();
        // 新的行位于第 `after` 行之后（行号从 1 起）。
        let after = if above {
            span.last.saturating_sub(1)
        } else {
            span.last
        };
        self.insert_lines(host, after, &lines)
    }

    /// 在第 `after` 行（从 1 起，0 表示文本的开头）之后插入 `lines`，光标放在最后一个新行上。
    fn insert_lines(
        &mut self,
        host: &mut impl Host,
        after: usize,
        lines: &[&str],
    ) -> Result<(), Failed> {
        let ending = host.line_ending().to_owned();
        let block = lines.join(&ending);
        let text = host.text();
        let cursor = text::first_non_blank(text, after.min(text::last_line(text)));
        let (start, inserted) = if after == 0 {
            (0, format!("{block}{ending}"))
        } else {
            (text.line_end(after - 1), format!("{ending}{block}"))
        };
        let change = Change {
            start,
            removed: String::new(),
            inserted,
        };
        if !self.edit(host, vec![change], cursor, after) {
            return Err(Failed);
        }
        let text = host.text();
        let last = (after + lines.len().max(1) - 1).min(text::last_line(text));
        let (from, to) = (text.line_start(after.min(last)), text.line_start(last));
        host.set_mark('[', from);
        host.set_mark(']', to);
        let pos = text::first_non_blank(host.text(), last);
        self.settle(host, pos);
        Ok(())
    }

    /// `:m`、`:t`：把范围中的行移动或复制到参数给出的行之后。
    fn ex_move(
        &mut self,
        shared: &mut Shared,
        host: &mut impl Host,
        cmd: Cmd,
        span: Span,
        args: &str,
    ) -> Result<(), Failed> {
        let target = match ex::parse(args) {
            Ok(line) if line.cmd == Cmd::Goto && line.args.is_empty() => {
                self.ex_span(shared, host, &line.addresses)?
            }
            _ => None,
        };
        let Some(target) = target else {
            return self.fail(host, Message::BadArgument);
        };
        let dest = target.last;
        let (first, last) = span.lines();
        let count = last - first + 1;
        let text = host.text();
        let block = operator::lines_text(text, first, last);
        let lines: Vec<&str> = block.split('\n').collect();
        if cmd == Cmd::Copy {
            return self.insert_lines(host, dest, &lines);
        }
        // 目标在范围之内时无法移动；紧邻范围时文本不变。
        if dest > first && dest <= last {
            return self.fail(host, Message::BadRange);
        }
        if dest == first || dest == last + 1 {
            let pos = text::first_non_blank(text, last);
            self.settle(host, pos);
            return Ok(());
        }
        let (from, to) = operator::lines_span(text, first, last);
        if !host.fits(from, to) {
            return self.fail(host, Message::TooLarge);
        }
        let ending = host.line_ending().to_owned();
        let moved = block.replace('\n', &ending);
        let removal = operator::removal(text, from, to);
        let insertion = |start: usize, inserted: String| Change {
            start,
            removed: String::new(),
            inserted,
        };
        // 先执行位置靠后的一处，另一处的位置因此不受影响。
        let (changes, landed) = if dest > last {
            let start = text.line_end(dest - 1);
            let insert = insertion(start, format!("{ending}{moved}"));
            (vec![insert, removal], dest - 1)
        } else if dest == 0 {
            (
                vec![removal, insertion(0, format!("{moved}{ending}"))],
                count - 1,
            )
        } else {
            let start = text.line_end(dest - 1);
            let insert = insertion(start, format!("{ending}{moved}"));
            (vec![removal, insert], dest + count - 1)
        };
        let cursor = self.caret(host);
        if !self.edit(host, changes, cursor, first.min(dest)) {
            return Err(Failed);
        }
        let text = host.text();
        let pos = text::first_non_blank(text, landed.min(text::last_line(text)));
        self.settle(host, pos);
        Ok(())
    }

    /// `:j`：合并范围中的行；`raw` 为真时原样连接。
    fn ex_join(
        &mut self,
        host: &mut impl Host,
        span: Span,
        raw: bool,
        args: &str,
    ) -> Result<(), Failed> {
        let count = match ex::number(args.trim()) {
            Some((count, "")) if count > 0 => Some(count),
            None if args.trim().is_empty() => None,
            _ => return self.fail(host, Message::BadArgument),
        };
        let (first, last) = span.lines();
        self.cursor_to_line(host, first);
        let (first, lines) = match count {
            Some(count) => (last, count),
            // 两个地址相同时不合并；只有一行时与下一行合并。
            None if first == last && span.given >= 2 => return Ok(()),
            None if first == last => (first, 2),
            None => (first, last - first + 1),
        };
        let text = host.text();
        let Some((changes, _)) = operator::join(text, first, lines, !raw) else {
            self.failed = true;
            return Err(Failed);
        };
        let cursor = self.caret(host);
        if !self.edit(host, changes, cursor, first) {
            return Err(Failed);
        }
        let pos = text::first_non_blank(host.text(), first);
        self.settle(host, pos);
        Ok(())
    }

    /// `:>`、`:<`：增减范围中各行的缩进 `amount` 级。
    fn ex_shift(
        &mut self,
        host: &mut impl Host,
        span: Span,
        amount: usize,
        left: bool,
        args: &str,
    ) -> Result<(), Failed> {
        let count = match ex::number(args.trim()) {
            Some((count, "")) if count > 0 => Some(count),
            None if args.trim().is_empty() => None,
            _ => return self.fail(host, Message::BadArgument),
        };
        let (first, last) = counted(host, span, count);
        let lines = last - first + 1;
        let by = host.indentation().width.max(1).saturating_mul(amount);
        if !left && lines.saturating_mul(by) > super::MAX_OUTPUT {
            return self.fail(host, Message::TooLarge);
        }
        let changes = operator::shift(host, first, last, amount, left);
        let cursor = self.caret(host);
        if !self.edit(host, changes, cursor, first) {
            return Err(Failed);
        }
        let pos = text::first_non_blank(host.text(), last);
        self.settle(host, pos);
        Ok(())
    }

    /// 文件、标签与窗口的命令：交给宿主执行。
    fn ex_app(
        &mut self,
        host: &mut impl Host,
        line: &Line<'_>,
        given: Option<Span>,
        args: &str,
    ) -> Result<(), Failed> {
        let args = args.trim();
        let path = (!args.is_empty()).then(|| args.to_owned());
        let force = line.bang;
        // 范围或参数给出的数。
        let amount = given
            .map(|span| span.last)
            .or_else(|| ex::number(args).map(|(amount, _)| amount));
        let step = isize::try_from(amount.unwrap_or(1)).unwrap_or(isize::MAX);
        let quit = |write| AppCommand::Quit { write, force };
        let command = match line.cmd {
            Cmd::Write => AppCommand::Write { path, force },
            Cmd::WriteAll => AppCommand::WriteAll,
            Cmd::Quit | Cmd::BufferDelete | Cmd::TabClose => quit(false),
            Cmd::WriteQuit => quit(true),
            Cmd::QuitAll => AppCommand::QuitAll {
                write: false,
                force,
            },
            Cmd::WriteQuitAll => AppCommand::QuitAll { write: true, force },
            Cmd::Edit => AppCommand::Edit { path, force },
            Cmd::BufferNext => AppCommand::BufferStep(step),
            Cmd::BufferPrevious => AppCommand::BufferStep(-step),
            Cmd::Buffer => match (amount, path) {
                (Some(number), _) => AppCommand::BufferNumber(number),
                (None, Some(name)) => AppCommand::BufferName(name),
                (None, None) => return Ok(()),
            },
            Cmd::TabNext => match amount {
                Some(number) => AppCommand::BufferNumber(number),
                None => AppCommand::BufferStep(1),
            },
            Cmd::TabPrevious => AppCommand::BufferStep(-step),
            Cmd::TabOnly => AppCommand::Only,
            Cmd::TabNew => AppCommand::New { path },
            Cmd::NoHighlight => AppCommand::NoHighlight,
            Cmd::Registers => AppCommand::List(Listing::Registers),
            Cmd::Marks => AppCommand::List(Listing::Marks),
            Cmd::Buffers => AppCommand::List(Listing::Buffers),
            Cmd::Help => AppCommand::Help,
            _ => return self.fail(host, Message::BadCommand),
        };
        if host.app(command) {
            Ok(())
        } else {
            self.fail(host, Message::Unsupported)
        }
    }

    /// `:normal`：回放参数中的按键；有范围时在每一行的行首各执行一次。`remap` 为假时按键
    /// 不经过用户的映射。
    fn ex_normal(
        &mut self,
        shared: &mut Shared,
        host: &mut impl Host,
        given: Option<Span>,
        remap: bool,
        args: &str,
    ) -> Result<(), Failed> {
        let keys = text_keys(args);
        if keys.is_empty() {
            return self.fail(host, Message::BadArgument);
        }
        let Some(span) = given else {
            self.normal_once(shared, host, &keys, remap);
            return Ok(());
        };
        let (first, last) = span.lines();
        for line in first..=last {
            if !self.spend(host) {
                break;
            }
            // 行号超出文本时在最后一行上执行。
            let text = host.text();
            let pos = text.line_start(line.min(text::last_line(text)));
            self.settle(host, pos);
            self.failed = false;
            self.normal_once(shared, host, &keys, remap);
        }
        self.failed = false;
        Ok(())
    }

    /// 回放 `keys`，结束时未完成的命令与插入按 `Esc` 结束。
    fn normal_once(
        &mut self,
        shared: &mut Shared,
        host: &mut impl Host,
        keys: &[Key],
        remap: bool,
    ) {
        self.play_now(shared, host, keys, remap);
        if self.prompt().is_some() {
            self.line.clear();
            self.prompt_cancel(host);
        }
        if self.confirm.is_some() {
            self.confirm_end(host);
        }
        self.parser.reset();
        if self.mode == Mode::Insert || self.oneshot.is_some() {
            self.key_raw(shared, host, Key::named(Named::Esc));
        }
    }

    /// `:g`、`:v`：在范围中有匹配（`inverse` 为真时没有匹配）的每一行上执行参数中的命令。
    fn ex_global(
        &mut self,
        shared: &mut Shared,
        host: &mut impl Host,
        given: Option<Span>,
        inverse: bool,
        args: &str,
    ) -> Result<(), Failed> {
        let mut chars = args.chars();
        let separator = match chars.next() {
            Some(c) if !c.is_alphanumeric() && !matches!(c, '\\' | '"' | '|') => c,
            _ => return self.fail(host, Message::BadArgument),
        };
        // 逐行执行的命令中不能再逐行执行。
        if self.in_global {
            return self.fail(host, Message::BadCommand);
        }
        let (pattern, command) = ex::delimited(chars.as_str(), separator);
        let pattern = if pattern.is_empty() {
            shared.search_pattern().map(str::to_owned)
        } else {
            Some(pattern)
        };
        let Some(pattern) = pattern else {
            return self.fail(host, Message::NoSearch);
        };
        let query = match pattern::translate(&pattern, shared.ignore_case, shared.smart_case) {
            Ok(query) => query,
            Err(at) => return self.fail(host, Message::BadPattern(at)),
        };
        shared.set_search_pattern(pattern);

        let text = host.text();
        let (first, last) = match given {
            Some(span) => span.lines(),
            None => (0, text::last_line(text)),
        };
        // 有匹配的行，从小到大。
        let mut matched = Vec::new();
        let mut pos = host.text().line_start(first);
        while let Some((start, _)) = host.find(&query, pos, true) {
            let text = host.text();
            let line = text::line_of(text, start);
            if line > last {
                break;
            }
            matched.push(line);
            if line >= text::last_line(text) {
                break;
            }
            pos = text.line_start(line + 1);
        }
        let lines: Vec<usize> = if inverse {
            let mut skip = matched.iter().copied().peekable();
            (first..=last)
                .filter(|line| skip.next_if_eq(line).is_none())
                .collect()
        } else {
            matched
        };
        if lines.is_empty() {
            return self.fail(host, Message::NotFound);
        }
        let command = command.trim();
        // 删除整行是最常见的用法，一次应用。
        let plain_delete = ex::parse(command).is_ok_and(|line| {
            line.cmd == Cmd::Delete && line.addresses.is_empty() && line.args.is_empty()
        });
        if plain_delete {
            return self.delete_lines(shared, host, &lines);
        }
        let text = host.text();
        host.track(lines.iter().map(|line| text.line_start(*line)).collect());
        self.in_global = true;
        while let Some(pos) = host.next_tracked() {
            if !self.spend(host) {
                break;
            }
            self.settle(host, pos.min(host.text().end_pos()));
            self.failed = false;
            // 没有写命令时只移动光标。
            if !command.is_empty() {
                self.ex_run(shared, host, command);
            }
        }
        host.track(Vec::new());
        self.in_global = false;
        self.failed = false;
        Ok(())
    }

    /// 删除 `lines`（从小到大）中的各行：寄存器的结果与逐行删除相同。
    fn delete_lines(
        &mut self,
        shared: &mut Shared,
        host: &mut impl Host,
        lines: &[usize],
    ) -> Result<(), Failed> {
        if self.blocked(host) {
            return Err(Failed);
        }
        // 编号寄存器只保留最后九次删除。
        for line in &lines[lines.len().saturating_sub(9)..] {
            let content = super::operate::content_of(host, line_range(host, *line, *line).region);
            let source = Source::Delete { numbered: true };
            if !Self::write_register(shared, host, None, content, source) {
                return Err(Failed);
            }
        }
        // 相邻的行合并成一处修改，从后往前排列。
        let text = host.text();
        let mut changes = Vec::new();
        let mut end = lines.len();
        while end > 0 {
            let mut start = end - 1;
            while start > 0 && lines[start - 1] + 1 == lines[start] {
                start -= 1;
            }
            let (from, to) = operator::lines_span(text, lines[start], lines[end - 1]);
            changes.push(operator::removal(text, from, to));
            end = start;
        }
        let (Some(first), Some(last)) = (lines.first(), lines.last()) else {
            return Ok(());
        };
        let cursor = text::first_non_blank(text, *first);
        if !self.edit(host, changes, cursor, *first) {
            return Err(Failed);
        }
        // 光标在最后一处删除的位置。
        let text = host.text();
        let line = (last + 1 - lines.len()).min(text::last_line(text));
        let pos = text::first_non_blank(text, line);
        self.settle(host, pos);
        Ok(())
    }

    /// `:sort`：对范围中的行排序，没有范围时为全部的行；`reverse` 为真时从大到小。
    fn ex_sort(
        &mut self,
        shared: &mut Shared,
        host: &mut impl Host,
        given: Option<Span>,
        reverse: bool,
        args: &str,
    ) -> Result<(), Failed> {
        let mut order = SortOrder {
            reverse,
            ..SortOrder::default()
        };
        let mut query = None;
        let mut rest = args.trim();
        while let Some(c) = rest.chars().next() {
            let after = &rest[c.len_utf8()..];
            match c {
                'i' => order.ignore_case = true,
                'n' => order.numeric = true,
                'x' => order.hex = true,
                'u' => order.unique = true,
                'r' => order.matched = true,
                ' ' | '\t' => {}
                c if !c.is_alphanumeric() && c != '"' && c != '|' => {
                    let (source, after) = ex::delimited(after, c);
                    let source = if source.is_empty() {
                        shared.search_pattern().map(str::to_owned)
                    } else {
                        Some(source)
                    };
                    let Some(source) = source else {
                        return self.fail(host, Message::NoSearch);
                    };
                    match pattern::translate(&source, shared.ignore_case, shared.smart_case) {
                        Ok(translated) => query = Some(translated),
                        Err(at) => return self.fail(host, Message::BadPattern(at)),
                    }
                    rest = after;
                    continue;
                }
                _ => return self.fail(host, Message::BadArgument),
            }
            rest = after;
        }

        let text = host.text();
        let (first, last) = match given {
            Some(span) => span.lines(),
            None => (0, text::last_line(text)),
        };
        let (from, to) = (text.line_start(first), text.line_end(last));
        if !host.fits(from, to) {
            return self.fail(host, Message::TooLarge);
        }
        // 各行的内容与排序依据。
        let mut rows: Vec<(String, String)> = Vec::with_capacity(last - first + 1);
        for line in first..=last {
            let text = host.text();
            let (start, end) = (text.line_start(line), text.line_end(line));
            let content = operator::normalized(text.substring(start, end));
            let found = (query.as_ref())
                .and_then(|query| host.find_groups(query, start))
                .and_then(|groups| groups.first().copied().flatten())
                .filter(|(_, to)| *to <= end);
            // 给出了模式而没有匹配的行，排序依据为空。
            let key = match found {
                Some((a, b)) if order.matched => host.text().substring(a, b),
                Some((_, b)) => host.text().substring(b, end),
                None if query.is_some() => String::new(),
                None => content.clone(),
            };
            rows.push((content, key));
        }
        let before: Vec<String> = rows.iter().map(|(content, _)| content.clone()).collect();
        rows.sort_by(|a, b| sort_compare(&a.1, &b.1, order));
        // 从大到小是把排好的结果倒过来，相同的行次序随之相反。
        if order.reverse {
            rows.reverse();
        }
        if order.unique {
            rows.dedup_by(|b, a| sort_compare(&a.1, &b.1, order) == Ordering::Equal);
        }
        let sorted: Vec<String> = rows.into_iter().map(|(content, _)| content).collect();
        if sorted != before {
            let ending = host.line_ending().to_owned();
            let text = host.text();
            let change = Change {
                start: from,
                removed: text.substring(from, to),
                inserted: sorted.join(&ending),
            };
            let cursor = self.caret(host);
            if !self.edit(host, vec![change], cursor, first) {
                return Err(Failed);
            }
        }
        let text = host.text();
        let pos = text::first_non_blank(text, first.min(text::last_line(text)));
        self.settle(host, pos);
        Ok(())
    }
}

/// `:sort` 的排序方式。
#[derive(Debug, Clone, Copy, Default)]
struct SortOrder {
    /// 是否忽略大小写。
    ignore_case: bool,
    /// 是否按行中的第一个十进制数排序。
    numeric: bool,
    /// 是否按行中的第一个十六进制数排序。
    hex: bool,
    /// 是否只保留相同的行中的第一行。
    unique: bool,
    /// 是否按模式匹配到的文本排序；为假时按匹配之后的文本排序。
    matched: bool,
    /// 是否从大到小。
    reverse: bool,
}

/// 返回文本 `key` 中第一个数的值；`hex` 为真时按十六进制读取。没有数时返回 `None`。
fn number_in(key: &str, hex: bool) -> Option<i128> {
    let bytes = key.as_bytes();
    let digit = |b: u8| {
        if hex {
            b.is_ascii_hexdigit()
        } else {
            b.is_ascii_digit()
        }
    };
    let mut start = bytes.iter().position(|b| digit(*b))?;
    // 十六进制数可以带 `0x` 前缀。
    if hex && bytes[start] == b'0' && matches!(bytes.get(start + 1), Some(b'x' | b'X')) {
        let after = start + 2;
        if bytes.get(after).is_some_and(|b| digit(*b)) {
            start = after;
        }
    }
    let end = (start..bytes.len())
        .find(|at| !digit(bytes[*at]))
        .unwrap_or(bytes.len());
    let radix = if hex { 16 } else { 10 };
    // 位数过多时取上限。
    let value = i128::from_str_radix(&key[start..end], radix).unwrap_or(i128::MAX);
    let negative = start > 0 && bytes[start - 1] == b'-';
    Some(if negative { -value } else { value })
}

/// 比较两行的排序依据。
fn sort_compare(a: &str, b: &str, order: SortOrder) -> Ordering {
    if order.numeric || order.hex {
        // 没有数的行排在最前。
        number_in(a, order.hex).cmp(&number_in(b, order.hex))
    } else if order.ignore_case {
        let fold = |text: &str| {
            text.chars()
                .flat_map(char::to_lowercase)
                .collect::<String>()
        };
        fold(a).cmp(&fold(b))
    } else {
        a.cmp(b)
    }
}

/// 返回范围从 0 起的首末行；给出行数 `count` 时改为从范围的末行起的 `count` 行。
fn counted<H: Host>(host: &H, span: Span, count: Option<usize>) -> (usize, usize) {
    let (first, last) = span.lines();
    match count {
        Some(count) => {
            let end = last.saturating_add(count - 1);
            (last, end.min(text::last_line(host.text())))
        }
        None => (first, last),
    }
}
