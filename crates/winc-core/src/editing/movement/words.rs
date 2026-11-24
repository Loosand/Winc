//! 按词移动和双击选词。文档位置用字符偏移，行内扫描使用 UTF-8 字节偏移。

use std::iter::Peekable;

use ropey::RopeSlice;

use super::{next_grapheme, prev_grapheme};
use crate::text::Lines;

mod graphemes;

#[cfg(test)]
mod tests;

/// 返回 `pos` 向左移动一个词后的位置：先跳过空白，再跳过同一类字素。
///
/// 在行首时等同于 [`prev_grapheme`]，即跳到上一行行末。
pub fn prev_word(text: &impl Lines, pos: usize) -> usize {
    let (line, col) = text.pos_to_line_col(pos);
    if col == 0 {
        return prev_grapheme(text, pos);
    }

    let content = text.line_content(line);
    let slice = content.slice();
    let at = slice.char_to_byte(col);
    let mut units = classified_graphemes(slice, at, true).peekable();
    let at = skip_class(&mut units, at, WordClass::Space);
    let at = match units.peek().copied() {
        Some((_, class)) => skip_class(&mut units, at, class),
        None => at,
    };
    text.line_start(line) + slice.byte_to_char(at)
}

/// 返回 `pos` 向右移动一个词后的位置：先跳过当前类别，再跳过空白。
///
/// 在行末时等同于 [`next_grapheme`]，即跳到下一行行首。
pub fn next_word(text: &impl Lines, pos: usize) -> usize {
    let (line, col) = text.pos_to_line_col(pos);
    let content = text.line_content(line);
    let slice = content.slice();
    if col >= slice.len_chars() {
        return next_grapheme(text, pos);
    }

    let at = graphemes::start(slice, slice.char_to_byte(col));
    let mut units = classified_graphemes(slice, at, false).peekable();
    let at = match units.peek().copied() {
        Some((_, class)) if class != WordClass::Space => skip_class(&mut units, at, class),
        _ => at,
    };
    let at = skip_class(&mut units, at, WordClass::Space);
    text.line_start(line) + slice.byte_to_char(at)
}

/// 返回双击选词的位置范围 `(起点, 终点)`，终点不含。
///
/// 范围由同类字素组成。`pos` 紧跟在单词之后、且右侧不是单词时，选中左侧的词。
pub fn word_at(text: &impl Lines, pos: usize) -> (usize, usize) {
    let (line, col) = text.pos_to_line_col(pos);
    let content = text.line_content(line);
    let slice = content.slice();
    let base = text.line_start(line);
    if slice.len_chars() == 0 {
        return (base, base);
    }

    let at = graphemes::start(slice, slice.char_to_byte(col));
    let left = classified_graphemes(slice, at, true).next();
    let right = classified_graphemes(slice, at, false).next();
    let (at, class) = match (left, right) {
        (Some(left), None) => left,
        (Some(left @ (_, WordClass::Word)), Some((_, WordClass::Space | WordClass::Punct))) => left,
        (_, Some((_, class))) => (at, class),
        (None, None) => return (base, base),
    };

    let edge = |backwards| {
        let mut units = classified_graphemes(slice, at, backwards).peekable();
        base + slice.byte_to_char(skip_class(&mut units, at, class))
    };
    (edge(true), edge(false))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WordClass {
    Space,
    /// 字母、数字和下划线。
    Word,
    Punct,
}

impl WordClass {
    fn of(first: char) -> Self {
        if first.is_whitespace() {
            Self::Space
        } else if first.is_alphanumeric() || first == '_' {
            Self::Word
        } else {
            Self::Punct
        }
    }
}

fn classified_graphemes(
    line: RopeSlice<'_>,
    at: usize,
    backwards: bool,
) -> impl Iterator<Item = (usize, WordClass)> + '_ {
    graphemes::iter(line, at, backwards).map(|(offset, first)| (offset, WordClass::of(first)))
}

fn skip_class(
    units: &mut Peekable<impl Iterator<Item = (usize, WordClass)>>,
    mut at: usize,
    class: WordClass,
) -> usize {
    while let Some((offset, _)) = units.next_if(|&(_, next)| next == class) {
        at = offset;
    }
    at
}
