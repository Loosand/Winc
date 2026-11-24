//! Copy a small Markdown fragment as plain text and portable HTML.

use std::ops::Range;

use pulldown_cmark::{Event, Parser, Tag, TagEnd, html};

/// Recover boundary markers when a whole formatted span was selected by its visible text.
/// A partial selection does not pull in any unselected words.
pub fn include_markers(line: &str, selection: Range<usize>) -> Range<usize> {
    include_hidden_markers(line, selection, None)
}

/// Expand a formatted selection while keeping any syntax already shown at the layout caret.
pub fn include_hidden_markers(
    line: &str,
    selection: Range<usize>,
    revealed_at: Option<usize>,
) -> Range<usize> {
    let mut result = selection.clone();
    let mut stack: Vec<(Range<usize>, Option<Range<usize>>)> = Vec::new();
    for (event, range) in Parser::new_ext(line, super::options()).into_offset_iter() {
        match event {
            Event::Start(Tag::Strong | Tag::Emphasis | Tag::Strikethrough | Tag::Link { .. }) => {
                stack.push((range, None));
            }
            Event::End(
                TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough | TagEnd::Link,
            ) => {
                if let Some((syntax, Some(visible))) = stack.pop() {
                    if selection.start <= visible.start
                        && selection.end >= visible.end
                        && !revealed_at.is_some_and(|at| syntax.start <= at && at <= syntax.end)
                    {
                        result.start = result.start.min(syntax.start);
                        result.end = result.end.max(syntax.end);
                    }
                    extend_visible(&mut stack, visible);
                }
            }
            Event::Text(_) | Event::Code(_) | Event::InlineMath(_) => {
                extend_visible(&mut stack, range);
            }
            _ => {}
        }
    }
    result
}

fn extend_visible(stack: &mut [(Range<usize>, Option<Range<usize>>)], range: Range<usize>) {
    if let Some((_, visible)) = stack.last_mut() {
        let current = visible.get_or_insert_with(|| range.clone());
        current.end = current.end.max(range.end);
    }
}

/// Preserve the formatting of a selection contained in a single visible text run.
/// Use source offsets only for events whose visible text maps byte-for-byte to the input.
pub fn formatted_run(line: &str, selection: Range<usize>) -> Option<(String, String)> {
    let mut tags = Vec::new();
    for (event, range) in Parser::new_ext(line, super::options()).into_offset_iter() {
        match event {
            Event::Start(
                tag @ (Tag::Strong | Tag::Emphasis | Tag::Strikethrough | Tag::Link { .. }),
            ) => tags.push(tag),
            Event::End(
                TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough | TagEnd::Link,
            ) => {
                tags.pop();
            }
            Event::Text(text)
                if !tags.is_empty()
                    && range.start <= selection.start
                    && selection.end <= range.end
                    && &line[range] == text.as_ref() =>
            {
                let plain = line[selection].to_owned();
                for tag in &mut tags {
                    if let Tag::Link { dest_url, .. } = tag
                        && !safe_destination(dest_url)
                    {
                        *dest_url = "".into();
                    }
                }
                let mut rich = String::new();
                let starts = tags.iter().cloned().map(Event::Start);
                let ends = tags.iter().rev().map(|tag| Event::End(tag.to_end()));
                html::push_html(
                    &mut rich,
                    starts
                        .chain([Event::Text(plain.as_str().into())])
                        .chain(ends),
                );
                return Some((plain, rich));
            }
            _ => {}
        }
    }
    None
}

/// Render a selection that cuts through an inline format in its original context.
/// Return `None` if the excerpt does not need its context or an endpoint cannot
/// be mapped exactly (for example, a partial decoded entity).
pub fn formatted_range(source: &str, selection: Range<usize>) -> Option<(String, String)> {
    if source.get(selection.clone())?.is_empty() {
        return None;
    }
    let mut boundary = false;
    let mut exact = true;
    let events = Parser::new_ext(source, super::options())
        .into_offset_iter()
        .filter_map(|(event, range)| {
            let from = range.start.max(selection.start);
            let to = range.end.min(selection.end);
            if from >= to {
                return None;
            }
            if from == range.start && to == range.end {
                return Some(event);
            }
            match event {
                Event::Start(
                    Tag::Strong | Tag::Emphasis | Tag::Strikethrough | Tag::Link { .. },
                ) => {
                    boundary = true;
                    Some(event)
                }
                Event::Text(text) if source.get(range) == Some(text.as_ref()) => {
                    Some(Event::Text(source.get(from..to)?.into()))
                }
                Event::Text(_) | Event::Code(_) | Event::InlineMath(_) | Event::DisplayMath(_) => {
                    exact = false;
                    None
                }
                _ => Some(event),
            }
        });
    let output = copy_events(events);
    (boundary && exact && !output.0.is_empty()).then_some(output)
}

/// Clipboard fallback text plus HTML for targets that support formatting.
/// Source HTML never becomes active clipboard HTML; only safe parser events are emitted.
pub fn formatted(markdown: &str) -> (String, String) {
    copy_events(Parser::new_ext(markdown, super::options()))
}

fn copy_events<'a>(events: impl Iterator<Item = Event<'a>>) -> (String, String) {
    let mut plain = String::new();
    let mut rich = String::new();
    let events = events.filter_map(|mut event| {
        match &mut event {
            Event::Html(_) | Event::InlineHtml(_) => return None,
            Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
                if !safe_destination(dest_url) {
                    *dest_url = "".into();
                }
            }
            Event::Text(s) | Event::Code(s) | Event::InlineMath(s) | Event::DisplayMath(s) => {
                plain.push_str(s);
            }
            Event::SoftBreak => plain.push(' '),
            Event::HardBreak => plain.push('\n'),
            Event::Start(Tag::Item) => plain.push_str("• "),
            Event::TaskListMarker(done) => plain.push_str(if *done { "[x] " } else { "[ ] " }),
            Event::End(TagEnd::TableCell) => plain.push('\t'),
            Event::End(TagEnd::TableRow | TagEnd::TableHead) => {
                if plain.ends_with('\t') {
                    plain.pop();
                }
                plain.push('\n');
            }
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::CodeBlock
                | TagEnd::Item
                | TagEnd::Table,
            ) if !plain.ends_with('\n') => plain.push('\n'),
            _ => {}
        }
        Some(event)
    });
    html::push_html(&mut rich, events);
    plain.truncate(plain.trim_end_matches('\n').len());
    (plain, rich)
}

fn safe_destination(url: &str) -> bool {
    if url.contains(|c: char| c.is_control() || c == '\\') {
        return false;
    }
    let url = url.trim();
    let first = url.split(['/', '?', '#']).next().unwrap_or_default();
    first.split_once(':').is_none_or(|(scheme, _)| {
        ["http", "https", "mailto"]
            .iter()
            .any(|s| scheme.eq_ignore_ascii_case(s))
    })
}

#[cfg(test)]
mod tests;
