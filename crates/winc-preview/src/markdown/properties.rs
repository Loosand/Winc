//! YAML front matter for note properties. The YAML decoder validates data; edits
//! patch only the selected field rather than serializing the entire header.

use std::ops::Range;

use serde_yaml::{Mapping, Value};

use super::{Block, Inline, Style};

/// Maximum front matter size accepted by the property editor, in bytes.
pub const LIMIT: usize = 64 * 1024;
/// Internal link prefix for a property row or the add button.
pub const PROPERTY: &str = "\u{1}property:";
/// Marker distinguishing the generated property panel from user-authored details blocks.
pub const PANEL: &str = "\u{1}property-panel";

/// Presentation and validation of a note property, independent of its YAML value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Single-line free text.
    Text,
    /// YAML number.
    Number,
    /// YAML boolean.
    Checkbox,
    /// Calendar date without a time or timezone.
    Date,
    /// An HTTP or HTTPS address.
    Url,
    /// Ordered array of strings.
    List,
    /// One string from a set of labels.
    Select,
    /// Several strings from a set of labels.
    MultiSelect,
}

impl Kind {
    /// Built-in types, in menu order.
    pub const ALL: [Self; 8] = [
        Self::Text,
        Self::Number,
        Self::Checkbox,
        Self::Date,
        Self::Url,
        Self::List,
        Self::Select,
        Self::MultiSelect,
    ];

    /// Stable identifier in notebook settings.
    pub fn id(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Number => "number",
            Self::Checkbox => "checkbox",
            Self::Date => "date",
            Self::Url => "url",
            Self::List => "list",
            Self::Select => "select",
            Self::MultiSelect => "multiselect",
        }
    }

    /// Read a native or known Obsidian type identifier.
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "tags" | "aliases" | "multitext" => Some(Self::List),
            _ => Self::ALL.into_iter().find(|kind| kind.id() == id),
        }
    }

    /// Localized menu label for the type.
    pub fn label(self) -> &'static str {
        use winc_i18n::t;
        match self {
            Self::Text => t!("properties-text"),
            Self::Number => t!("properties-number"),
            Self::Checkbox => t!("properties-checkbox"),
            Self::Date => t!("properties-date"),
            Self::Url => t!("properties-url"),
            Self::List => t!("properties-list"),
            Self::Select => t!("properties-select"),
            Self::MultiSelect => t!("properties-multi-select"),
        }
    }

    /// A short example suitable for the editing field's placeholder.
    pub fn sample(self) -> &'static str {
        match self {
            Self::Checkbox => "true / false",
            Self::Date => "YYYY-MM-DD",
            Self::Number => "42",
            Self::Url => "https://example.com",
            Self::List | Self::MultiSelect => "[\"A\", \"B\"]",
            _ => "",
        }
    }

    /// Whether this type keeps a separate list of options.
    pub fn is_choice(self) -> bool {
        matches!(self, Self::Select | Self::MultiSelect)
    }
}

/// One field as read from a local Markdown note.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Field {
    /// Original YAML key.
    pub name: String,
    /// Human-readable value shown in the panel.
    pub display: String,
    /// Value as accepted by the editor for the inferred type.
    pub input: String,
    /// Suggested type if no notebook definition exists.
    pub kind: Kind,
    /// Whether the value's shape can be edited by the panel.
    pub editable: bool,
}

/// Result of reading front matter without altering the source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Header {
    /// No recognized metadata delimiters.
    Absent,
    /// Malformed, oversized, or not a mapping with string keys.
    Unsupported,
    /// A validated mapping in source order.
    Valid {
        /// Byte position immediately following the closing delimiter.
        end: usize,
        /// The fields in the order they appear in the note.
        fields: Vec<Field>,
    },
}

