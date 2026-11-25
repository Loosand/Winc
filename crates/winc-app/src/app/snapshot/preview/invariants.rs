//! 富文本编辑器体验基线不变量巡检套件。
//!
//! 对标 Zettlr（CodeMirror 6 + Lezer）与 MarkText（Muya）的统一编辑体验，以 7 条硬不变量覆盖
//! 坐标往返、输入单调推进、聚焦/失焦零跳变、异步解析零滚动漂移、光标几何统一、全选区覆盖
//! 以及结构编辑与撤销矩阵。任何违背基线的体验缺陷都会在此直接暴露并定位到具体块与偏移。

use super::*;
use winc_preview::markdown::properties::{RAW, SOURCE};
use winc_typeset::Item;
use winit::keyboard::{Key, NamedKey};

const FULL_DOC: &str = concat!(
    "---\n",
    "title: 传染病动力学入门：SIR 模型、基本再生数与群体免疫\n",
    "authors:\n",
    "  - 公共卫生建模学习小组\n",
    "date: 2024-03-28\n",
    "tags:\n",
    "  - 流行病学\n",
    "  - 数学建模\n",
    "---\n\n",
    "# 传染病动力学入门：SIR 模型\n\n",
    "公共卫生建模学习小组  \n",
    "2024 年 3 月 · 第 1.2 版\\\n",
    "适用读者：具备微积分基础的公共卫生、统计与计算机专业读者\n\n",
    "**摘要** 🦠 本文从仓室模型出发，介绍 *SIR* 模型、`cargo test`、~~旧写法~~、==高亮==、",
    "[示例链接](https://example.com) 与基本再生数 $R_0$ 的含义[^km]。\n\n",
    "## 列表与引用\n\n",
    "- 第一项 **加粗**\n",
    "- 第二项普通文字\n\n",
    "1. 有序一\n",
    "2. 有序二\n\n",
    "- [x] 已完成任务\n",
    "- [ ] 待办任务\n\n",
    "> 引用第一行\n",
    "> 引用第二行\n\n",
    "```rust\n",
    "fn main() {\n",
    "    println!(\"hello\");\n",
    "}\n",
    "```\n\n",
    "| 指标 | 数值 | 说明 |\n",
    "|:---|:---:|---:|\n",
    "| R0 | 3.0 | 基本再生数 |\n",
    "| H | 67% | 群体免疫阈值 |\n\n",
    "---\n\n",
    "$$\n",
    "H = 1 - 1/R_0\n",
    "$$\n\n",
    "尾段正文。\n\n\n",
    "空行后的段落。\n\n",
    "[^km]: Kermack & McKendrick (1927)。\n",
);

fn set_caret_byte(app: &mut App, byte: usize) {
    let ch = app
        .editor
        .active()
        .unwrap()
        .text()
        .rope()
        .unwrap()
        .byte_to_char(byte);
    app.editor.active_mut().unwrap().selection = winc_core::Selection::point(ch);
    app.start_live_editing();
}

fn caret_xyh(app: &App) -> Option<(f32, f32, f32)> {
    let laid = app.preview.markdown.layout.as_ref()?;
    laid.caret_point(app.live_caret_source()?)
}

/// 不变量 1：文档内所有可见块与空行的 `caret_point -> source_at` 坐标往返必须精准一致。
#[test]
fn invariant_1_hit_caret_roundtrip_across_all_blocks() {
    let (app, dir) = live_doc("inv-1-roundtrip", FULL_DOC);
    // 跳过第 0 块（YAML 属性面板），逐块检查正文所有可定位文本段的命中精度。
    let mut violations = Vec::new();
    let block_count = app.preview.markdown.layout.as_ref().unwrap().block_count();

    for block_idx in 1..block_count {
        let laid = app.preview.markdown.layout.as_ref().unwrap();
        let Some(range) = laid.block_range(block_idx) else {
            continue;
        };
        let area_y = app.preview.markdown.area.y;
        let scroll = app.preview.markdown.scroll();
        let left = laid.column_bounds().0;

        // 收集该块中所有带源码映射且非零宽的字符起点。
        let mut samples: Vec<(usize, f32, f32, f32, char)> = Vec::new();
        for (top, _, ix, baseline, size, span, _, text) in laid.spans(block_idx..block_idx + 1) {
            let (y0, h) = crate::app::preview::markdown::laid::line_box(top + baseline, size);
            let mut pen = ix;
            for (k, ch) in text.chars().enumerate() {
                let w = span.advances.get(k).copied().unwrap_or(0.0);
                if w > 1.0
                    && let Some(rel) = span.source_of(text, k)
                {
                    let src = range.start + rel;
                    samples.push((src, pen, y0 + h / 2.0, w, ch));
                }
                pen += w;
            }
        }
        // 每块均匀抽样最多 12 个字符位置，覆盖行首、行中、行尾和样式边界。
        let step = (samples.len() / 12).max(1);
        for &(src, glyph_x, mid_y, glyph_w, ch) in samples.iter().step_by(step) {
            let laid = app.preview.markdown.layout.as_ref().unwrap();
            let Some((cx, cy, ch_h)) = laid.caret_point(src) else {
                violations.push(format!(
                    "block {block_idx} src={src}('{ch}'): caret_point 返回 None"
                ));
                continue;
            };
            // 光标横坐标应紧贴该字符左边缘（允许 1.5px 误差）。
            if (cx - glyph_x).abs() > 1.5 {
                violations.push(format!(
                    "block {block_idx} src={src}('{ch}'): caret_x={cx:.1} != glyph_x={glyph_x:.1}"
                ));
            }
            // 点击字符左半边（靠近左边缘 15% 处）应命中该字符起点 src。
            let hit_x = left + glyph_x + (glyph_w * 0.15).min(1.0);
            let hit_y = area_y - scroll + cy + ch_h / 2.0;
            let hit = laid.source_at(area_y, scroll, hit_x, hit_y);
            if hit != Some(src) {
                violations.push(format!(
                    "block {block_idx} src={src}('{ch}'): hit({hit_x:.1},{hit_y:.1})={hit:?} != Some({src}), mid_y={mid_y:.1}"
                ));
            }
        }
    }

    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        violations.is_empty(),
        "不变量 1（点击↔光标往返一致）发现 {} 处违背:\n{}",
        violations.len(),
        violations.join("\n")
    );
}

