//! 标题的锚点和脚注的悬停提示文字。

use std::collections::HashMap;

use super::*;

impl Markdown {
    /// 返回各顶层块作为标题时的锚点，同名的第二个起加上 `-1`、`-2`；不是标题时为 `None`。
    pub fn heading_keys(&self) -> Vec<Option<String>> {
        let mut seen: HashMap<String, usize> = HashMap::new();
        (self.blocks.iter())
            .map(|block| {
                let Block::Heading(_, inline) = block else {
                    return None;
                };
                let slug = slug(&inline.text);
                let n = seen.entry(slug.clone()).or_default();
                let key = if *n == 0 { slug } else { format!("{slug}-{n}") };
                *n += 1;
                Some(key)
            })
            .collect()
    }

    /// 返回有文字的顶层标题：级别、文字和锚点（见 [`Markdown::heading_keys`]）。
    pub fn headings(&self) -> Vec<(u8, String, String)> {
        (self.blocks.iter().zip(self.heading_keys()))
            .filter_map(|(block, key)| match block {
                Block::Heading(level, inline) if !inline.text.trim().is_empty() => {
                    Some((*level, inline.text.trim().to_owned(), key?))
                }
                _ => None,
            })
            .collect()
    }

    /// 编号为 `number` 的脚注的文字（去掉末尾跳回正文的 ↩）；没有这条脚注时为 `None`。
    pub fn footnote_text(&self, number: &str) -> Option<String> {
        let blocks = self.blocks.iter().find_map(|block| match block {
            Block::Footnote(n, blocks) if n == number => Some(blocks),
            _ => None,
        })?;
        let mut text = String::new();
        plain_text(blocks, &mut text);
        Some(text.trim_end().trim_end_matches('↩').trim_end().to_owned())
    }
}

/// 把 `blocks` 中的文字依次接到 `out` 后面，块之间隔一个空格。
fn plain_text(blocks: &[Block], out: &mut String) {
    fn push(out: &mut String, text: &str) {
        if !out.is_empty() && !text.is_empty() {
            out.push(' ');
        }
        out.push_str(text.trim());
    }
    for block in blocks {
        match block {
            Block::Paragraph(inline) | Block::Heading(_, inline) => push(out, &inline.text),
            Block::Code { text, .. } | Block::Math(text, _) => push(out, text),
            Block::Quote(_, blocks) | Block::Footnote(_, blocks) => plain_text(blocks, out),
            Block::Details {
                summary, blocks, ..
            } => {
                push(out, &summary.text);
                plain_text(blocks, out);
            }
            Block::List(_, items) => {
                for item in items {
                    plain_text(&item.blocks, out);
                }
            }
            Block::Table(_, rows, _) => {
                for cell in rows.iter().flatten() {
                    push(out, &cell.text);
                }
            }
            _ => {}
        }
    }
}
