//! 宏与按键的回放：`q` 的记录、`@` 的执行，以及回放所用的按键队列与上限。
//!
//! 回放的按键先进入队列，再逐个按键入的方式处理：`@` 在队列的前端插入寄存器的内容，因此在宏的
//! 末尾执行自身不会加深调用层次。命令失败时丢弃队列中属于本次回放的按键。

use std::time::{Duration, Instant};

use super::{Shared, Vim};
use crate::command::Mode;
use crate::host::{Host, Message};
use crate::key::{Code, Key, Mods, Named, notation};
use crate::register::{Content, Kind};

/// 一次按键引发的回放可以处理的按键数上限（个）。
///
/// 取自调参：远大于正常的宏与次数的乘积；每个按键约需数微秒，达到上限之前先受时间限制。
pub(super) const MAX_STEPS: usize = 1_000_000;

/// 一次按键引发的回放与逐行执行的时间上限。
///
/// 取自调参：期间界面不响应，超过两秒即视为失控。
pub(super) const MAX_TIME: Duration = Duration::from_secs(2);

/// 队列中等待回放的按键数上限（个）；次数使按键超过上限时命令不执行。
const MAX_QUEUE: usize = 1_000_000;

/// 回放的预算：从最外层的一次按键开始计。
#[derive(Debug, Clone, Copy)]
pub(super) struct Budget {
    /// 还可以处理的按键数。
    steps: usize,
    /// 开始的时刻。
    started: Instant,
}

/// 进行中的宏记录。
#[derive(Debug, Clone)]
pub(super) struct Recording {
    /// 寄存器名；大写表示追加。
    name: char,
    /// 已记录的按键。
    keys: Vec<Key>,
}

/// 返回按键 `key` 在寄存器中的写法：产生字符的按键是该字符，`Ctrl` 组合是控制字符，其余的
/// 按键写成按键记法。
fn key_text(key: Key, out: &mut String) {
    if let Some(c) = key.as_char() {
        out.push(c);
        return;
    }
    let plain = key.mods().is_empty();
    match key.code() {
        Code::Named(Named::Esc) if plain => out.push('\u{1b}'),
        Code::Named(Named::Enter) if plain => out.push('\r'),
        Code::Named(Named::Tab) if plain => out.push('\t'),
        Code::Char(c) if key.mods() == Mods::CTRL && matches!(c, 'a'..='z' | '\\'..='_') => {
            out.push(char::from(c.to_ascii_uppercase() as u8 ^ 0x40));
        }
        _ => out.push_str(&notation::write(&[key])),
    }
}

/// 返回 `text` 开头的尖括号记法所表示的按键与记法的长度：只识别在寄存器中写成记法的按键
/// 与表示 `<` 的 `<lt>`。
fn noted(text: &str) -> Option<(Key, usize)> {
    let end = text.find('>')?;
    if !text.starts_with('<') {
        return None;
    }
    let [key] = notation::parse(&text[..=end])[..] else {
        return None;
    };
    let mut written = String::new();
    key_text(key, &mut written);
    let special = written.len() > 1 && written.starts_with('<');
    (special || key == Key::char('<')).then_some((key, end + 1))
}

/// 把寄存器中的文本 `text` 转换成按键：换行是回车，尖括号中的功能键名是该功能键。
pub(super) fn text_keys(text: &str) -> Vec<Key> {
    let mut keys = Vec::new();
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if let Some((key, length)) = noted(rest) {
            keys.push(key);
            rest = &rest[length..];
            continue;
        }
        keys.push(match c {
            '\n' => Key::named(Named::Enter),
            c => Key::char(c),
        });
        rest = &rest[c.len_utf8()..];
    }
    keys
}

/// 把记录的按键 `keys` 写成寄存器中的文本；键入的 `<` 之后恰好构成按键记法时写成 `<lt>`。
fn keys_text(keys: &[Key]) -> String {
    let pieces: Vec<String> = (keys.iter())
        .map(|key| {
            let mut piece = String::new();
            key_text(*key, &mut piece);
            piece
        })
        .collect();
    let mut text = String::new();
    for (index, piece) in pieces.iter().enumerate() {
        let ahead: String = pieces[index..].concat();
        if piece == "<" && noted(&ahead).is_some() {
            text.push_str("<lt>");
        } else {
            text.push_str(piece);
        }
    }
    text
}

impl Vim {
    /// 返回正在记录的宏的寄存器名；没有在记录时返回 `None`。
    pub fn recording(&self) -> Option<char> {
        self.recording.as_ref().map(|recording| recording.name)
    }

