use super::*;

fn patched(source: &str, name: &str, ty: Kind, input: &str, adding: bool) -> Option<String> {
    let (range, value) = patch(source, name, ty, input, adding)?;
    let mut out = source.to_owned();
    out.replace_range(range, &value);
    Some(out)
}

#[test]
fn reads_obsidian_properties_without_losing_order_or_lists() {
    let source = "---\ncreated: \"2026-11-09\"\nkind: 文章\nauthor:\n  - Dan Abramov\ntags:\n  - 技术\n  - 前端\ncomplex:\n  level: 2\n---\n\n正文\n";
    let Header::Valid { end, fields } = header(source) else {
        panic!("valid YAML");
    };
    assert!(source[end..].ends_with("正文\n"));
    assert_eq!(
        fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        ["created", "kind", "author", "tags", "complex"]
    );
    assert_eq!(fields[0].kind, Kind::Date);
    assert_eq!(fields[3].input, "[\"技术\", \"前端\"]");
    assert!(!fields[4].editable);
    let doc = super::super::parse(source);
    assert!(
        doc.properties.is_some(),
        "header and rendered block must agree"
    );
}

#[test]
fn edits_one_field_without_normalizing_the_rest_of_the_header_or_body() {
    let source = "---\n# 顶部注释\nkind: 文章   # 保留\nauthor:\n  - Dan Abramov\nother: {a: 42} # 别动\n---\n\n正文 **原样**\n";
    let changed = patched(source, "kind", Kind::Select, "教程", false).unwrap();
    assert_eq!(changed, source.replace("kind: 文章", "kind: \"教程\""));
    let changed = patched(&changed, "author", Kind::List, "Ada, Lin", false).unwrap();
    assert!(changed.contains("author: [\"Ada\", \"Lin\"]\nother: {a: 42} # 别动"));
    assert!(changed.ends_with("\n---\n\n正文 **原样**\n"));
    assert!(patch(source, "other", Kind::Text, "no", false).is_none());
    assert!(patch(source, "kind", Kind::Text, "duplicate", true).is_none());
    assert!(
        patch(
            "---\na: &anchor core\nb: *anchor\n---\n",
            "a",
            Kind::Text,
            "new",
            false
        )
        .is_none()
    );
}

#[test]
fn validation_and_creation_use_portable_yaml() {
    for (kind, input) in [
        (Kind::Date, "2025-02-29"),
        (Kind::Number, "NaN"),
        (Kind::Checkbox, "yes"),
        (Kind::Url, "javascript:alert(1)"),
        (Kind::List, "[42]"),
    ] {
        assert!(input_value(kind, input).is_none(), "{kind:?}: {input}");
    }
    assert!(input_value(Kind::Date, "2024-02-29").is_some());
    let created = patched("正文\r\n", "评分: 样例", Kind::Number, "4.5", true).unwrap();
    assert!(created.starts_with("---\n\"评分: 样例\": 4.5\n---\n\n"));
    assert!(created.ends_with("正文\r\n"));
    assert!(patch("---\na: [\n---\n", "new", Kind::Text, "value", true).is_none());
}
