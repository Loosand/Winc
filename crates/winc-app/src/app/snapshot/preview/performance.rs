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
    app.preview.markdown.loaded.push(dir.join("other.png"));
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
