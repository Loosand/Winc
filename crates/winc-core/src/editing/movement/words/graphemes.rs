//! 在 rope 片段之间读取 Unicode 字素边界。这里的位置都用 UTF-8 字节偏移。

use ropey::RopeSlice;
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};

/// 在 rope 上逐字素移动。顺序读取时复用当前片段，跨片段时按 Unicode 的要求补充上下文。
struct LineGraphemes<'a> {
    line: RopeSlice<'a>,
    cursor: GraphemeCursor,
    chunk: &'a str,
    start: usize,
}

impl<'a> LineGraphemes<'a> {
    fn new(line: RopeSlice<'a>, at: usize, backwards: bool) -> Self {
        let (chunk, start, ..) = line.chunk_at_byte(at.saturating_sub(usize::from(backwards)));
        let cursor = GraphemeCursor::new(at, line.len_bytes(), true);
        Self {
            line,
            cursor,
            chunk,
            start,
        }
    }

    fn query<T>(
        &mut self,
        query: impl Fn(&mut GraphemeCursor, &str, usize) -> Result<T, GraphemeIncomplete>,
    ) -> T {
        loop {
            match query(&mut self.cursor, self.chunk, self.start) {
                Ok(value) => return value,
                Err(GraphemeIncomplete::PrevChunk) => {
                    (self.chunk, self.start, ..) = self.line.chunk_at_byte(self.start - 1);
                }
                Err(GraphemeIncomplete::NextChunk) => {
                    (self.chunk, self.start, ..) =
                        self.line.chunk_at_byte(self.start + self.chunk.len());
                }
                Err(GraphemeIncomplete::PreContext(end)) => {
                    let (context, start, ..) = self.line.chunk_at_byte(end - 1);
                    self.cursor.provide_context(&context[..end - start], start);
                }
                Err(GraphemeIncomplete::InvalidOffset) => unreachable!("cursor outside rope chunk"),
            }
        }
    }

    fn char_at(&self, at: usize) -> Option<char> {
        let (chunk, start) = if (self.start..self.start + self.chunk.len()).contains(&at) {
            (self.chunk, self.start)
        } else {
            let (chunk, start, ..) = self.line.chunk_at_byte(at);
            (chunk, start)
        };
        chunk[at - start..].chars().next()
    }
}

/// 返回 `at` 处或之前最近的字素边界。位置以 UTF-8 字节计。
pub(super) fn start(line: RopeSlice<'_>, at: usize) -> usize {
    let mut units = LineGraphemes::new(line, at, false);
    if units.query(GraphemeCursor::is_boundary) {
        at
    } else {
        units.query(GraphemeCursor::prev_boundary).unwrap_or(0)
    }
}

/// 从 `at` 开始按指定方向迭代，返回移动后的字节位置和跨过的字素的首字符。
pub(super) fn iter(
    line: RopeSlice<'_>,
    at: usize,
    backwards: bool,
) -> impl Iterator<Item = (usize, char)> + '_ {
    let mut units = LineGraphemes::new(line, at, backwards);
    std::iter::from_fn(move || {
        let at = units.cursor.cur_cursor();
        let (start, next) = if backwards {
            let prev = units.query(GraphemeCursor::prev_boundary)?;
            (prev, prev)
        } else {
            (at, units.query(GraphemeCursor::next_boundary)?)
        };
        Some((next, units.char_at(start)?))
    })
}
