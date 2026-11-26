//! 解析的回归语料：逐条断言 CommonMark、GFM 中易错写法的解析结果。
//!
//! 结果写成提纲（[`outline`]）：每块一行，行内样式写回 Markdown 标记，嵌套的块缩进两格。

use super::*;

/// 把解析结果写成提纲。
fn outline(markdown: &str) -> String {
    let doc = parse(markdown);
    let mut out = String::new();
    blocks(&doc.blocks, 0, &mut out);
    out
}

fn blocks(blocks: &[Block], depth: usize, out: &mut String) {
    for block in blocks {
        let pad = "  ".repeat(depth);
        match block {
            Block::Paragraph(inline) => out.push_str(&format!("{pad}P {}\n", inline_text(inline))),
            Block::Heading(level, inline) => {
                out.push_str(&format!("{pad}H{level} {}\n", inline_text(inline)));
            }
            Block::Code { lang, text, .. } => {
                out.push_str(&format!("{pad}Code({lang}) {text:?}\n"));
            }
            Block::Quote(alert, inner) => {
                out.push_str(&format!(
                    "{pad}Quote{}\n",
                    alert.map_or(String::new(), |a| format!("({a:?})"))
                ));
                self::blocks(inner, depth + 1, out);
            }
            Block::List(start, items) => {
                out.push_str(&format!(
                    "{pad}List({})\n",
                    start.map_or("-".to_owned(), |n| n.to_string())
                ));
                for item in items {
                    let task = match item.task {
                        Some(true) => "[x] ",
                        Some(false) => "[ ] ",
                        None => "",
                    };
                    out.push_str(&format!("{pad}  Item {task}\n"));
                    self::blocks(&item.blocks, depth + 2, out);
                }
            }
            Block::Table(aligns, rows, head) => {
                let cells: Vec<String> = (rows.iter())
                    .map(|r| r.iter().map(inline_text).collect::<Vec<_>>().join(" | "))
                    .collect();
                out.push_str(&format!("{pad}Table{aligns:?} head={head} {cells:?}\n"));
            }
            Block::Math(tex, _) => out.push_str(&format!("{pad}Math {tex:?}\n")),
            Block::Rule(_) => out.push_str(&format!("{pad}Rule\n")),
            Block::Footnote(n, inner) => {
                out.push_str(&format!("{pad}Footnote {n}\n"));
                self::blocks(inner, depth + 1, out);
            }
            Block::Details {
                open,
                summary,
                blocks: inner,
            } => {
                out.push_str(&format!(
                    "{pad}Details open={open} {}\n",
                    inline_text(summary)
                ));
                self::blocks(inner, depth + 1, out);
            }
        }
    }
}

/// 行内文字，样式写回 Markdown 的标记。
fn inline_text(inline: &Inline) -> String {
    let mut out = String::new();
    let mut images = inline.images.iter();
    for (range, style) in inline.runs() {
        let mut text: String = inline.text[range].to_owned();
        if text.contains(IMAGE_MARK) {
            text = text
                .chars()
                .map(|c| match c {
                    IMAGE_MARK => images
                        .next()
                        .map_or(String::new(), |i| format!("![{}]({})", i.alt, i.url)),
                    c => c.to_string(),
                })
                .collect();
        }
        for (flag, mark) in [
            (Style::CODE, "`"),
            (Style::STRIKE, "~~"),
            (Style::EMPHASIS, "*"),
            (Style::STRONG, "**"),
        ] {
            if style.has(flag) {
                text = format!("{mark}{text}{mark}");
            }
        }
        if style.link > 0
            && let Some(url) = inline.links.get(usize::from(style.link) - 1)
        {
            text = format!("[{text}]({url})");
        }
        out.push_str(&text);
    }
    out
}

/// 逐条比对：每条是原文和期望的提纲。
fn check(cases: &[(&str, &str)]) {
    for (markdown, expected) in cases {
        assert_eq!(outline(markdown), *expected, "原文：{markdown:?}");
    }
}