/// Parse the metadata at the beginning of a note.
pub fn header(source: &str) -> Header {
    let (body, end) = match bounds(source) {
        Ok(None) => return Header::Absent,
        Err(()) => return Header::Unsupported,
        Ok(Some(bounds)) => bounds,
    };
    let raw = &source[body];
    let Some(map) = mapping(raw) else {
        return Header::Unsupported;
    };
    let fields = map
        .iter()
        .map(|(key, value)| {
            let name = key.as_str().unwrap_or_default().to_owned();
            let kind = infer(value);
            let editable = supported(value);
            let input = match value {
                Value::Sequence(items) => list_literal(items),
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                _ if supported(value) => scalar(value),
                _ => String::new(),
            };
            let display = match value {
                Value::Bool(true) => "☑".into(),
                Value::Bool(false) => "☐".into(),
                Value::Sequence(items) if supported(value) => {
                    items.iter().map(scalar).collect::<Vec<_>>().join(" · ")
                }
                _ if !supported(value) => winc_i18n::t!("properties-unsupported-value").into(),
                _ => input.clone(),
            };
            Field {
                name,
                display,
                input,
                kind,
                editable,
            }
        })
        .collect();
    Header::Valid { end, fields }
}

fn bounds(source: &str) -> Result<Option<(Range<usize>, usize)>, ()> {
    let Some(first) = source.split_inclusive('\n').next() else {
        return Ok(None);
    };
    if first
        .trim_start_matches('\u{feff}')
        .trim_end_matches(['\r', '\n'])
        != "---"
    {
        return Ok(None);
    }
    let mut at = first.len();
    for line in source[at..].split_inclusive('\n') {
        if at + line.len() > LIMIT {
            return Err(());
        }
        if matches!(line.trim_end_matches(['\r', '\n']), "---" | "...") {
            return Ok(Some((first.len()..at, at + line.len())));
        }
        at += line.len();
    }
    // A lone rule (`---`) without a closing header delimiter is Markdown, not a property block.
    Ok(None)
}

fn mapping(raw: &str) -> Option<Mapping> {
    match serde_yaml::from_str::<Value>(raw).ok()? {
        Value::Null
            if raw
                .lines()
                .all(|l| l.trim().is_empty() || l.trim_start().starts_with('#')) =>
        {
            Some(Mapping::new())
        }
        Value::Mapping(map) if map.keys().all(|k| k.as_str().is_some()) => Some(map),
        _ => None,
    }
}

fn supported(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => true,
        Value::String(s) => !s.contains(['\r', '\n']),
        Value::Sequence(items) => items
            .iter()
            .all(|item| item.as_str().is_some_and(|s| !s.contains(['\r', '\n']))),
        _ => false,
    }
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

fn quoted(text: &str) -> String {
    // YAML's double quoted style from the parser's own serializer is unambiguous
    // for Unicode, apostrophes, colons and strings that resemble booleans.
    let mut escaped = String::from("\"");
    for c in text.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            c if c.is_control() => {
                use std::fmt::Write;
                let _ = write!(escaped, "\\u{:04x}", u32::from(c));
            }
            _ => escaped.push(c),
        }
    }
    escaped.push('"');
    escaped
}

fn list_literal(items: &[Value]) -> String {
    format!(
        "[{}]",
        items
            .iter()
            .map(|item| quoted(&scalar(item)))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn infer(value: &Value) -> Kind {
    match value {
        Value::Bool(_) => Kind::Checkbox,
        Value::Number(_) => Kind::Number,
        Value::Sequence(_) => Kind::List,
        Value::String(s) if date(s) => Kind::Date,
        Value::String(s) if url(s) => Kind::Url,
        _ => Kind::Text,
    }
}

fn url(text: &str) -> bool {
    ["https://", "http://"].iter().any(|prefix| {
        text.strip_prefix(prefix)
            .is_some_and(|tail| !tail.is_empty())
    }) && !text.contains(|c: char| c.is_whitespace() || c.is_control())
}

/// Parse a calendar date without depending on a locale or time zone.
pub fn date_parts(text: &str) -> Option<(i32, u32, u32)> {
    let bytes = text.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes
            .iter()
            .enumerate()
            .all(|(i, b)| [4, 7].contains(&i) || b.is_ascii_digit())
    {
        return None;
    }
    let (year, month, day) = (
        text[..4].parse().ok()?,
        text[5..7].parse().ok()?,
        text[8..].parse().ok()?,
    );
    (1..=days_in_month(year, month))
        .contains(&day)
        .then_some((year, month, day))
}

/// Length of a Gregorian calendar month (or zero for an invalid month).
pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        2 => 28 + u32::from(year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)),
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => 0,
    }
}

