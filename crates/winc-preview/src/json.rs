//! JSON 预览的卡片树：对象字段、数组下标和标量按源代码顺序显示。

use serde::de::{Deserializer, MapAccess, Visitor};
use serde_json::value::RawValue;
use std::fmt;

/// 预览读取的字节数上限；编辑源代码不受此限制。
pub const MAX_BYTES: usize = 1024 * 1024;
/// 限制预览图的大小和布局递归深度，与编辑器的文件大小上限无关。
const MAX_ROWS: usize = 2000;
const MAX_DEPTH: usize = 64;

/// 自外向内排列的卡片，第一张是根。
pub type Graph = Vec<Vec<Row>>;

/// 卡片中的一行。有子卡片时，从这一行引出连线。
#[derive(Debug, PartialEq, Eq)]
pub struct Row {
    /// 对象键、数组下标，或根值的空标签。
    pub key: String,
    /// 标量的显示文字或容器摘要。
    pub value: Value,
    /// 非空容器对应的子卡片下标。
    pub child: Option<usize>,
}

/// 行内显示的值。容器只显示数量，成员放在子卡片中。
#[derive(Debug, PartialEq, Eq)]
pub enum Value {
    /// 对象中的字段数。
    Object(usize),
    /// 数组中的元素数。
    Array(usize),
    /// 带引号、单行显示的字符串。
    String(String),
    /// 数字在原文中的写法（过长时省略）。
    Number(String),
    /// `true`、`false` 或 `null`。
    Literal(&'static str),
}

/// 无法生成结构图的原因。
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// 不是有效的 JSON。
    Syntax {
        /// 从 1 开始的行号。
        line: usize,
        /// 从 1 开始的列号（按 UTF-8 字节计）。
        column: usize,
    },
    /// 原文大小、层数或行数超出预览上限。
    Limit,
}

/// 读取一段标准 JSON。保留重复键与数字原文，不经过 map 去重或浮点转换。
///
/// # Errors
///
/// 格式不正确或超出预览大小上限时返回原因。
pub fn parse(text: &str) -> Result<Graph, Error> {
    if text.len() > MAX_BYTES {
        return Err(Error::Limit);
    }
    let source = Source(text);
    let raw = source.read(text)?;
    let mut builder = Builder {
        source,
        graph: Vec::new(),
        rows: 0,
    };
    let (value, child) = builder.card(raw, 0)?;
    if child.is_none() {
        builder.graph.push(vec![Row {
            key: String::new(),
            value,
            child: None,
        }]);
    }
    Ok(builder.graph)
}

struct Source<'a>(&'a str);

impl<'a> Source<'a> {
    fn read<T: serde::Deserialize<'a>>(&self, text: &'a str) -> Result<T, Error> {
        serde_json::from_str(text).map_err(|err| {
            // `text` 是原文或 RawValue 返回的原文切片，子值的行列要加回它的起点。
            let start = text.as_ptr() as usize - self.0.as_ptr() as usize;
            let prefix = &self.0[..start];
            let line = prefix.bytes().filter(|&b| b == b'\n').count() + err.line().max(1);
            let column = err.column()
                + if err.line() <= 1 {
                    prefix.rsplit('\n').next().map_or(0, str::len)
                } else {
                    0
                };
            Error::Syntax {
                line,
                column: column.max(1),
            }
        })
    }
}

/// 只在需要某张卡片时读取它的成员；RawValue 借用原文，不拷贝子树。
impl<'a> serde::Deserialize<'a> for Object<'a> {
    fn deserialize<D: Deserializer<'a>>(deserializer: D) -> Result<Self, D::Error> {
        struct Fields;
        impl<'a> Visitor<'a> for Fields {
            type Value = Object<'a>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object")
            }

            fn visit_map<M: MapAccess<'a>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut fields = Vec::new();
                while let Some(pair) = map.next_entry()? {
                    fields.push(pair);
                }
                Ok(Object(fields))
            }
        }
        deserializer.deserialize_map(Fields)
    }
}

struct Object<'a>(Vec<(String, &'a RawValue)>);

struct Builder<'a> {
    source: Source<'a>,
    graph: Graph,
    rows: usize,
}