#[test]
fn block_structure() {
    check(&[
        // 引用中的懒惰续行属于同一段；中文之间的软换行不加空格。
        ("> 一\n二\n", "Quote\n  P 一二\n"),
        // Setext 标题、行尾的 `#`。
        ("标题\n===\n\n## 二级 ##\n", "H1 标题\nH2 二级\n"),
        // 缩进四格为代码；未闭合的围栏延续到文末。
        ("    code\n", "Code() \"code\"\n"),
        ("~~~ rust ignore\nfn a() {}\n", "Code(rust) \"fn a() {}\"\n"),
        // 分隔线的几种写法。
        ("* * *\n\n___\n", "Rule\nRule\n"),
        // 有序列表的起始编号；松散列表的项里是段落。
        (
            "3. 甲\n4. 乙\n",
            "List(3)\n  Item \n    P 甲\n  Item \n    P 乙\n",
        ),
        // 嵌套列表和任务项。
        (
            "- 一\n  - [x] 二\n  - [ ] 三\n",
            "List(-)\n  Item \n    P 一\n    List(-)\n      Item [x] \n        P 二\n      Item [ ] \n        P 三\n",
        ),
        // GFM 提示块。
        ("> [!NOTE]\n> 注意\n", "Quote(Note)\n  P 注意\n"),
    ]);
}

#[test]
fn inline_markup() {
    check(&[
        // 反斜杠转义、实体。
        (
            "\\*不是斜体\\* &amp; &copy; &#35;\n",
            "P *不是斜体* & © #\n",
        ),
        // 词中的下划线不表示强调，星号表示强调。
        ("snake_case_name 和 a*b*c\n", "P snake_case_name 和 a*b*c\n"),
        (
            "**粗** __粗__ *斜* ~~删~~ `码`\n",
            "P **粗** **粗** *斜* ~~删~~ `码`\n",
        ),
        // 链接引用定义；尖括号自动链接和裸网址。
        ("[文档][d]\n\n[d]: /docs \"标题\"\n", "P [文档](/docs)\n"),
        (
            "<https://a.dev> 和 www.b.dev\n",
            "P [https://a.dev](https://a.dev) 和 [www.b.dev](http://www.b.dev)\n",
        ),
        // 图片的标题不计入替代文字。
        ("![图](a.png \"标题\")\n", "P ![图](a.png)\n"),
        // 行尾两个空格和反斜杠是硬换行。
        ("一  \n二\\\n三\n", "P 一\n二\n三\n"),
    ]);
}

#[test]
fn gfm_tables() {
    check(&[
        // 转义的竖线保留在单元格中；缺少的单元格补为空。
        (
            "| a | b |\n|:-|-:|\n| x \\| y | z |\n| 只有一格 |\n",
            "Table[Left, Right] head=true [\"a | b\", \"x | y | z\", \"只有一格 | \"]\n",
        ),
    ]);
}

/// 递归检查公式语法树中是否出现未识别的命令或括号不配对的错误节点。
fn collect_math_errors(node: &math::Node, out: &mut Vec<String>) {
    match node {
        math::Node::Error(err) => out.push(err.clone()),
        math::Node::Row(items) => {
            for item in items {
                collect_math_errors(item, out);
            }
        }
        math::Node::Frac(a, b, ..) => {
            collect_math_errors(a, out);
            collect_math_errors(b, out);
        }
        math::Node::Sqrt(n, inner) => {
            if let Some(n) = n {
                collect_math_errors(n, out);
            }
            collect_math_errors(inner, out);
        }
        math::Node::Scripts(base, sup, sub) | math::Node::Stack(base, sup, sub) => {
            collect_math_errors(base, out);
            if let Some(sup) = sup {
                collect_math_errors(sup, out);
            }
            if let Some(sub) = sub {
                collect_math_errors(sub, out);
            }
        }
        math::Node::Fenced(_, inner, _)
        | math::Node::Accent(_, inner)
        | math::Node::Style(_, inner)
        | math::Node::Color(_, inner)
        | math::Node::Class(_, inner)
        | math::Node::Boxed(inner)
        | math::Node::Phantom(inner) => collect_math_errors(inner, out),
        math::Node::Grid(grid) => {
            for row in &grid.rows {
                for cell in row {
                    collect_math_errors(cell, out);
                }
            }
        }
        math::Node::Sym(..) | math::Node::Op(..) | math::Node::Big(..) | math::Node::Space(_) => {}
    }
}