fn date(text: &str) -> bool {
    date_parts(text).is_some()
}

/// Validate dialog input, returning its YAML literal and any selected labels.
pub fn input_value(kind: Kind, input: &str) -> Option<(String, Vec<String>)> {
    let input = input.trim();
    if input.is_empty() {
        return Some(("null".into(), Vec::new()));
    }
    if input.contains(['\r', '\n']) {
        return None;
    }
    match kind {
        Kind::Text => Some((quoted(input), Vec::new())),
        Kind::Select => Some((quoted(input), vec![input.to_owned()])),
        Kind::Url if url(input) => Some((quoted(input), Vec::new())),
        Kind::Date if date(input) => Some((quoted(input), Vec::new())),
        Kind::Checkbox => input
            .parse::<bool>()
            .ok()
            .map(|b| (b.to_string(), Vec::new())),
        Kind::Number => input
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite())
            .and_then(|_| {
                let Value::Number(number) = serde_yaml::from_str::<Value>(input).ok()? else {
                    return None;
                };
                Some((number.to_string(), Vec::new()))
            }),
        Kind::List | Kind::MultiSelect => {
            let items = if input.starts_with('[') {
                let Value::Sequence(items) = serde_yaml::from_str::<Value>(input).ok()? else {
                    return None;
                };
                items
                    .into_iter()
                    .map(|item| item.as_str().map(str::to_owned))
                    .collect::<Option<Vec<_>>>()?
            } else {
                input
                    .split([',', '，'])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            };
            if items.iter().any(|item| item.contains(['\r', '\n'])) {
                return None;
            }
            let value = format!(
                "[{}]",
                items
                    .iter()
                    .map(|item| quoted(item))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            Some((value, if kind.is_choice() { items } else { Vec::new() }))
        }
        _ => None,
    }
}

/// Locate a conservative, editable field span. Indented children belong to the same field;
/// comments on its first line survive scalar edits, while ambiguous children remain read-only.
fn patch_span(raw: &str, name: &str, original: &Value) -> Option<Range<usize>> {
    if name.is_empty() {
        return None;
    }
    let mut offset = 0;
    for line in raw.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        if line.starts_with([' ', '\t', '#']) {
            continue;
        }
        for (colon, _) in line.match_indices(':') {
            let key = serde_yaml::from_str::<Value>(&line[..colon]).ok();
            if key.as_ref().and_then(Value::as_str) != Some(name) {
                continue;
            }
            let line = line.trim_end_matches(['\n', '\r']);
            let value_start = start + colon + 1;
            let mut end = start + line.len();
            let mut at = offset;
            for continuation in raw[offset..].split_inclusive('\n') {
                if continuation.trim().is_empty() {
                    at += continuation.len();
                    continue;
                }
                if !continuation.starts_with([' ', '\t'])
                    || continuation.trim_start().starts_with('#')
                {
                    break;
                }
                if continuation.contains('#') || line[colon + 1..].contains('#') {
                    return None;
                }
                end = at + continuation.trim_end_matches(['\r', '\n']).len();
                at += continuation.len();
            }
            let fragment = &raw[start..end];
            let matches = |s: &str| mapping(s).as_ref().and_then(|m| m.get(name)) == Some(original);
            if !matches(fragment) {
                return None;
            }
            // Preserve a real inline comment, but don't mistake hashes in quoted values for it.
            if end == start + line.len() {
                for (hash, _) in line[colon + 1..].match_indices('#') {
                    let h = colon + 1 + hash;
                    if line.as_bytes()[h - 1].is_ascii_whitespace() && matches(line[..h].trim_end())
                    {
                        end = start + line[..h].trim_end().len();
                        break;
                    }
                }
            }
            return Some(value_start..end.max(value_start));
        }
    }
    None
}