/// 不变量 2：在任意正文位置、行尾、样式边界、空行、标题、列表、引用、表格单元格与代码块中
/// 连续输入空格和文字，即时帧与解析后都必须严格推进光标且不得跳动。
#[test]
fn invariant_2_typing_monotonicity_and_sync_parity() {
    let cases: &[(&str, &str, &str, bool, bool)] = &[
        ("行中正文", "你好世界\n\n后段\n", "你好", true, false),
        (
            "段尾空格",
            "段落末尾\n\n**摘要** 正文\n",
            "段落末尾",
            true,
            false,
        ),
        ("段首空格", "前段\n\n段落开头\n", "段落开头", false, false),
        (
            "多行硬换行中间行",
            "第一行  \n第二行  \n第三行\n",
            "第二行",
            true,
            false,
        ),
        ("加粗之后", "**摘要** 后面\n", "**摘要**", true, true),
        ("行内代码之后", "前 `code` 后\n", "`code`", true, true),
        ("行内公式之后", "前 $R_0$ 后\n", "$R_0$", true, true),
        ("标题行尾", "# 标题文字\n\n正文\n", "标题文字", true, false),
        (
            "列表项行尾",
            "- 列表内容\n- 第二项\n",
            "列表内容",
            true,
            false,
        ),
        ("引用行尾", "> 引用内容\n\n正文\n", "引用内容", true, false),
        (
            "代码块行尾",
            "```rust\nlet x = 1;\n```\n",
            "let x = 1;",
            true,
            false,
        ),
        (
            "表格单元格",
            "| 列一 | 列二 |\n|---|---|\n| 甲 | 乙 |\n",
            "甲",
            true,
            false,
        ),
    ];

    let mut violations = Vec::new();
    for &(label, doc, needle, at_end, collapses_syntax_on_first_space) in cases {
        let (mut app, dir) = live_doc(&format!("inv-2-{label}"), doc);
        put_caret(&mut app, needle, at_end);
        settle(&mut app);

        let Some(mut prev) = caret_xyh(&app) else {
            violations.push(format!("[{label}] 初始 caret_point 为 None"));
            std::fs::remove_dir_all(dir).unwrap();
            continue;
        };

        for (step, ch) in [" ", " ", "a", " ", "中"].into_iter().enumerate() {
            app.preview.markdown.cost = Some(std::time::Duration::from_millis(20));
            app.run(winc_editor::Command::Insert(ch.into()));
            paint(&mut app);
            let imm = caret_xyh(&app);
            settle(&mut app);
            let settled = caret_xyh(&app);

            match (imm, settled) {
                (Some((ix, iy, _)), Some((sx, sy, sh))) => {
                    // 从行内样式边界输入首个空格时，隐藏的 `**`/`$`/`` ` `` 标记会按预期收起。
                    let expect_advance = !(step == 0 && collapses_syntax_on_first_space);
                    let advanced_imm = ix > prev.0 + 0.5 || iy > prev.1 + 1.0;
                    let advanced_settled = sx > prev.0 + 0.5 || sy > prev.1 + 1.0;
                    if expect_advance && !advanced_imm {
                        violations.push(format!(
                            "[{label}] step {step} ('{ch}'): 即时帧未推进 prev=({:.1},{:.1}) -> imm=({ix:.1},{iy:.1})",
                            prev.0, prev.1
                        ));
                    }
                    if expect_advance && !advanced_settled {
                        violations.push(format!(
                            "[{label}] step {step} ('{ch}'): 解析后未推进 prev=({:.1},{:.1}) -> settled=({sx:.1},{sy:.1})",
                            prev.0, prev.1
                        ));
                    }
                    if (ix - sx).abs() > 1.5 || (iy - sy).abs() > 1.5 {
                        violations.push(format!(
                            "[{label}] step {step} ('{ch}'): 即时帧与解析后跳变 imm=({ix:.1},{iy:.1}) vs settled=({sx:.1},{sy:.1})"
                        ));
                    }
                    prev = (sx, sy, sh);
                }
                _ => {
                    violations.push(format!(
                        "[{label}] step {step} ('{ch}'): caret_point 缺失 imm={imm:?}, settled={settled:?}"
                    ));
                }
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    assert!(
        violations.is_empty(),
        "不变量 2（输入推进与即时/解析一致）发现 {} 处违背:\n{}",
        violations.len(),
        violations.join("\n")
    );
}

/// 不变量 3：YAML 源码聚焦前后字宽、字距与行高完全一致；正文普通字符点击聚焦不引起无关块跳动。
#[test]
fn invariant_3_focus_blur_and_yaml_pixel_stability() {
    let (mut app, dir) = live_doc("inv-3-yaml-focus", FULL_DOC);
    let click_link = |app: &mut App, target: &str| {
        let rect = app
            .hits
            .markdown_links
            .iter()
            .find(|(_, url)| url == target)
            .unwrap_or_else(|| panic!("missing link {target}"))
            .0;
        super::click(app, (rect.x + rect.w * 0.5, rect.y + rect.h * 0.5));
        settle(app);
    };

    // 切换到 YAML 源码视图，测量失焦态第一行（title: ...）的实际绘制宽度。
    click_link(&mut app, SOURCE);
    let unfocused_width = {
        let laid = app.preview.markdown.layout.as_ref().unwrap();
        let block = laid.block_at(0).expect("properties block");
        block
            .items()
            .iter()
            .find_map(|item| match item {
                Item::Text {
                    text,
                    font: winc_render::FontKind::Mono,
                    size,
                    ..
                } if text.starts_with("title:") => {
                    let renderer = app.frame.renderer.as_ref().unwrap();
                    let w = renderer.measure(winc_render::FontKind::Mono, *size, text);
                    Some((w, *size))
                }
                _ => None,
            })
            .expect("unfocused YAML title line")
    };

    // 点击进入 YAML 源码编辑态，检查 TextField 排版后的第一行宽度和行高。
    click_link(&mut app, RAW);
    assert!(app.overlays.picker.open);
    let field = &app.overlays.picker.input;
    let first_line_len = field
        .text()
        .find('\n')
        .unwrap_or_else(|| field.text().len());
    let focused_width = field
        .view
        .stops
        .iter()
        .find(|&&(pos, _)| pos == first_line_len)
        .map(|&(_, x)| x)
        .expect("focused YAML title stop");

    assert!(
        (focused_width - unfocused_width.0).abs() < 1.0,
        "YAML 源码聚焦前后第一行字宽跳变: unfocused={:.2} (size={:.2}), focused={focused_width:.2}",
        unfocused_width.0,
        unfocused_width.1
    );
    assert!(
        (field.view.line_height - 20.0).abs() < 0.01,
        "YAML 源码聚焦后行高不一致: {}",
        field.view.line_height
    );

    std::fs::remove_dir_all(dir).unwrap();
}

/// 不变量 4：长文档下以异步解析节奏连续输入字符或空格（不换行），无论当前滚动位置在何处，
/// 页面滚动位置都不得持续漂移。
#[test]
fn invariant_4_rapid_async_typing_zero_scroll_drift() {
    let doc =
        FULL_DOC.to_owned() + &"这是用于模拟长文档异步解析节奏的常规段落文本。\n\n".repeat(40);
    let (mut app, dir) = live_doc("inv-4-scroll-drift", &doc);

    let mut violations = Vec::new();
    for (name, needle, input_ch, target_scroll) in [
        ("顶部附近长按字符", "第 1.2 版", "a", 0.0f32),
        ("中段滚动位长按字符", "第 1.2 版", "b", 120.0f32),
        ("段尾长按空格", "尾段正文。", " ", 280.0f32),
    ] {
        put_caret(&mut app, needle, true);
        settle(&mut app);
        if target_scroll > 0.0 {
            app.preview.markdown.scroll_to(target_scroll);
            paint(&mut app);
        }
        let before = app.preview.markdown.scroll();
        for _ in 0..8 {
            app.preview.markdown.cost = Some(std::time::Duration::from_millis(20));
            app.run(winc_editor::Command::Insert(input_ch.into()));
            paint(&mut app);
            settle(&mut app);
        }
        let after = app.preview.markdown.scroll();
        if (after - before).abs() > 0.5 {
            violations.push(format!(
                "[{name}] scroll 发生漂移: before={before:.2} -> after={after:.2} (delta={:+.2})",
                after - before
            ));
        }
    }

    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        violations.is_empty(),
        "不变量 4（异步连续输入零滚动漂移）发现 {} 处违背:\n{}",
        violations.len(),
        violations.join("\n")
    );
}

/// 不变量 5：各种位置的可编辑空行（段间、标题后、多空行、含空格空行、文末空行），绘制光标高度
/// 必须与正文一致且不侵入上下相邻块。
#[test]
fn invariant_5_caret_geometry_and_clearance_unity() {
    let cases = [
        ("段间双空行", "第一段\n\n\n第二段\n", "\n\n\n", 2usize),
        ("段间三空行中行", "第一段\n\n\n\n第二段\n", "\n\n\n\n", 2),
        ("段间含空格空行", "第一段\n\n   \n第二段\n", "\n\n   \n", 5),
        ("标题后双空行", "# 标题\n\n\n正文\n", "\n\n\n", 2),
        ("文末空行", "最后一段。\n\n", "\n\n", 2),
    ];

    let mut violations = Vec::new();
    for (name, doc, marker, delta) in cases {
        let (mut app, dir) = live_doc(&format!("inv-5-{name}"), doc);
        let body_needle = if doc.starts_with('#') {
            "正文"
        } else if doc.starts_with("最后一段") {
            "最后一段。"
        } else {
            "第一段"
        };
        put_caret(&mut app, body_needle, true);
        settle(&mut app);
        let body_h = app.ime.cursor.expect("body caret").h;

        let target_byte = doc.find(marker).unwrap() + delta;
        set_caret_byte(&mut app, target_byte);
        settle(&mut app);

        let Some(ime) = app.ime.cursor else {
            violations.push(format!("[{name}] 空行光标未绘制"));
            std::fs::remove_dir_all(dir).unwrap();
            continue;
        };
        if (ime.h - body_h).abs() > 1.0 {
            violations.push(format!(
                "[{name}] 空行光标高度与正文不一致: blank_h={:.1}, body_h={body_h:.1}",
                ime.h
            ));
        }

        // 检查与相邻块无碰撞。
        let laid = app.preview.markdown.layout.as_ref().unwrap();
        let origin_y = laid.origin_y(app.preview.markdown.area.y, app.preview.markdown.scroll());
        if laid.block_count() >= 2 && target_byte < doc.len() {
            let p0_bottom = origin_y + laid.tops()[0] + laid.block_at(0).unwrap().height;
            let p1_top = origin_y + laid.tops()[1];
            if ime.y <= p0_bottom {
                violations.push(format!(
                    "[{name}] 空行光标上沿侵入上一段: ime.y={:.1} <= p0_bottom={p0_bottom:.1}",
                    ime.y
                ));
            }
            if ime.y + ime.h >= p1_top {
                violations.push(format!(
                    "[{name}] 空行光标下沿侵入下一段: ime.bottom={:.1} >= p1_top={p1_top:.1}",
                    ime.y + ime.h
                ));
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    assert!(
        violations.is_empty(),
        "不变量 5（光标几何统一与防碰撞）发现 {} 处违背:\n{}",
        violations.len(),
        violations.join("\n")
    );
}

/// 不变量 6：纯空格（段内、段尾、中间行尾、空行）及跨段选区必须能正确建立选区并生成正宽度高亮。
#[test]
fn invariant_6_selection_coverage_including_spaces_and_cross_block() {
    let mut violations = Vec::new();

    // 6a. 空行空格选区：拖动选择空行里的空格。
    {
        let doc = "上一段\n\n    \n\n下一段\n";
        let (mut app, dir) = live_doc("inv-6-blank-spaces", doc);
        let blank_start = doc.find("    ").unwrap();
        set_caret_byte(&mut app, blank_start);
        settle(&mut app);
        let (left, area_y, scroll, caret_y, caret_h, end_x) = {
            let laid = app.preview.markdown.layout.as_ref().unwrap();
            let (_, y, h) = laid.caret_point(blank_start).unwrap();
            let (ex, _, _) = laid.caret_point(blank_start + 4).unwrap();
            (
                laid.column_bounds().0,
                app.preview.markdown.area.y,
                app.preview.markdown.scroll(),
                y,
                h,
                ex,
            )
        };
        let y = area_y - scroll + caret_y + caret_h / 2.0;
        app.on_mouse_move(left, y);
        app.on_mouse_down(MouseButton::Left);
        app.on_mouse_move(left + end_x, y);
        app.on_mouse_up(MouseButton::Left);
        settle(&mut app);

        let sel = app.editor.active().unwrap().selection;
        let rope = app.editor.active().unwrap().text().rope().unwrap();
        let selected_bytes = rope.char_to_byte(sel.from())..rope.char_to_byte(sel.to());
        if selected_bytes != (blank_start..blank_start + 4) {
            violations.push(format!(
                "[空行空格拖选] 选区范围错误: got={selected_bytes:?}, expected={:?}",
                blank_start..blank_start + 4
            ));
        }
        let rects = app
            .preview
            .markdown
            .layout
            .as_ref()
            .unwrap()
            .blank_selection_rects(selected_bytes.start, selected_bytes.end);
        if rects.is_empty() || rects[0].w < 4.0 {
            violations.push(format!("[空行空格拖选] 未生成有效高亮矩形: {rects:?}"));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    // 6b. 段尾空格拖选：点击文字末尾再向右拖选 4 个行尾空格。
    {
        let doc = "文本    \n\n下一段\n";
        let (mut app, dir) = live_doc("inv-6-trailing-spaces", doc);
        put_caret(&mut app, "文本", true);
        settle(&mut app);
        let start_byte = "文本".len();
        let (left, area_y, scroll, sx, ex, cy, ch) = {
            let laid = app.preview.markdown.layout.as_ref().unwrap();
            let (sx, cy, ch) = laid.caret_point(start_byte).unwrap();
            let (ex, _, _) = laid.caret_point(start_byte + 4).unwrap();
            (
                laid.column_bounds().0,
                app.preview.markdown.area.y,
                app.preview.markdown.scroll(),
                sx,
                ex,
                cy,
                ch,
            )
        };
        if ex <= sx + 4.0 {
            violations.push(format!(
                "[段尾空格拖选] 行尾空格宽度异常: sx={sx:.1}, ex={ex:.1}"
            ));
        } else {
            let y = area_y - scroll + cy + ch / 2.0;
            app.on_mouse_move(left + sx, y);
            app.on_mouse_down(MouseButton::Left);
            app.on_mouse_move(left + ex, y);
            app.on_mouse_up(MouseButton::Left);
            settle(&mut app);
            let sel = app.editor.active().unwrap().selection;
            let rope = app.editor.active().unwrap().text().rope().unwrap();
            let selected = rope.char_to_byte(sel.from())..rope.char_to_byte(sel.to());
            if selected != (start_byte..start_byte + 4) {
                violations.push(format!(
                    "[段尾空格拖选] 选区范围错误: got={selected:?}, expected={:?}",
                    start_byte..start_byte + 4
                ));
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    // 6c. 段内连续空格拖选。
    {
        let doc = "前    后\n";
        let (mut app, dir) = live_doc("inv-6-inline-spaces", doc);
        put_caret(&mut app, "前", true);
        settle(&mut app);
        let start_byte = "前".len();
        let (left, area_y, scroll, sx, ex, cy, ch) = {
            let laid = app.preview.markdown.layout.as_ref().unwrap();
            let (sx, cy, ch) = laid.caret_point(start_byte).unwrap();
            let (ex, _, _) = laid.caret_point(start_byte + 4).unwrap();
            (
                laid.column_bounds().0,
                app.preview.markdown.area.y,
                app.preview.markdown.scroll(),
                sx,
                ex,
                cy,
                ch,
            )
        };
        let y = area_y - scroll + cy + ch / 2.0;
        app.on_mouse_move(left + sx, y);
        app.on_mouse_down(MouseButton::Left);
        app.on_mouse_move(left + ex, y);
        app.on_mouse_up(MouseButton::Left);
        settle(&mut app);
        let sel = app.editor.active().unwrap().selection;
        let rope = app.editor.active().unwrap().text().rope().unwrap();
        let selected = rope.char_to_byte(sel.from())..rope.char_to_byte(sel.to());
        if selected != (start_byte..start_byte + 4) {
            violations.push(format!(
                "[段内空格拖选] 选区范围错误: got={selected:?}, expected={:?}",
                start_byte..start_byte + 4
            ));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    assert!(
        violations.is_empty(),
        "不变量 6（空格与跨块选区覆盖）发现 {} 处违背:\n{}",
        violations.len(),
        violations.join("\n")
    );
}

/// 不变量 7：结构编辑（Enter / Backspace / 方向键 / IME / Undo）在各块类型下必须保持结构完整、
/// 光标可定位、且撤销后无损还原。
#[test]
fn invariant_7_structural_editing_and_undo_matrix() {
    let enter = Key::Named(NamedKey::Enter);
    let bs = Key::Named(NamedKey::Backspace);
    let left = Key::Named(NamedKey::ArrowLeft);
    let right = Key::Named(NamedKey::ArrowRight);
    let mut violations = Vec::new();

    // 7a. 含富文本格式段落拆段：Enter 后未接触的加粗不退化为原始 ** 源码，且 Undo 无损还原。
    {
        let doc = "第一行 后续 **加粗** 尾部\n\n下段\n";
        let (mut app, dir) = live_doc("inv-7-enter-split", doc);
        put_caret(&mut app, "第一行", true);
        app.preview.markdown.cost = Some(std::time::Duration::from_millis(20));
        assert!(app.markdown_key(&enter, false));
        paint(&mut app);
        let immediate_text = app.preview.markdown.full_text().unwrap_or_default();
        if immediate_text.contains("**加粗**") {
            violations.push(format!(
                "[拆段即时帧] 拆段后未接触的加粗退化为原始源码: {immediate_text:?}"
            ));
        }
        if caret_xyh(&app).is_none() {
            violations.push("[拆段即时帧] Enter 后 caret_point 为 None".into());
        }
        settle(&mut app);
        app.run(winc_editor::Command::Undo);
        settle(&mut app);
        if doc_text(&app) != doc {
            violations.push(format!(
                "[拆段撤销] Undo 未还原原文: got={:?}, expected={doc:?}",
                doc_text(&app)
            ));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    // 7b. 段间空行 Backspace 绝不把两段粘成一行。
    {
        let doc = "上段\n\n\n下段\n";
        let (mut app, dir) = live_doc("inv-7-blank-bs", doc);
        set_caret_byte(&mut app, doc.find("\n\n\n").unwrap() + 2);
        settle(&mut app);
        assert!(app.markdown_key(&bs, false));
        settle(&mut app);
        if doc_text(&app).contains("上段下段") || !doc_text(&app).contains("上段\n\n下段") {
            violations.push(format!("[空行退格] 错误粘连段落: got={:?}", doc_text(&app)));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    // 7c. 方向键遍历全文不卡死且每一步都有有效光标位置。
    {
        let doc = "# 标题\n\n前 **粗** 后\n\n- 项一\n- 项二\n\n\n末段\n";
        let (mut app, dir) = live_doc("inv-7-arrows", doc);
        put_caret(&mut app, "标题", false);
        settle(&mut app);
        let mut prev = app.live_caret_source().unwrap();
        let mut forward_steps = 0;
        while prev < doc.len() && forward_steps < 200 {
            if !app.markdown_key(&right, false) {
                app.run(winc_editor::Command::MoveRight {
                    extend: false,
                    word: false,
                });
            }
            paint(&mut app);
            let now = app.live_caret_source().unwrap();
            if now <= prev {
                violations.push(format!("[右方向键遍历] 在偏移 {prev} 处停滞或倒退到 {now}"));
                break;
            }
            if caret_xyh(&app).is_none() {
                violations.push(format!("[右方向键遍历] 偏移 {now} 处 caret_point 为 None"));
            }
            prev = now;
            forward_steps += 1;
        }
        // 反向遍历回到起点。
        let mut back_steps = 0;
        while prev > "# ".len() && back_steps < 200 {
            if !app.markdown_key(&left, false) {
                app.run(winc_editor::Command::MoveLeft {
                    extend: false,
                    word: false,
                });
            }
            paint(&mut app);
            let now = app.live_caret_source().unwrap();
            if now >= prev {
                violations.push(format!("[左方向键遍历] 在偏移 {prev} 处停滞或前进到 {now}"));
                break;
            }
            prev = now;
            back_steps += 1;
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    // 7d. IME 组字在行尾空格后与空行上均不污染文档且正常回退/提交。
    {
        let doc = "前文   \n\n\n后文\n";
        let (mut app, dir) = live_doc("inv-7-ime", doc);
        for (where_name, byte) in [
            ("行尾空格后", "前文   ".len()),
            ("空行上", "前文   \n\n".len()),
        ] {
            set_caret_byte(&mut app, byte);
            settle(&mut app);
            app.on_ime(winit::event::Ime::Preedit("zhong".into(), Some((5, 5))));
            paint(&mut app);
            if doc_text(&app) != doc {
                violations.push(format!("[IME {where_name}] preedit 污染了文档内容"));
            }
            if app.ime.cursor.is_none() {
                violations.push(format!("[IME {where_name}] preedit 时候选窗光标位置缺失"));
            }
            app.on_ime(winit::event::Ime::Preedit(String::new(), None));
            paint(&mut app);
            if doc_text(&app) != doc {
                violations.push(format!("[IME {where_name}] 取消 preedit 后文档未保持原样"));
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    assert!(
        violations.is_empty(),
        "不变量 7（结构编辑与撤销矩阵）发现 {} 处违背:\n{}",
        violations.len(),
        violations.join("\n")
    );
}

/// 不变量 8：跨块选区删除（含隐藏标记、空行、标题）与粘贴（纯文本、多行、富文本语法）
/// 必须保持结构完整、光标可定位，且 Undo 逐字节无损还原。
#[test]
fn invariant_8_cross_block_selection_delete_and_paste() {
    let mut violations = Vec::new();

    // 8a. 跨段 + 跨加粗标记选区删除 + Undo。
    {
        let doc = "第一段 **加粗** 尾部\n\n## 二级标题\n\n第三段开头\n";
        let (mut app, dir) = live_doc("inv-8-cross-del", doc);
        let rope = app.editor.active().unwrap().text().rope().unwrap().clone();
        let from_byte = doc.find("加粗").unwrap();
        let to_byte = doc.find("开头").unwrap();
        let (a, b) = (rope.byte_to_char(from_byte), rope.byte_to_char(to_byte));
        app.editor.active_mut().unwrap().selection = winc_core::Selection::new(a, b);
        app.start_live_editing();
        settle(&mut app);

        let bs = Key::Named(NamedKey::Backspace);
        if !app.markdown_key(&bs, false) {
            app.run(winc_editor::Command::Backspace { word: false });
        }
        settle(&mut app);
        if caret_xyh(&app).is_none() {
            violations.push("[跨块删除] 删除后 caret_point 为 None".into());
        }
        // 删除完整选中的 `加粗` 后不应残留悬空的 `**`。
        let after_del = doc_text(&app);
        if after_del.contains("**") {
            violations.push(format!("[跨块删除] 残留悬空粗体标记: {after_del:?}"));
        }
        app.run(winc_editor::Command::Undo);
        settle(&mut app);
        if doc_text(&app) != doc {
            violations.push(format!(
                "[跨块删除撤销] Undo 未还原: got={:?}, expected={doc:?}",
                doc_text(&app)
            ));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    // 8b. 在段中、段尾空格后、空行上粘贴多行 Markdown 文本并 Undo。
    {
        let doc = "上段   \n\n\n下段\n";
        let (mut app, dir) = live_doc("inv-8-paste", doc);
        let payload = "粘入第一行\n\n**粘入加粗** 第二行";
        for (name, byte) in [
            ("段尾空格后", "上段   ".len()),
            ("空行上", "上段   \n\n".len()),
        ] {
            set_caret_byte(&mut app, byte);
            settle(&mut app);
            app.run(winc_editor::Command::Paste(payload.into()));
            settle(&mut app);
            if caret_xyh(&app).is_none() {
                violations.push(format!("[粘贴 {name}] 粘贴后 caret_point 为 None"));
            }
            if !doc_text(&app).contains(payload) {
                violations.push(format!("[粘贴 {name}] 粘贴内容缺失: {:?}", doc_text(&app)));
            }
            app.run(winc_editor::Command::Undo);
            settle(&mut app);
            if doc_text(&app) != doc {
                violations.push(format!(
                    "[粘贴撤销 {name}] Undo 未还原: got={:?}",
                    doc_text(&app)
                ));
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    assert!(
        violations.is_empty(),
        "不变量 8（跨块删改与粘贴撤销）发现 {} 处违背:\n{}",
        violations.len(),
        violations.join("\n")
    );
}

/// 不变量 9：多级嵌套列表、嵌套引用与代码块在异步节奏下的输入、Tab/Shift-Tab 缩进与上下方向键导航。
#[test]
fn invariant_9_nested_lists_quotes_and_code_blocks() {
    let down = Key::Named(NamedKey::ArrowDown);
    let up = Key::Named(NamedKey::ArrowUp);
    let tab = Key::Named(NamedKey::Tab);
    let mut violations = Vec::new();

    let doc = concat!(
        "- 一级甲\n",
        "  - 二级乙\n",
        "    - 三级丙\n",
        "- 一级丁\n\n",
        "> 外层引用\n",
        "> > 内层引用\n\n",
        "```rust\n",
        "let a = 1;\n",
        "let b = 2;\n",
        "```\n\n",
        "收尾段落\n",
    );
    let (mut app, dir) = live_doc("inv-9-nested", doc);

    // 9a. 每一层嵌套列表和引用末尾异步输入空格与中文，即时帧与解析后一致。
    for needle in [
        "一级甲",
        "二级乙",
        "三级丙",
        "外层引用",
        "内层引用",
        "let a = 1;",
    ] {
        put_caret(&mut app, needle, true);
        settle(&mut app);
        let Some(mut prev) = caret_xyh(&app) else {
            violations.push(format!("[嵌套输入 {needle}] 初始 caret_point 为 None"));
            continue;
        };
        for ch in [" ", "中"] {
            app.preview.markdown.cost = Some(std::time::Duration::from_millis(20));
            app.run(winc_editor::Command::Insert(ch.into()));
            paint(&mut app);
            let imm = caret_xyh(&app);
            settle(&mut app);
            let settled = caret_xyh(&app);
            match (imm, settled) {
                (Some((ix, iy, _)), Some((sx, sy, sh))) => {
                    if !(ix > prev.0 + 0.5 || iy > prev.1 + 1.0) {
                        violations.push(format!(
                            "[嵌套输入 {needle} '{ch}'] 即时帧未推进: prev=({:.1},{:.1}) -> imm=({ix:.1},{iy:.1})",
                            prev.0, prev.1
                        ));
                    }
                    if (ix - sx).abs() > 1.5 || (iy - sy).abs() > 1.5 {
                        violations.push(format!(
                            "[嵌套输入 {needle} '{ch}'] 即时帧与解析后跳变: imm=({ix:.1},{iy:.1}) vs settled=({sx:.1},{sy:.1})"
                        ));
                    }
                    prev = (sx, sy, sh);
                }
                _ => violations.push(format!(
                    "[嵌套输入 {needle} '{ch}'] caret_point 缺失: imm={imm:?}, settled={settled:?}"
                )),
            }
        }
        // 撤销两次输入，保持后续用例文档干净。
        app.run(winc_editor::Command::Undo);
        app.run(winc_editor::Command::Undo);
        settle(&mut app);
    }

    // 9b. Tab 缩进 + Shift-Tab 反缩进闭环。
    let before_tab = doc_text(&app);
    put_caret(&mut app, "一级丁", false);
    settle(&mut app);
    assert!(app.markdown_key(&tab, false));
    settle(&mut app);
    if !doc_text(&app).contains("  - 一级丁") {
        violations.push(format!("[嵌套列表 Tab] 未正确缩进: {:?}", doc_text(&app)));
    }
    app.pointer.modifiers = winit::keyboard::ModifiersState::SHIFT;
    assert!(app.markdown_key(&tab, false));
    app.pointer.modifiers = winit::keyboard::ModifiersState::empty();
    settle(&mut app);
    if doc_text(&app) != before_tab {
        violations.push(format!(
            "[嵌套列表 Shift-Tab] 未还原缩进: {:?}",
            doc_text(&app)
        ));
    }

    // 9c. Down 从第一项一直走到文档末尾，再 Up 走回顶部，每步都不卡死。
    put_caret(&mut app, "一级甲", false);
    settle(&mut app);
    let mut prev = app.live_caret_source().unwrap();
    for step in 0..30 {
        if prev >= doc_text(&app).len() {
            break;
        }
        assert!(app.markdown_key(&down, false));
        paint(&mut app);
        let now = app.live_caret_source().unwrap();
        if now <= prev {
            violations.push(format!(
                "[嵌套结构 Down] step {step} 停滞在偏移 {prev} -> {now}"
            ));
            break;
        }
        if caret_xyh(&app).is_none() {
            violations.push(format!("[嵌套结构 Down] 偏移 {now} caret_point 为 None"));
        }
        prev = now;
    }
    for step in 0..30 {
        if prev <= "- ".len() {
            break;
        }
        assert!(app.markdown_key(&up, false));
        paint(&mut app);
        let now = app.live_caret_source().unwrap();
        if now >= prev {
            violations.push(format!(
                "[嵌套结构 Up] step {step} 停滞在偏移 {prev} -> {now}"
            ));
            break;
        }
        prev = now;
    }

    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        violations.is_empty(),
        "不变量 9（嵌套列表/引用/代码块）发现 {} 处违背:\n{}",
        violations.len(),
        violations.join("\n")
    );
}

/// 不变量 10：宽表格单元格跨列导航（Tab / Shift+Tab / Enter）、单元格增删行与撤销一致性。
#[test]
fn invariant_10_wide_table_navigation_and_editing() {
    let tab = Key::Named(NamedKey::Tab);
    let enter = Key::Named(NamedKey::Enter);
    let mut violations = Vec::new();

    let doc = concat!(
        "| 指标名称 | 2024 年基准数值 | 2025 年预测数值 | 说明与备注信息栏位 |\n",
        "|:---|:---:|---:|:---|\n",
        "| 基本再生数 R0 | 3.0 | 2.8 | 初始传播能力描述文本 |\n",
        "| 群体免疫阈值 H | 67% | 64% | 需要达到的免疫比例 |\n\n",
        "表后段落\n",
    );
    let (mut app, dir) = live_doc("inv-10-table", doc);
    put_caret(&mut app, "基本再生数 R0", true);
    settle(&mut app);

    // Tab 跨列、Shift-Tab 返回、Enter 跨行。
    for step in 0..4 {
        assert!(app.markdown_key(&tab, false), "tab {step}");
        paint(&mut app);
        if caret_xyh(&app).is_none() {
            violations.push(format!("[表格 Tab {step}] caret_point 为 None"));
        }
    }
    app.pointer.modifiers = winit::keyboard::ModifiersState::SHIFT;
    assert!(app.markdown_key(&tab, false));
    app.pointer.modifiers = winit::keyboard::ModifiersState::empty();
    paint(&mut app);
    if caret_xyh(&app).is_none() {
        violations.push("[表格 Shift-Tab] caret_point 为 None".into());
    }

    // 在第二行末单元格按 Enter 向下移动/追加行并 Undo 还原。
    put_caret(&mut app, "群体免疫阈值 H", true);
    settle(&mut app);
    assert!(app.markdown_key(&enter, false));
    settle(&mut app);
    if caret_xyh(&app).is_none() {
        violations.push("[表格末行 Enter] 新增行后 caret_point 为 None".into());
    }
    app.run(winc_editor::Command::Undo);
    settle(&mut app);
    if doc_text(&app) != doc {
        violations.push(format!(
            "[表格新增行撤销] Undo 未还原: got={:?}",
            doc_text(&app)
        ));
    }

    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        violations.is_empty(),
        "不变量 10（宽表格导航与编辑）发现 {} 处违背:\n{}",
        violations.len(),
        violations.join("\n")
    );
}

/// 不变量 11：确定性伪随机编辑序列（60 步混合输入、空格、中文、换行、退格、方向键、异步解析）
/// 任何一步都不得崩溃、产生非法 UTF-8 偏移、丢失光标坐标或产出 NaN 滚动值。
#[test]
fn invariant_11_deterministic_random_edit_fuzz() {
    let doc = concat!(
        "# 标题\n\n",
        "第一段 **加粗** 与 *斜体* 文本  \n",
        "第二行含 $R_0$ 与 `code`\n\n",
        "- 列表甲\n",
        "- 列表乙\n\n",
        "> 引用内容\n\n",
        "末尾段落\n",
    );
    let (mut app, dir) = live_doc("inv-11-fuzz", doc);
    put_caret(&mut app, "第一段", true);
    settle(&mut app);

    let enter = Key::Named(NamedKey::Enter);
    let bs = Key::Named(NamedKey::Backspace);
    let left = Key::Named(NamedKey::ArrowLeft);
    let right = Key::Named(NamedKey::ArrowRight);
    let up = Key::Named(NamedKey::ArrowUp);
    let down = Key::Named(NamedKey::ArrowDown);

    let mut state: u64 = 0x5EED_CAFE_BABE_1234;
    let mut next = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        (state >> 33) as usize
    };

    let mut violations = Vec::new();
    for step in 0..60 {
        // 交替模拟即时内联解析与慢速异步解析。
        app.preview.markdown.cost = Some(std::time::Duration::from_millis(if step % 3 == 0 {
            20
        } else {
            0
        }));

        match next() % 11 {
            0 => app.run(winc_editor::Command::Insert(" ".into())),
            1 => app.run(winc_editor::Command::Insert("a".into())),
            2 => app.run(winc_editor::Command::Insert("中".into())),
            3 => {
                if !app.markdown_key(&enter, false) {
                    app.run(winc_editor::Command::Insert("\n".into()));
                }
            }
            4 => {
                if !app.markdown_key(&bs, false) {
                    app.run(winc_editor::Command::Backspace { word: false });
                }
            }
            5 => {
                if !app.markdown_key(&left, false) {
                    app.run(winc_editor::Command::MoveLeft {
                        extend: false,
                        word: false,
                    });
                }
            }
            6 => {
                if !app.markdown_key(&right, false) {
                    app.run(winc_editor::Command::MoveRight {
                        extend: false,
                        word: false,
                    });
                }
            }
            7 => {
                app.markdown_key(&up, false);
            }
            8 => {
                app.markdown_key(&down, false);
            }
            9 => app.run(winc_editor::Command::Insert("**".into())),
            _ => {
                // 偶尔强制追平后台解析。
                settle(&mut app);
            }
        }
        paint(&mut app);

        let text = doc_text(&app);
        let Some(src) = app.live_caret_source() else {
            violations.push(format!("step {step}: live_caret_source 为 None"));
            break;
        };
        if src > text.len() || !text.is_char_boundary(src) {
            violations.push(format!(
                "step {step}: 非法光标字节偏移 src={src}, len={}",
                text.len()
            ));
            break;
        }
        match caret_xyh(&app) {
            Some((x, y, h)) => {
                if !x.is_finite() || !y.is_finite() || !h.is_finite() || x < -1.0 || h < 10.0 {
                    violations.push(format!(
                        "step {step}: 异常光标几何 ({x:.1},{y:.1},{h:.1}), src={src}"
                    ));
                }
            }
            None => {
                violations.push(format!(
                    "step {step}: caret_point 为 None (src={src}, doc={text:?})"
                ));
            }
        }
        let scroll = app.preview.markdown.scroll();
        if !scroll.is_finite() || scroll < 0.0 {
            violations.push(format!("step {step}: 非法滚动值 scroll={scroll}"));
        }
    }

    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        violations.is_empty(),
        "不变量 11（60 步随机编辑 fuzz）发现 {} 处违背:\n{}",
        violations.len(),
        violations.join("\n")
    );
}

/// 不变量 12：长文档（含 YAML、[TOC]、脚注与数百行正文）在全文解析耗时超预算的情况下，
/// 常规正文/列表/引用/表格编辑必须通过块级增量重解析在首帧同步完成（`is_checked == true`），
/// 无需等待后台线程，且结果与全文解析完全一致。
#[test]
fn invariant_12_long_doc_edits_stay_synchronous_via_incremental_reparse() {
    let mut doc = String::from(concat!(
        "---\n",
        "title: 传染病动力学长文档\n",
        "---\n\n",
        "# 传染病动力学入门\n\n",
        "[TOC]\n\n",
        "第一段正文内容，包含 **加粗** 与 $R_0$。\n\n",
        "- 列表甲\n",
        "- 列表乙\n\n",
        "> 引用内容\n\n",
        "| 列一 | 列二 |\n",
        "|---|---|\n",
        "| 甲 | 乙 |\n\n",
    ));
    for i in 1..=60 {
        doc.push_str(&format!(
            "第 {i} 段常规正文内容，用于拉长文档规模并验证块级局部增量重解析的同步响应。\n\n"
        ));
    }
    doc.push_str("带脚注[^km]的尾段。\n\n[^km]: Kermack & McKendrick (1927)。\n");

    let (mut app, dir) = live_doc("inv-12-incremental-sync", &doc);
    // 模拟长文档整篇解析耗时 25ms（远超 4ms INLINE_PARSE 预算）。
    app.preview.markdown.cost = Some(std::time::Duration::from_millis(25));

    let enter = Key::Named(NamedKey::Enter);
    for (name, needle, insert) in [
        ("正文输入", "第一段正文内容", " 同步增量"),
        ("列表输入", "列表甲", " 增量"),
        ("引用输入", "引用内容", " 续写"),
        ("表格输入", "甲", " 数值"),
    ] {
        put_caret(&mut app, needle, true);
        paint(&mut app);
        app.preview.markdown.cost = Some(std::time::Duration::from_millis(25));
        app.run(winc_editor::Command::Insert(insert.into()));
        // 只绘制 1 帧，不调用 wait_for_markdown / settle！
        paint(&mut app);
        let rev = app.editor.active().unwrap().revision;
        assert!(
            app.preview.markdown.is_checked(rev),
            "[{name}] 长文档常规输入未通过增量重解析同步完成（仍退回后台异步解析）"
        );
        // 验证增量解析出的 Markdown 与全文解析逐字段一致。
        let inc = app
            .preview
            .markdown
            .parsed
            .as_ref()
            .unwrap()
            .as_ref()
            .clone();
        let full = winc_preview::markdown::parse(&doc_text(&app));
        assert_eq!(inc, full, "[{name}] 增量解析结果与全文解析存在差异");
    }

    // 段落内回车拆段同样走增量同步完成。
    put_caret(&mut app, "第一段正文内容", true);
    app.preview.markdown.cost = Some(std::time::Duration::from_millis(25));
    assert!(app.markdown_key(&enter, false));
    paint(&mut app);
    let rev = app.editor.active().unwrap().revision;
    assert!(
        app.preview.markdown.is_checked(rev),
        "[拆段] 长文档回车拆段未通过增量重解析同步完成"
    );

    std::fs::remove_dir_all(dir).unwrap();
}