/// 真实场景样本（`testdata/markdown/epidemic-modeling/index.md`）的全量格式覆盖与无未识别内容校验。
#[test]
fn real_sample_epidemic_modeling_covers_all_supported_syntax() {
    use std::collections::HashSet;

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/markdown/epidemic-modeling/index.md");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("无法读取真实场景样本 {}: {e}", path.display()));
    let doc = parse(&source);

    // 1. YAML 文档属性。
    let (_, fields) = doc
        .properties
        .as_ref()
        .expect("真实样本应解析出 YAML 文档属性");
    assert!(fields.len() >= 8, "文档属性字段不足: {}", fields.len());

    #[derive(Default)]
    struct Coverage {
        headings: u8,
        alerts: HashSet<Alert>,
        plain_quotes: usize,
        nested_quotes: usize,
        unordered_lists: usize,
        ordered_lists: usize,
        nested_lists: usize,
        tasks_done: usize,
        tasks_todo: usize,
        tables: usize,
        aligns: HashSet<Align>,
        code_langs: HashSet<String>,
        indented_code: usize,
        mermaid_kinds: HashSet<&'static str>,
        unsupported_mermaid: Vec<String>,
        display_math: usize,
        inline_math: usize,
        math_errors: Vec<String>,
        rules: usize,
        details: usize,
        footnotes: usize,
        multi_para_footnotes: usize,
        style_flags: u16,
        links: Vec<String>,
        images: Vec<(String, Option<Length>)>,
        hard_breaks: usize,
        plain_text: String,
    }

    fn scan_inline(inline: &Inline, cov: &mut Coverage) {
        cov.plain_text.push_str(&inline.text);
        cov.plain_text.push('\n');
        if inline.text.contains('\n') {
            cov.hard_breaks += 1;
        }
        for (range, style) in inline.runs() {
            cov.style_flags |= style.flags;
            if style.has(Style::MATH) {
                cov.inline_math += 1;
                let tree = math::parse(&inline.text[range]);
                collect_math_errors(&tree, &mut cov.math_errors);
            }
        }
        cov.links.extend(inline.links.iter().cloned());
        for img in &inline.images {
            cov.images.push((img.url.clone(), img.width));
        }
    }

    fn walk(blocks: &[Block], quote_depth: usize, list_depth: usize, cov: &mut Coverage) {
        for block in blocks {
            match block {
                Block::Heading(level, inline) => {
                    if (1..=6).contains(level) {
                        cov.headings |= 1u8 << (level - 1);
                    }
                    scan_inline(inline, cov);
                }
                Block::Paragraph(inline) => scan_inline(inline, cov),
                Block::Quote(alert, inner) => {
                    if let Some(kind) = alert {
                        cov.alerts.insert(*kind);
                    } else {
                        cov.plain_quotes += 1;
                        if quote_depth >= 1 {
                            cov.nested_quotes += 1;
                        }
                    }
                    walk(inner, quote_depth + 1, list_depth, cov);
                }
                Block::List(start, items) => {
                    if start.is_some() {
                        cov.ordered_lists += 1;
                    } else {
                        cov.unordered_lists += 1;
                    }
                    if list_depth >= 1 {
                        cov.nested_lists += 1;
                    }
                    for item in items {
                        match item.task {
                            Some(true) => cov.tasks_done += 1,
                            Some(false) => cov.tasks_todo += 1,
                            None => {}
                        }
                        walk(&item.blocks, quote_depth, list_depth + 1, cov);
                    }
                }
                Block::Table(aligns, rows, _) => {
                    cov.tables += 1;
                    cov.aligns.extend(aligns.iter().copied());
                    for row in rows {
                        for cell in row {
                            scan_inline(cell, cov);
                        }
                    }
                }
                Block::Code { lang, text, .. } => {
                    if lang.is_empty() {
                        cov.indented_code += 1;
                    } else {
                        cov.code_langs.insert(lang.clone());
                    }
                    if lang == "mermaid" {
                        match mermaid::parse(text) {
                            Some(mermaid::Diagram::Flow(_)) => {
                                cov.mermaid_kinds.insert("flow");
                            }
                            Some(mermaid::Diagram::Sequence(_)) => {
                                cov.mermaid_kinds.insert("sequence");
                            }
                            Some(mermaid::Diagram::Pie(_)) => {
                                cov.mermaid_kinds.insert("pie");
                            }
                            Some(mermaid::Diagram::Class(_)) => {
                                cov.mermaid_kinds.insert("class");
                            }
                            Some(mermaid::Diagram::Er(_)) => {
                                cov.mermaid_kinds.insert("er");
                            }
                            Some(mermaid::Diagram::Mindmap(_)) => {
                                cov.mermaid_kinds.insert("mindmap");
                            }
                            Some(mermaid::Diagram::Git(_)) => {
                                cov.mermaid_kinds.insert("git");
                            }
                            Some(mermaid::Diagram::Gantt(_)) => {
                                cov.mermaid_kinds.insert("gantt");
                            }
                            Some(mermaid::Diagram::Unsupported(kind)) => {
                                cov.unsupported_mermaid.push(kind);
                            }
                            None => cov.unsupported_mermaid.push("<empty>".to_owned()),
                        }
                    }
                }
                Block::Math(tex, _) => {
                    cov.display_math += 1;
                    let tree = math::parse(tex);
                    collect_math_errors(&tree, &mut cov.math_errors);
                }
                Block::Rule(_) => cov.rules += 1,
                Block::Footnote(_, inner) => {
                    cov.footnotes += 1;
                    if inner.len() >= 2 {
                        cov.multi_para_footnotes += 1;
                    }
                    walk(inner, quote_depth, list_depth, cov);
                }
                Block::Details {
                    summary, blocks, ..
                } => {
                    cov.details += 1;
                    scan_inline(summary, cov);
                    walk(blocks, quote_depth, list_depth, cov);
                }
            }
        }
    }

    let mut cov = Coverage::default();
    walk(&doc.blocks, 0, 0, &mut cov);

    // 2. 块级覆盖校验。
    assert_eq!(
        cov.headings, 0b0011_1111,
        "真实样本应覆盖 1–6 级全部标题: {:06b}",
        cov.headings
    );
    assert!(doc.has_toc, "真实样本中的 [TOC] 应被展开为目录");
    assert!(
        !doc.blocks.iter().any(|b| matches!(
            b,
            Block::Paragraph(inline) if inline.text.trim().eq_ignore_ascii_case("[toc]")
        )),
        "[TOC] 段落不应残留为原始文本"
    );
    for alert in [
        Alert::Note,
        Alert::Tip,
        Alert::Important,
        Alert::Warning,
        Alert::Caution,
    ] {
        assert!(cov.alerts.contains(&alert), "缺少 GFM 提示块: {alert:?}");
    }
    assert!(cov.plain_quotes >= 1 && cov.nested_quotes >= 1);
    assert!(cov.unordered_lists >= 1 && cov.ordered_lists >= 1 && cov.nested_lists >= 1);
    assert!(cov.tasks_done >= 1 && cov.tasks_todo >= 1);
    assert!(cov.tables >= 3);
    for align in [Align::Left, Align::Center, Align::Right] {
        assert!(cov.aligns.contains(&align), "表格缺少对齐方式: {align:?}");
    }
    for lang in ["python", "r", "json", "bash", "text", "mermaid"] {
        assert!(cov.code_langs.contains(lang), "缺少代码块语言: {lang}");
    }
    assert!(cov.indented_code >= 1, "缺少缩进式代码块");
    assert!(cov.rules >= 2 && cov.details >= 1);
    assert!(cov.footnotes >= 6 && cov.multi_para_footnotes >= 1);

    // 3. Mermaid 全 8 种支持图表与公式零错误。
    assert!(
        cov.unsupported_mermaid.is_empty(),
        "真实样本存在未识别的 Mermaid 图表: {:?}",
        cov.unsupported_mermaid
    );
    for kind in [
        "flow", "sequence", "pie", "class", "er", "mindmap", "git", "gantt",
    ] {
        assert!(
            cov.mermaid_kinds.contains(kind),
            "缺少已支持的 Mermaid 图表类型: {kind}"
        );
    }
    assert!(cov.display_math >= 8 && cov.inline_math >= 10);
    assert!(
        cov.math_errors.is_empty(),
        "真实样本存在未识别的公式命令: {:?}",
        cov.math_errors
    );

    // 4. 行内样式、链接、图片与特殊语法全覆盖。
    for (name, flag) in [
        ("STRONG", Style::STRONG),
        ("EMPHASIS", Style::EMPHASIS),
        ("STRIKE", Style::STRIKE),
        ("CODE", Style::CODE),
        ("NOTE", Style::NOTE),
        ("SUP", Style::SUP),
        ("SUB", Style::SUB),
        ("MATH", Style::MATH),
        ("UNDERLINE", Style::UNDERLINE),
        ("MARK", Style::MARK),
        ("SMALL", Style::SMALL),
    ] {
        assert!(cov.style_flags & flag != 0, "真实样本缺少行内样式: {name}");
    }
    assert!(cov.hard_breaks >= 1, "真实样本缺少硬换行");
    assert!(
        cov.links.iter().any(|u| u.starts_with('#')),
        "缺少文内锚点链接"
    );
    assert!(
        cov.links.iter().any(|u| u == "generate_figures.py"),
        "缺少本地文件链接"
    );
    assert!(
        cov.links
            .iter()
            .any(|u| u.starts_with("https://doi.org/10.1098/")),
        "缺少引用式外部链接"
    );
    assert!(
        cov.links.iter().any(|u| u.contains("wikipedia.org")),
        "缺少尖括号自动链接"
    );
    assert!(
        cov.links.iter().any(|u| u.contains("www.who.int")),
        "缺少裸网址自动链接"
    );
    assert!(
        cov.images.iter().any(|(u, _)| u == "images/sir-curves.svg"),
        "缺少 SVG Markdown 图片"
    );
    assert!(
        cov.images
            .iter()
            .any(|(u, w)| u == "images/herd-immunity-threshold.svg" && *w == Some(Length::Px(640))),
        "缺少指定宽度的 HTML 图片"
    );
    assert!(
        cov.images
            .iter()
            .any(|(u, _)| u == "images/attack-rate-heatmap.png"),
        "缺少 PNG 图片链接"
    );
    assert!(cov.plain_text.contains('🦠'), "缺少原生 Emoji");
    assert!(cov.plain_text.contains('💡'), ":bulb: 短码应转换为 💡");
    assert!(cov.plain_text.contains('©'), "&copy; 实体应转换为 ©");

    // 5. 无未识别原文泄露校验。
    for leaked in [
        "维护者备注",
        "[!NOTE]",
        "[!TIP]",
        "[!IMPORTANT]",
        "[!WARNING]",
        "[!CAUTION]",
        ":bulb:",
        "==比例==",
        "[^km]",
    ] {
        assert!(
            !cov.plain_text.contains(leaked),
            "正文残留未识别的原始标记或注释: {leaked}"
        );
    }
}