    /// 记录一次键入的按键。
    pub(super) fn record(&mut self, key: Key) {
        if let Some(recording) = &mut self.recording {
            recording.keys.push(key);
        }
    }

    /// 记录在宿主的输入框中输入的一行：`text` 之后是回车，放弃输入时是 `Esc`。
    pub(super) fn record_line(&mut self, text: Option<&str>) {
        let Some(recording) = &mut self.recording else {
            return;
        };
        match text {
            Some(text) => {
                recording.keys.extend(text.chars().map(Key::char));
                recording.keys.push(Key::named(Named::Enter));
            }
            None => recording.keys.push(Key::named(Named::Esc)),
        }
    }

    /// `q{寄存器}`：开始记录。
    pub(super) fn start_recording(&mut self, name: Option<char>) {
        match name {
            Some(name) if name.is_ascii_alphanumeric() || name == '"' => {
                self.recording = Some(Recording {
                    name,
                    keys: Vec::new(),
                });
            }
            _ => self.failed = true,
        }
    }

    /// 普通与可视模式下、不在多键命令之中时，`q` 结束记录并把按键存入寄存器；返回是否已结束。
    pub(super) fn stop_recording(&mut self, shared: &mut Shared, key: Key) -> bool {
        let idle = self.mode != Mode::Insert
            && self.parser.at_start()
            && !self.parser.is_operator_pending();
        if !idle || key != Key::char('q') {
            return false;
        }
        let Some(mut recording) = self.recording.take() else {
            return false;
        };
        self.parser.reset();
        // 键入的 `q` 已被记录，不属于宏。
        if recording.keys.last() == Some(&key) {
            recording.keys.pop();
        }
        let content = Content {
            kind: Kind::Charwise,
            text: keys_text(&recording.keys),
        };
        shared.registers.set_macro(recording.name, content);
        true
    }

    /// `@{寄存器}`：把寄存器的内容作为按键回放 `count` 次；`@@` 重复上一次，`@:` 重复上一次
    /// 命令行。
    pub(super) fn play_register(
        &mut self,
        shared: &mut Shared,
        host: &mut impl Host,
        count: Option<u32>,
        name: Option<char>,
    ) {
        let times = count.map_or(1, |n| usize::try_from(n).unwrap_or(usize::MAX));
        let name = match name {
            Some('@') => shared.last_macro,
            name => name,
        };
        let Some(name) = name else {
            self.failed = true;
            return;
        };
        if name == ':' {
            let Some(line) = shared.last_command.clone() else {
                return self.error(host, Message::NoCommand);
            };
            shared.last_macro = Some(name);
            for _ in 0..times {
                // 命令行开始逐处确认之后不再重复。
                if !self.spend(host) || self.failed || self.confirm.is_some() {
                    break;
                }
                self.command_line(shared, host, &line);
            }
            return;
        }
        let Some(content) = Self::read_register(shared, host, Some(name)) else {
            return self.error(host, Message::EmptyRegister(name));
        };
        shared.last_macro = Some(name);
        let mut text = content.text;
        if content.kind == Kind::Linewise {
            text.push('\n');
        }
        let keys = text_keys(&text);
        if keys.len().saturating_mul(times) > MAX_QUEUE.saturating_sub(self.typeahead.len()) {
            return self.error(host, Message::TooLarge);
        }
        for _ in 0..times {
            for key in keys.iter().rev() {
                self.typeahead.push_front((*key, true));
            }
        }
    }

    /// 把一次按键引发的回放的上限改为 `steps` 个按键与时间 `time`；默认约一百万个按键、两秒。
    pub fn limit_playback(&mut self, steps: usize, time: Duration) {
        self.limits = (steps, time);
    }

    /// 消耗一步预算；步数或时间用尽时提示、丢弃全部待回放的按键并返回假。
    pub(super) fn spend(&mut self, host: &mut impl Host) -> bool {
        if self.aborted {
            return false;
        }
        let (steps, time) = self.limits;
        let budget = self.budget.get_or_insert_with(|| Budget {
            steps,
            started: Instant::now(),
        });
        budget.steps = budget.steps.saturating_sub(1);
        // 时间每隔若干步核对一次。
        let expired = budget.steps.is_multiple_of(64) && budget.started.elapsed() > time;
        if budget.steps == 0 || expired {
            self.aborted = true;
            self.typeahead.clear();
            self.parser.reset();
            self.error(host, Message::Interrupted);
            return false;
        }
        true
    }
}