/// Return one source patch; reparse the result to verify that no unrelated field changed.
/// `source` can be a prefix containing the entire front matter, not necessarily the whole note.
pub fn patch(
    source: &str,
    name: &str,
    kind: Kind,
    value: &str,
    adding: bool,
) -> Option<(Range<usize>, String)> {
    let name = name.trim();
    if name.is_empty() || name.contains(char::is_control) {
        return None;
    }
    let (value, _) = input_value(kind, value)?;
    let Some((body, _)) = bounds(source).ok()? else {
        let Header::Absent = header(source) else {
            return None;
        };
        if !adding {
            return None;
        }
        let start = usize::from(source.starts_with('\u{feff}')) * '\u{feff}'.len_utf8();
        return Some((
            start..start,
            format!("---\n{}: {value}\n---\n\n", quoted(name)),
        ));
    };
    let raw = &source[body.clone()];
    let mut expected = mapping(raw)?;
    let desired = serde_yaml::from_str::<Value>(&value).ok()?;
    let (span, replacement) = if adding {
        if expected.contains_key(name) {
            return None;
        }
        (raw.len()..raw.len(), format!("{}: {value}\n", quoted(name)))
    } else {
        let old = expected.get(name)?;
        if old == &desired {
            return Some((body.start..body.start, String::new()));
        }
        if !supported(old) {
            return None;
        }
        (patch_span(raw, name, old)?, format!(" {value}"))
    };
    expected.insert(Value::String(name.to_owned()), desired);
    let mut candidate = raw.to_owned();
    candidate.replace_range(span.clone(), &replacement);
    (mapping(&candidate)? == expected)
        .then_some((body.start + span.start..body.start + span.end, replacement))
}

/// Build the collapsible, source-independent display block for the writing view.
pub fn panel(fields: &[Field]) -> Block {
    use winc_i18n::t;
    let mut summary = Inline::default();
    summary.links.push(PANEL.into());
    summary.push(
        t!("properties-title"),
        Style {
            flags: Style::STRONG,
            link: 0,
        },
    );
    let link = |text: &str, target: String| {
        let mut inline = Inline::default();
        inline.links.push(target);
        inline.push(text, Style::default());
        inline
    };
    let rows = fields
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let mut name = Inline::default();
            name.push(&f.name, Style::default());
            // The layout recognizes PANEL and consumes these as metadata, not hyperlinks.
            name.links.push(f.kind.id().to_owned());
            let display = match f.kind {
                Kind::Date => date_parts(&f.input).map(|(year, month, day)| {
                    t!(
                        "properties-date-display",
                        year = format!("{year:04}"),
                        month = format!("{month:02}"),
                        day = format!("{day:02}")
                    )
                }),
                _ => None,
            };
            let display = display.as_deref().unwrap_or(&f.display);
            let mut value = Inline::default();
            if f.editable {
                value.links.push(format!("{PROPERTY}{i}"));
                if f.kind.is_choice()
                    && let Some((_, labels)) = input_value(f.kind, &f.input)
                {
                    value.links.extend(labels);
                }
            }
            value.push(display, Style::default());
            vec![name, value]
        })
        .collect::<Vec<_>>();
    let mut blocks = Vec::new();
    if !rows.is_empty() {
        blocks.push(Block::Table(vec![super::Align::None; 2], rows, false));
    }
    blocks.push(Block::Paragraph(link(
        &format!("＋  {}", t!("properties-add")),
        format!("{PROPERTY}add"),
    )));
    Block::Details {
        open: true,
        summary,
        blocks,
    }
}

#[cfg(test)]
#[path = "properties_tests.rs"]
mod tests;
