//! 提取笔记正文中的链接及其源码范围，不解析文件目标。

use std::ops::Range;

use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};

/// Marks a Wiki target in the preview's string-only link address channel.
/// Markdown input cannot supply a literal NUL to that channel.
pub const WIKI_TARGET: &str = "\0wiki:";

/// Whether the link opens a web page outside the editor.
pub fn is_web(target: &str) -> bool {
    target.split_once(':').is_some_and(|(scheme, _)| {
        scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http")
    })
}

/// 链接目标的解释规则由源码语法决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkSyntax {
    /// 目标是 Markdown 链接地址。
    Markdown,
    /// 目标是 Wiki 笔记名或路径。
    Wiki,
}

/// 笔记正文中的单次链接引用，范围采用源码的 UTF-8 字节偏移。
#[derive(Debug, PartialEq, Eq)]
pub struct NoteLink {
    /// 用于选择目标解析规则的语法。
    pub syntax: LinkSyntax,
    /// 解析器给出的目标，尚未进行 URI 百分号解码。
    pub target: String,
    /// 链接在正文里的使用位置，而非引用定义的位置。
    pub source: Range<usize>,
    /// 不含 Markdown 样式标记的链接文字。
    pub label: String,
}

/// 按源码顺序提取正文链接，排除图片、代码和文档属性。
pub fn extract(text: &str) -> impl Iterator<Item = NoteLink> + '_ {
    let mut image_depth = 0;
    let mut pending: Option<NoteLink> = None;
    events(text, crate::markdown_options()).filter_map(move |(event, source)| match event {
        Event::Start(Tag::Image { .. }) => {
            image_depth += 1;
            None
        }
        Event::End(TagEnd::Image) => {
            image_depth -= 1;
            None
        }
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            ..
        }) if image_depth == 0 => {
            pending = Some(NoteLink {
                syntax: match link_type {
                    LinkType::WikiLink { .. } => LinkSyntax::Wiki,
                    _ => LinkSyntax::Markdown,
                },
                target: dest_url.into_string(),
                source,
                label: String::new(),
            });
            None
        }
        Event::End(TagEnd::Link) if image_depth == 0 => pending.take(),
        Event::Text(text) | Event::Code(text) | Event::InlineMath(text) => {
            if let Some(link) = &mut pending {
                link.label.push_str(&text);
            }
            None
        }
        Event::SoftBreak | Event::HardBreak => {
            if let Some(link) = &mut pending {
                link.label.push(' ');
            }
            None
        }
        _ => None,
    })
}

/// Share the Markdown event stream with the renderer, including its legacy standalone `[[TOC]]` marker.
pub(crate) fn events(
    text: &str,
    options: Options,
) -> impl Iterator<Item = (Event<'_>, Range<usize>)> + '_ {
    let mut toc = false;
    let mut skip_link = false;
    Parser::new_ext(text, options)
        .into_offset_iter()
        .filter_map(move |(event, range)| match event {
            Event::Start(Tag::Paragraph) => {
                toc = text[range.clone()].trim().eq_ignore_ascii_case("[[TOC]]");
                Some((event, range))
            }
            Event::End(TagEnd::Paragraph) => {
                toc = false;
                Some((event, range))
            }
            Event::Start(Tag::Link {
                link_type: LinkType::WikiLink { .. },
                ..
            }) if toc => {
                skip_link = true;
                Some((Event::Text(text[range.clone()].into()), range))
            }
            Event::End(TagEnd::Link) if skip_link => {
                skip_link = false;
                None
            }
            _ if skip_link => None,
            _ => Some((event, range)),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_syntax_keeps_distinct_occurrences_and_utf8_ranges() {
        let source = "中文段落：[[音乐审美？|笔记]] [笔记](./音乐审美？.md)\r\n[[音乐审美？]]";
        let links: Vec<_> = extract(source).collect();
        let actual: Vec<_> = links
            .iter()
            .map(|link| {
                (
                    link.syntax,
                    link.target.as_str(),
                    &source[link.source.clone()],
                )
            })
            .collect();
        assert_eq!(
            actual,
            [
                (LinkSyntax::Wiki, "音乐审美？", "[[音乐审美？|笔记]]"),
                (
                    LinkSyntax::Markdown,
                    "./音乐审美？.md",
                    "[笔记](./音乐审美？.md)"
                ),
                (LinkSyntax::Wiki, "音乐审美？", "[[音乐审美？]]"),
            ]
        );
    }

    #[test]
    fn reference_links_keep_the_use_site_and_raw_uri_parts() {
        let source = "[显示文字][笔记]\n\n[笔记]: ./A%3F%23.md#heading\n";
        let links: Vec<_> = extract(source).collect();
        assert_eq!(
            links,
            [NoteLink {
                syntax: LinkSyntax::Markdown,
                target: "./A%3F%23.md#heading".into(),
                source: 0.."[显示文字][笔记]".len(),
                label: "显示文字".into(),
            }]
        );
    }

    #[test]
    fn skips_code_math_images_and_front_matter() {
        let source = concat!(
            "---\ntitle: '[[属性]]'\n---\n\n",
            "`[[代码]]` $[[公式]]$ ![[图片]] ![图](./a.png)\n\n",
            "```md\n[[代码块]]\n```\n\n",
            "[[正文]]"
        );
        let targets: Vec<_> = extract(source).map(|link| link.target).collect();
        assert_eq!(targets, ["正文"]);
    }
}
