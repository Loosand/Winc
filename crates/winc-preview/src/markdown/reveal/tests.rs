//! `reveal` 的测试：只显示光标所在元素的标记，续行去掉容器的标记。

use super::*;

/// 返回光标位于 `text` 的 `|` 处时显示的文字，标记用 `‹›` 括起。
fn shown(text: &str) -> String {
    let caret = text.find('|').unwrap();
    let text = text.replace('|', "");
    let (Block::Paragraph(inline) | Block::Heading(_, inline)) =
        reveal(&text, Some(caret), None, false)
    else {
        panic!();
    };
    let mut out = String::new();
    for (range, style) in inline.runs() {
        let part = &inline.text[range];
        if style.has(Style::SYNTAX) {
            out.push_str(&format!("‹{part}›"));
        } else {
            out.push_str(part);
        }
    }
    out
}

#[test]
fn only_the_touched_element_shows_its_markers() {
    // 未接触任何元素时，显示结果即排版后的文字。
    assert_eq!(shown("|默认 **关** 和 `x`"), "默认 关 和 x");
    assert_eq!(shown("默认 **关|** 和 `x`"), "默认 ‹**›关‹**› 和 x");
    assert_eq!(shown("默认 **关** 和 |`x`"), "默认 关 和 ‹`›x‹`›");
    assert_eq!(shown("见 [文档|](a.md)。"), "见 ‹[›文档‹](a.md)›。");
    // 嵌套元素只显示被接触的各层。
    assert_eq!(shown("**粗 *斜|* 粗**"), "‹**›粗 ‹*›斜‹*› 粗‹**›");
    // 标题的 `#` 在光标接触时显示。
    assert_eq!(shown("## 标|题"), "标题");
    assert_eq!(shown("|## 标题"), "‹## ›标题");
    // 光标接触到的公式显示 LaTeX 原文。
    assert_eq!(shown("设 $x|^2$。"), "设 ‹$›x^2‹$›。");
    // 光标接触到的图片显示原文，图片另起一行排在下方。
    let mark = super::super::IMAGE_MARK;
    assert_eq!(shown("|![图](a.png)"), format!("‹![图](a.png)›\n{mark}"));
    assert_eq!(shown("见 ![图](a.png) |尾"), format!("见 {mark} 尾"));
}

#[test]
fn continuation_lines_drop_container_markers() {
    // 续行的 `>` 和缩进不显示，软换行按通常规则处理（中文之间不加空格）；文字与原文逐字对照。
    let text = "一 **二**\n> 三";
    let Block::Paragraph(inline) = reveal(text, Some(0), None, false) else {
        panic!();
    };
    assert_eq!(inline.text, "一 二三");
    let three = inline.text.find('三').unwrap();
    assert_eq!(inline.source_of(three), text.find('三'));
    let two = inline.text.find('二').unwrap();
    assert_eq!(inline.source_of(two), text.find('二'));
    // 输入法组字排在光标处，其后的文字向后移动。
    let Block::Paragraph(inline) = reveal("甲乙", Some(3), Some("丙"), false) else {
        panic!();
    };
    assert_eq!(inline.text, "甲丙乙");
    // 已输入为其他种类的块（尚未重新解析）时逐字显示。
    let Block::Paragraph(inline) = reveal("- 一\n- 二", Some(0), None, false) else {
        panic!();
    };
    assert_eq!(inline.text, "- 一\n- 二");
}

#[test]
fn highlight_markers_show_when_touched() {
    // 光标在高亮里：两个 `==` 显示出来。
    assert_eq!(shown("前面 ==高|亮== 后面"), "前面 ‹==›高亮‹==› 后面");
    assert_eq!(shown("前面 =|=高亮== 后面"), "前面 ‹==›高亮‹==› 后面");
    assert_eq!(shown("前面 ==高亮==| 后面"), "前面 ‹==›高亮‹==› 后面");
    // 光标在别处：标记不显示。
    assert_eq!(shown("前面| ==高亮== 后面"), "前面 高亮 后面");
    // 不成对的 `==` 是文字。
    assert_eq!(shown("前面 == 高|亮 后面"), "前面 == 高亮 后面");
    // 转义的 `\==` 是文字，不随光标显示为标记。
    assert_eq!(shown("前面 \\==高|亮== 后面"), "前面 ==高亮== 后面");
}

#[test]
fn expanding_selection_across_a_quote_continuation_uses_source_offsets() {
    let source = "前 **甲\r\n> 乙** 后";
    let selected = source.find('甲').unwrap()..source.find("** 后").unwrap();
    let expanded = include_hidden_paragraph_markers(source, selected.clone(), None);
    assert_eq!(&source[expanded], "**甲\r\n> 乙**");
    let caret = Some(source.find('甲').unwrap());
    assert_eq!(
        include_hidden_paragraph_markers(source, selected.clone(), caret),
        selected
    );
}

#[test]
fn trailing_and_leading_spaces_are_preserved_when_caret_touches_them() {
    // 行尾输入多个空格时必须保留为普通文字，不能被吞掉或标成灰色标记。
    assert_eq!(shown("你好，打了很多空格   |"), "你好，打了很多空格   ");
    assert_eq!(shown("**摘要**   |"), "摘要   ");
    assert_eq!(shown("   |开头空格"), "   开头空格");

    // 验证行尾空格都带精确的原文位置映射。
    let text = "你好   ";
    let Block::Paragraph(inline) = reveal(text, Some(text.len()), None, false) else {
        panic!();
    };
    assert_eq!(inline.text, text);
    assert_eq!(inline.source_of("你好".len()), Some("你好".len()));
    assert_eq!(inline.source_of("你好  ".len()), Some("你好  ".len()));
}

#[test]
fn reveal_blocks_keeps_formatting_when_paragraph_splits_into_multiple_blocks() {
    let text = "2024 年 3 月 · 第 1.2 版\\\n适用读者\n\n**摘要** 经典 *SIR* 模型 $R_0$";
    let caret = text.find("**摘要**").unwrap();
    let blocks = reveal_blocks(text, Some(caret), None, false);
    assert_eq!(blocks.len(), 2);

    // 第一段不在光标下：保留排版，不露硬换行反斜杠 `\`。
    let Block::Paragraph(first) = &blocks[0] else {
        panic!("第一块应为段落");
    };
    assert!(
        !first.text.contains('\\'),
        "第一段不应显露反斜杠: {}",
        first.text
    );

    // 第二段仅在光标接触到的 `**摘要**` 处显露标记，其余 `*SIR*` 和 `$R_0$` 保持排版。
    let Block::Paragraph(second) = &blocks[1] else {
        panic!("第二块应为段落");
    };
    assert!(second.text.starts_with("**摘要**"));
    assert!(
        !second.text.contains("*SIR*"),
        "未接触的斜体不应退化为原文: {}",
        second.text
    );
    assert!(
        !second.text.contains("$R_0$"),
        "未接触的公式不应退化为原文: {}",
        second.text
    );
    assert_eq!(second.source_of(0), Some(caret));
}
