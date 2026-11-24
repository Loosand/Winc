use super::*;

#[test]
fn includes_markers_only_for_complete_visible_spans() {
    let line = "A **重点** or [link](https://example.com).";
    let start = line.find("重点").unwrap();
    assert_eq!(
        &line[include_markers(line, start..start + "重点".len())],
        "**重点**"
    );
    assert_eq!(
        &line[include_markers(line, start..start + "重".len())],
        "重"
    );
    let start = line.find("link").unwrap();
    assert_eq!(
        &line[include_markers(line, start..start + 4)],
        "[link](https://example.com)"
    );
}

#[test]
fn copying_part_of_a_bold_run_keeps_the_style_but_not_its_other_words() {
    let line = "前 **重点** 后";
    let start = line.find('点').unwrap();
    let (plain, rich) = formatted_run(line, start..start + '点'.len_utf8()).unwrap();
    assert_eq!(plain, "点");
    assert_eq!(rich, "<strong>点</strong>");
}

#[test]
fn clipping_a_multiline_span_uses_its_opening_and_closing_context() {
    let text = "**prefix\n保留**\n\n**终点**";
    let start = text.find("保留").unwrap();
    let end = text.find('点').unwrap();
    let (plain, html) = formatted_range(text, start..end).unwrap();
    assert_eq!(plain, "保留\n终");
    assert_eq!(
        html,
        "<p><strong>保留</strong></p>\n<p><strong>终</strong></p>\n"
    );
}

#[test]
fn a_clipped_link_is_filtered_like_any_other_clipboard_html() {
    let text = "链接 [甲乙](javascript:alert%281%29)\n\n后文";
    let start = text.find('乙').unwrap();
    let end = text.find('文').unwrap();
    let (plain, html) = formatted_range(text, start..end).unwrap();
    assert_eq!(plain, "乙\n后");
    assert!(!html.contains("javascript:") && !html.contains(['甲', '文']));
}

#[test]
fn html_keeps_formatting_without_executing_raw_markup_or_unsafe_urls() {
    let (plain, html) =
        formatted("A **boy** and [apples](javascript:alert%281%29) <script>bad()</script>");
    assert_eq!(plain, "A boy and apples bad()");
    assert!(html.contains("<strong>boy</strong>"));
    assert!(!html.contains("<script") && !html.contains("javascript:"));
}