impl<'a> Builder<'a> {
    /// 返回值的摘要，以及在非空容器时生成的卡片。
    fn card(&mut self, raw: &'a RawValue, depth: usize) -> Result<(Value, Option<usize>), Error> {
        if depth > MAX_DEPTH {
            return Err(Error::Limit);
        }
        let text = raw.get();
        let (value, entries) = match text.as_bytes()[0] {
            b'{' => {
                let Object(fields) = self.source.read(text)?;
                (Value::Object(fields.len()), fields)
            }
            b'[' => {
                let array: Vec<&RawValue> = self.source.read(text)?;
                if self.rows + array.len() > MAX_ROWS {
                    return Err(Error::Limit);
                }
                let fields = array
                    .into_iter()
                    .enumerate()
                    .map(|(i, raw)| (format!("[{i}]"), raw))
                    .collect::<Vec<_>>();
                (Value::Array(fields.len()), fields)
            }
            b'"' => {
                let value: String = self.source.read(text)?;
                (Value::String(label(&value, 42, true)), Vec::new())
            }
            b't' => (Value::Literal("true"), Vec::new()),
            b'f' => (Value::Literal("false"), Vec::new()),
            b'n' => (Value::Literal("null"), Vec::new()),
            _ => (Value::Number(label(text, 42, false)), Vec::new()),
        };
        if entries.is_empty() {
            return Ok((value, None));
        }
        self.rows += entries.len();
        if self.rows > MAX_ROWS {
            return Err(Error::Limit);
        }
        let id = self.graph.len();
        self.graph.push(Vec::new());
        let mut rows = Vec::with_capacity(entries.len());
        for (key, raw) in entries {
            let (value, child) = self.card(raw, depth + 1)?;
            rows.push(Row {
                key: if key.is_empty() {
                    "\"\"".into()
                } else {
                    label(&key, 28, false)
                },
                value,
                child,
            });
        }
        self.graph[id] = rows;
        Ok((value, Some(id)))
    }
}

/// 只把短标签交给排版；控制字符转义后不另起一行。
fn label(text: &str, limit: usize, quoted: bool) -> String {
    let mut out = String::new();
    if quoted {
        out.push('"');
    }
    let mut chars = text.chars();
    for c in chars.by_ref().take(limit) {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') => {
                out.extend(c.escape_default())
            }
            c => out.push(c),
        }
    }
    if chars.next().is_some() {
        out.push('…');
    }
    if quoted {
        out.push('"');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cards_keep_member_order_and_number_spelling() {
        let graph = parse(
            r#"{"z":[{"city":"杭州"},false,[]],"a":1.230e+400,"a":900719925474099321,"":null}"#,
        )
        .unwrap();
        assert_eq!(graph.len(), 3);
        let root = &graph[0];
        assert_eq!(
            root.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(),
            ["z", "a", "a", "\"\""]
        );
        assert_eq!(root[0].value, Value::Array(3));
        assert_eq!(root[0].child, Some(1));
        assert_eq!(root[1].value, Value::Number("1.230e+400".into()));
        assert_eq!(root[2].value, Value::Number("900719925474099321".into()));
        assert_eq!(graph[1][0].key, "[0]");
        assert_eq!(graph[1][0].child, Some(2));
        assert_eq!(graph[1][1].value, Value::Literal("false"));
        assert_eq!(graph[1][2].value, Value::Array(0));
        assert_eq!(graph[1][2].child, None);
        assert_eq!(graph[2][0].value, Value::String("\"杭州\"".into()));
    }

    #[test]
    fn scalar_and_empty_roots_have_a_card() {
        for (text, value) in [
            ("{}", Value::Object(0)),
            ("[]", Value::Array(0)),
            (" null ", Value::Literal("null")),
            (r#""a\nb\"c""#, Value::String(r#""a\nb\"c""#.into())),
        ] {
            let graph = parse(text).unwrap();
            assert_eq!(graph.len(), 1);
            assert_eq!(graph[0][0].value, value);
            assert!(graph[0][0].key.is_empty());
        }
        assert!(label(&"好".repeat(100), 42, true).ends_with("…\""));
    }

    #[test]
    fn malformed_json_is_rejected_and_limits_are_bounded() {
        assert!(matches!(
            parse("{\n\"a\": }"),
            Err(Error::Syntax { line: 2, .. })
        ));
        for text in ["[1,]", "true false", "", "01", r#"{"a":"\uD800"}"#] {
            assert!(matches!(parse(text), Err(Error::Syntax { .. })), "{text:?}");
        }
        assert_eq!(parse(&" ".repeat(MAX_BYTES + 1)), Err(Error::Limit));
        let nested = "[".repeat(MAX_DEPTH + 1) + "1" + &"]".repeat(MAX_DEPTH + 1);
        assert_eq!(parse(&nested), Err(Error::Limit));
        let wide = "[".to_owned() + &"0,".repeat(MAX_ROWS) + "0]";
        assert_eq!(parse(&wide), Err(Error::Limit));
    }
}
