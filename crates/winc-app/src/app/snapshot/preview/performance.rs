//! 手动运行的 Markdown 性能与内存测量。

use super::*;

#[test]
#[ignore = "offscreen writing performance, run explicitly"]
fn time_single_view_writing() {
    use std::time::Instant;
    use winc_editor::Command;
    let list: String = (0..4000)
        .map(|i| format!("- 第 {i} 项，说明 **重点**\n"))
        .collect();
    let rows: String = (0..4000)
        .map(|i| format!("| {i} | 第 {i} 项 |\n"))
        .collect();
    let table = format!("| 序号 | 说明 |\n| --- | --- |\n{rows}");
    let report = |values: &mut [f64]| {
        values.sort_by(f64::total_cmp);
        (
            values[values.len() / 2],
            values[(values.len() * 95).div_ceil(100) - 1],
        )
    };
    for (name, body) in [
        ("mixed-small", long_notes(1000)),
        ("mixed-large", long_notes(10000)),
        ("list", list),
        ("table", table),
    ] {
        let text = format!("# 性能样稿\n\n标记位置\n\n{body}");
        let open = Instant::now();
        let (mut app, dir) = live_doc(&format!("md-timing-{name}"), &text);
        let open_ms = open.elapsed().as_secs_f64() * 1000.0;
        put_caret(&mut app, "标记位置", true);
        let (mut first, mut parser, mut reflow, mut settled, mut scroll, mut jump) = (
            [0.0; 16], [0.0; 16], [0.0; 16], [0.0; 16], [0.0; 16], [0.0; 16],
        );
        for i in 0..16 {
            let now = Instant::now();
            app.run(Command::Insert("字".into()));
            paint(&mut app);
            first[i] = now.elapsed().as_secs_f64() * 1000.0;
            app.wait_for_markdown();
            parser[i] = app.preview.markdown.cost.unwrap_or_default().as_secs_f64() * 1000.0;
            let parsed = Instant::now();
            paint(&mut app);
            reflow[i] = parsed.elapsed().as_secs_f64() * 1000.0;
            settled[i] = now.elapsed().as_secs_f64() * 1000.0;
        }
        for time in &mut scroll {
            let started = Instant::now();
            app.scroll_markdown(-120.0);
            paint(&mut app);
            *time = started.elapsed().as_secs_f64() * 1000.0;
        }
        for (i, time) in jump.iter_mut().enumerate() {
            let started = Instant::now();
            app.scroll_markdown_to(app.preview.markdown.height * (i + 1) as f32 / 18.0);
            paint(&mut app);
            *time = started.elapsed().as_secs_f64() * 1000.0;
        }
        eprintln!(
            "{name} {} KiB open={open_ms:.1}ms p50/p95(ms) first={:.2?} parse={:.2?} reflow={:.2?} settled={:.2?} scroll={:.2?} jump={:.2?}",
            text.len() / 1024,
            report(&mut first),
            report(&mut parser),
            report(&mut reflow),
            report(&mut settled),
            report(&mut scroll),
            report(&mut jump)
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}

/// 计时：只有一个很大的顶层块（长列表、长表格、长代码块）时，分栏滚动时每帧的耗时。
#[test]
#[ignore = "计时，手动运行"]
fn time_huge_blocks() {
    let list: String = (0..8000)
        .map(|i| format!("- 第 {i} 项，**加粗** 和 `代码`\n"))
        .collect();
    let table: String = "| a | b | c |\n|---|---|---|\n".to_owned()
        + &(0..8000)
            .map(|i| format!("| {i} | 第 {i} 行 | 说明 |\n"))
            .collect::<String>();
    let code = format!(
        "```rust\n{}```\n",
        (0..20000)
            .map(|i| format!("let x{i} = {i}; // 注释\n"))
            .collect::<String>()
    );
    for (name, text) in [("list", list), ("table", table), ("code", code)] {
        let dir = sample_folder(&format!("md-huge-{name}"));
        let path = dir.join("BIG.md");
        std::fs::write(&path, &text).unwrap();
        let mut app = app_with(&path, false);
        paint(&mut app);
        // 对照：不分栏时滚动到同样的位置。
        let started = std::time::Instant::now();
        for i in 0..50 {
            app.editor.active_mut().unwrap().scroll_line = (i * 97) as f64;
            paint(&mut app);
        }
        eprintln!("{name} 不分栏滚 50 次 {:?}", started.elapsed());
        app.set_preview_mode(PreviewMode::Split);
        app.wait_for_markdown();
        let started = std::time::Instant::now();
        paint(&mut app);
        eprintln!(
            "{name} {} KB 第一次排版 {:?}",
            text.len() / 1024,
            started.elapsed()
        );
        let started = std::time::Instant::now();
        for i in 0..50 {
            app.editor.active_mut().unwrap().scroll_line = (i * 97) as f64;
            paint(&mut app);
        }
        eprintln!("{name} 滚左边 50 次 {:?}", started.elapsed());
        // 在中间某一项（行）中修改一个字符。
        app.go_to(4000, 3, 0);
        paint(&mut app);
        app.run(winc_editor::Command::Insert("字".into()));
        app.wait_for_markdown();
        let started = std::time::Instant::now();
        paint(&mut app);
        eprintln!("{name} 改一个字 {:?}", started.elapsed());
        // 只看预览时在同一处输入：光标所在的块显示原文，每输入一个字符重排该块（不等待解析）。
        app.set_preview_mode(PreviewMode::Preview);
        settle(&mut app);
        app.run(winc_editor::Command::Insert("字".into()));
        paint(&mut app);
        let started = std::time::Instant::now();
        for _ in 0..10 {
            app.run(winc_editor::Command::Insert("字".into()));
            paint(&mut app);
        }
        eprintln!("{name} 预览中打 10 个字 {:?}", started.elapsed());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// 测量约 1 MB 的文档在预览中执行各步骤后增加的堆内存（MB）和排版缓存的条目数。
#[test]
#[ignore = "测量，手动运行"]
// 按全局分配器（`large_alloc`，仅 macOS）记录的大块内存计算。
#[cfg(target_os = "macos")]
fn measure_markdown_memory() {
    let mb = |b: u64| b as f64 / (1024.0 * 1024.0);
    let section = SAMPLE.replace("logo.svg", "missing.svg");
    let text: String = (0..900)
        .map(|i| format!("## 第 {i} 节\n\n{section}\n"))
        .collect();
    let dir = sample_folder("md-memory");
    let path = dir.join("BIG.md");
    std::fs::write(&path, &text).unwrap();
    std::fs::write(dir.join("other.rs"), "fn main() {}\n").unwrap();
    let mut app = app_with(&path, false);
    paint(&mut app);
    let report = |app: &App, step: &str, base: isize| {
        let memo = &app.preview.markdown.memo;
        eprintln!(
            "{step:<16} {:>7.1} MB  缓存 {} 条",
            mb(
                (crate::large_alloc::LIVE.load(std::sync::atomic::Ordering::Relaxed) - base).max(0)
                    as u64
            ),
            memo.len()
        );
    };
    let base = crate::large_alloc::LIVE.load(std::sync::atomic::Ordering::Relaxed);
    report(&app, "打开源代码", base);
    app.set_preview_mode(PreviewMode::Preview);
    settle(&mut app);
    report(&app, "只看预览", base);
    if std::env::var_os("WINC_MD_BREAKDOWN").is_some() {
        let laid = app.preview.markdown.layout.take();
        drop(laid);
        report(&app, "去掉排版结果", base);
        app.preview.markdown.memo = Default::default();
        report(&app, "去掉排版缓存", base);
        heap_breakdown(app.preview.markdown.parsed.as_ref().unwrap());
        app.preview.markdown.parsed = None;
        report(&app, "去掉解析结果", base);
        return;
    }
    // 从开头滚动到末尾。
    let height = app.preview.markdown.height;
    let mut y = 0.0;
    while y < height {
        app.preview.markdown.scroll_to(y);
        paint(&mut app);
        y += app.preview.markdown.area.h;
    }
    report(&app, "滚到底", base);
    // 在中间的段落中输入 300 个字符，每个字符绘制一帧并等待一次解析。
    let at = text.len() / 2;
    let at = text[at..].find("轻量").map_or(at, |i| at + i);
    let doc = app.editor.active_mut().unwrap();
    let pos = doc.text().rope().unwrap().byte_to_char(at);
    doc.selection = winc_core::Selection::point(pos);
    for _ in 0..300 {
        app.run(winc_editor::Command::Insert("字".into()));
        app.wait_for_markdown();
        paint(&mut app);
    }
    report(&app, "打 300 个字", base);
    app.set_preview_mode(PreviewMode::Source);
    settle(&mut app);
    report(&app, "换回源代码", base);
    app.set_preview_mode(PreviewMode::Preview);
    settle(&mut app);
    app.open_path(&dir.join("other.rs"));
    settle(&mut app);
    report(&app, "换到别的标签", base);
    let _ = std::fs::remove_dir_all(&dir);
}

/// 计时：用约 1 MB 的文档测量首次排版与修改一个字符后重排的耗时。
#[test]
#[ignore = "计时，手动运行"]
fn time_large_markdown() {
    let section = SAMPLE.replace("logo.svg", "missing.svg");
    let text: String = (0..900)
        .map(|i| format!("## 第 {i} 节\n\n{section}\n"))
        .collect();
    eprintln!("{} KB", text.len() / 1024);
    let dir = sample_folder("md-large");
    let path = dir.join("BIG.md");
    std::fs::write(&path, &text).unwrap();
    let mut app = app_with(&path, false);
    paint(&mut app);
    app.set_preview_mode(PreviewMode::Split);
    let started = std::time::Instant::now();
    app.wait_for_markdown();
    eprintln!("解析 {:?}", started.elapsed());
    let started = std::time::Instant::now();
    paint(&mut app);
    eprintln!("第一次排版加两帧 {:?}", started.elapsed());
    let started = std::time::Instant::now();
    paint(&mut app);
    eprintln!("再画两帧 {:?}", started.elapsed());
    settle(&mut app);
    app.run(winc_editor::Command::Insert("字".into()));
    app.wait_for_markdown();
    let started = std::time::Instant::now();
    paint(&mut app);
    eprintln!("改一个字后重排加两帧 {:?}", started.elapsed());
    // 一张图片读取完成：只重排用到该图片的块。
    app.preview
        .markdown
        .images
        .mark_loaded_for_test(dir.join("other.png"));
    let started = std::time::Instant::now();
    paint(&mut app);
    eprintln!("读好一张图片后重排加两帧 {:?}", started.elapsed());
    // 分栏时滚动左侧文字（右侧随之滚动）、滚动右侧预览（左侧随之滚动），每次一帧。
    let started = std::time::Instant::now();
    for i in 0..100 {
        app.editor.active_mut().unwrap().scroll_line = (i * 37) as f64;
        paint(&mut app);
    }
    eprintln!("滚左边 100 帧 {:?}", started.elapsed());
    let started = std::time::Instant::now();
    for _ in 0..100 {
        app.scroll_markdown(-300.0);
        paint(&mut app);
    }
    eprintln!("滚右边 100 帧 {:?}", started.elapsed());
    let _ = std::fs::remove_dir_all(&dir);
}

/// 不变量 16：真实粗体、中西文/数字/公式混排留白、连续全角标点挤压、行首开括号挤压与拉丁字偶调整
/// 全部只作用于显示层，不修改 Markdown 原文；光标、点击命中、选区矩形及在中西文交界键入空格均严格精确。
#[test]
fn invariant_16_typography_preserves_caret_hit_and_selection_geometry() {
    use super::invariants::{caret_xyh, set_caret_byte};

    let doc = concat!(
        "# 标题Title与**粗体Rust**\n\n",
        "「测试」，（《嵌套》）中文Rust版本2026年，当 $R_0 > 1$ 时 AVATAR To We。\n",
    );
    let (mut app, dir) = live_doc("inv-16-typography", doc);
    settle(&mut app);

    let mut violations = Vec::new();
    let laid = app.preview.markdown.layout.as_ref().unwrap();
    let area = app.preview.markdown.area;
    let scroll = app.preview.markdown.scroll();
    let (left, _) = laid.column_bounds();

    assert_eq!(doc_text(&app), doc, "排版微调不得改动原文");
    for needle in [
        "标题Title",
        "粗体Rust",
        "「测试」，",
        "（《嵌套》）",
        "中文Rust版本2026年",
        "AVATAR",
    ] {
        let start = doc.find(needle).unwrap();
        let end = start + needle.len();
        let rects = laid.mark_rects(start..end);
        if rects.is_empty() || rects.iter().any(|r| r.w <= 0.0 || r.h <= 0.0) {
            violations.push(format!("[排版选区 {needle}] mark_rects 无效: {rects:?}"));
        }
        let mut prev_pt: Option<(f32, f32)> = None;
        for (rel, ch) in needle.char_indices() {
            let byte = start + rel;
            let Some((cx, cy, ch_h)) = laid.caret_point(byte) else {
                violations.push(format!("[排版光标 {needle}] 字节 {byte} ('{ch}') 无位置"));
                continue;
            };
            if let Some((px, py)) = prev_pt
                && (cy - py).abs() < 1.0
                && cx <= px + 0.1
            {
                violations.push(format!(
                    "[排版光标单调性 {needle}] '{ch}' 未右移: prev={px:.2} -> cur={cx:.2}"
                ));
            }
            prev_pt = Some((cx, cy));
            let hit = laid.source_at(
                area.y,
                scroll,
                left + cx + 1.0,
                area.y - scroll + cy + ch_h * 0.5,
            );
            if hit.is_none_or(|b| b < start || b > end) {
                violations.push(format!(
                    "[排版点击回环 {needle}] 点击 '{ch}'({byte}) 命中偏移 {hit:?} 越出 {start}..={end}"
                ));
            }
        }
    }

    let boundary = doc.find("中文Rust").unwrap() + "中文".len();
    set_caret_byte(&mut app, boundary);
    settle(&mut app);
    let before_pt = caret_xyh(&app).expect("中西文交界应有光标");
    app.preview.markdown.cost = Some(std::time::Duration::from_millis(20));
    app.run(winc_editor::Command::Insert(" ".into()));
    paint(&mut app);
    let imm_pt = caret_xyh(&app).expect("键入空格即时帧应有光标");
    settle(&mut app);
    let settled_pt = caret_xyh(&app).expect("键入空格解析后应有光标");
    if !(imm_pt.0 > before_pt.0 + 0.5 || imm_pt.1 > before_pt.1 + 1.0) {
        violations.push(format!(
            "[中西文交界空格] 即时帧未前进: before={before_pt:?}, imm={imm_pt:?}"
        ));
    }
    if (imm_pt.0 - settled_pt.0).abs() > 1.5 || (imm_pt.1 - settled_pt.1).abs() > 1.5 {
        violations.push(format!(
            "[中西文交界空格] 即时帧与解析后跳变: imm={imm_pt:?}, settled={settled_pt:?}"
        ));
    }
    app.run(winc_editor::Command::Undo);
    settle(&mut app);
    if doc_text(&app) != doc {
        violations.push("[中西文交界空格] Undo 后原文未还原".into());
    }

    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        violations.is_empty(),
        "不变量 16（排版质量与几何一致性）发现 {} 处违背:\n{}",
        violations.len(),
        violations.join("\n")
    );
}

/// 不变量 17：长文档视口外块卸载与原地刷新、以及超大单块（表/列表/代码）原位修补的伸缩性与几何正确性。
#[test]
fn invariant_17_long_document_and_huge_block_scaling_stays_bounded() {
    use super::invariants::{caret_xyh, set_caret_byte};
    use std::time::Instant;

    let short_text = format!("# 短文\n\n标记位置\n\n{}", long_notes(20));
    let long_text = format!("# 长文\n\n标记位置\n\n{}", long_notes(400));

    let measure = |tag: &str, text: &str| {
        let (mut app, dir) = live_doc(tag, text);
        put_caret(&mut app, "标记位置", true);
        settle(&mut app);
        app.run(winc_editor::Command::Insert("字".into()));
        settle(&mut app);
        let t0 = Instant::now();
        for _ in 0..4 {
            app.run(winc_editor::Command::Insert("字".into()));
            paint(&mut app);
        }
        let type_ms = t0.elapsed().as_secs_f64() * 1000.0 / 4.0;

        let t1 = Instant::now();
        for i in 1..=6 {
            let y = app.preview.markdown.height * i as f32 / 8.0;
            app.scroll_markdown_to(y);
            paint(&mut app);
        }
        let scroll_ms = t1.elapsed().as_secs_f64() * 1000.0 / 6.0;

        let near_active = app
            .preview
            .markdown
            .layout
            .as_ref()
            .is_some_and(|l| l.near().is_some());
        let shed_count = app.preview.markdown.layout.as_ref().map_or(0, |l| {
            l.iter_blocks().filter(|(_, _, b)| b.is_shed()).count()
        });
        std::fs::remove_dir_all(dir).unwrap();
        (type_ms, scroll_ms, near_active, shed_count)
    };

    let (short_type, short_scroll, _, _) = measure("inv-17-short", &short_text);
    let (long_type, long_scroll, near_active, shed_count) = measure("inv-17-long", &long_text);
    assert!(near_active, "超过 SHED_BLOCKS 的长文档应启用视口邻域卸载");
    assert!(
        shed_count > 500,
        "长文档滚动后远端块应保持卸载状态: shed={shed_count}"
    );
    let type_ratio = long_type / short_type.max(0.5);
    let scroll_ratio = long_scroll / short_scroll.max(0.5);
    assert!(
        type_ratio < 6.0,
        "20 倍长文档输入耗时比例退化: short={short_type:.2}ms, long={long_type:.2}ms, ratio={type_ratio:.2}x"
    );
    assert!(
        scroll_ratio < 6.0,
        "20 倍长文档跳转滚动耗时比例退化: short={short_scroll:.2}ms, long={long_scroll:.2}ms, ratio={scroll_ratio:.2}x"
    );

    let huge_table: String = "| 序号 | 名称 | 备注 |\n| --- | --- | --- |\n".to_owned()
        + &(0..160)
            .map(|i| format!("| {i} | 第 {i} 行 | 说明 |\n"))
            .collect::<String>();
    let huge_list: String = (0..160)
        .map(|i| format!("- 第 {i} 项，含 **粗体** 与 `code`\n"))
        .collect();
    let huge_code = format!(
        "```rust\n{}```\n",
        (0..220)
            .map(|i| format!("let v{i} = {i}; // 行 {i}\n"))
            .collect::<String>()
    );
    for (name, text, needle) in [
        ("huge-table", huge_table, "| 80 |"),
        ("huge-list", huge_list, "第 80 项"),
        ("huge-code", huge_code, "let v100 = 100;"),
    ] {
        let (mut app, dir) = live_doc(&format!("inv-17-{name}"), &text);
        let at = text.find(needle).unwrap() + needle.len().min(3);
        set_caret_byte(&mut app, at);
        settle(&mut app);
        let mut prev = caret_xyh(&app).expect("初始超大块应有光标");
        for _ in 0..4 {
            app.preview.markdown.cost = Some(std::time::Duration::from_millis(20));
            app.run(winc_editor::Command::Insert("字".into()));
            paint(&mut app);
            let now = caret_xyh(&app).expect("超大块原位修补后应有光标");
            assert!(
                now.0 > prev.0 + 0.5 || now.1 > prev.1 + 1.0,
                "[{name}] 原位修补后光标未右移: prev={prev:?} -> now={now:?}"
            );
            prev = now;
        }
        settle(&mut app);
        let settled = caret_xyh(&app).expect("超大块解析后应有光标");
        assert!(
            (prev.0 - settled.0).abs() < 1.5 && (prev.1 - settled.1).abs() < 1.5,
            "[{name}] 超大块原位修补与解析后跳变: patched={prev:?} vs settled={settled:?}"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}

/// 不变量 18：全格式实战样稿（`testdata/markdown/epidemic-modeling/index.md`，只读副本）
/// 必须覆盖全部核心 Markdown 格式，并在完整滚动与点击/光标查询下零崩溃、几何稳定。
#[test]
fn invariant_18_showcase_document_format_coverage_and_stability() {
    use winc_preview::markdown::{Block, Style, parse};

    let showcase = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/markdown/epidemic-modeling/index.md");
    let Ok(content) = std::fs::read_to_string(&showcase) else {
        return;
    };

    let parsed = parse(&content);
    assert!(
        parsed.properties.is_some(),
        "样稿应包含 YAML frontmatter 属性"
    );

    #[derive(Default)]
    struct Seen {
        headings: u8,
        tables: usize,
        code: usize,
        mermaid: usize,
        display_math: usize,
        inline_math: usize,
        quotes: usize,
        lists: usize,
        footnotes: usize,
        links: usize,
        images: usize,
    }
    fn scan_inline(inline: &winc_preview::markdown::Inline, seen: &mut Seen) {
        if !inline.links.is_empty() {
            seen.links += inline.links.len();
        }
        if !inline.images.is_empty() {
            seen.images += inline.images.len();
        }
        if inline.runs.iter().any(|r| r.style.has(Style::MATH)) {
            seen.inline_math += 1;
        }
    }
    fn walk(blocks: &[Block], seen: &mut Seen) {
        for b in blocks {
            match b {
                Block::Heading(lvl, inline) => {
                    seen.headings |= 1u8 << (*lvl).clamp(1, 6);
                    scan_inline(inline, seen);
                }
                Block::Paragraph(inline) => scan_inline(inline, seen),
                Block::Table(_, rows, _) => {
                    seen.tables += 1;
                    for r in rows {
                        for c in r {
                            scan_inline(c, seen);
                        }
                    }
                }
                Block::Code { lang, .. } => {
                    seen.code += 1;
                    if lang == "mermaid" {
                        seen.mermaid += 1;
                    }
                }
                Block::Math(..) => seen.display_math += 1,
                Block::Quote(_, inner) => {
                    seen.quotes += 1;
                    walk(inner, seen);
                }
                Block::List(_, items) => {
                    seen.lists += 1;
                    for it in items {
                        walk(&it.blocks, seen);
                    }
                }
                Block::Footnote(_, inner) => {
                    seen.footnotes += 1;
                    walk(inner, seen);
                }
                Block::Details {
                    summary, blocks, ..
                } => {
                    scan_inline(summary, seen);
                    walk(blocks, seen);
                }
                Block::Rule(_) => {}
            }
        }
    }
    let mut seen = Seen::default();
    walk(&parsed.blocks, &mut seen);
    assert!(seen.headings.count_ones() >= 3, "样稿应覆盖至少 3 级标题");
    assert!(seen.tables >= 1, "样稿应包含表格");
    assert!(
        seen.code >= 1 && seen.mermaid >= 1,
        "样稿应包含代码块与 Mermaid 图"
    );
    assert!(
        seen.display_math >= 1 && seen.inline_math >= 1,
        "样稿应包含块级与行内公式"
    );
    assert!(seen.quotes >= 1, "样稿应包含引用/提示块");
    assert!(seen.lists >= 1, "样稿应包含列表");
    assert!(seen.footnotes >= 1, "样稿应包含脚注");
    assert!(seen.links >= 1, "样稿应包含链接");

    let (mut app, dir) = live_doc("inv-18-showcase", &content);
    settle(&mut app);
    let height = app.preview.markdown.height;
    assert!(height > 1000.0, "样稿排版高度过小: {height}");
    let step = (app.preview.markdown.area.h * 0.8).max(300.0);
    let mut y = 0.0;
    while y < height {
        app.scroll_markdown_to(y);
        paint(&mut app);
        y += step;
    }
    app.scroll_markdown_to(0.0);
    paint(&mut app);
    let laid = app.preview.markdown.layout.as_ref().unwrap();
    assert!(
        !laid.anchor_positions().is_empty(),
        "样稿排版应生成标题锚点表"
    );
    if let Some(at) = content.find("仓室模型（compartmental model）") {
        assert!(
            laid.caret_point(at).is_some(),
            "样稿正文中西文混排处应可定位光标"
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}
