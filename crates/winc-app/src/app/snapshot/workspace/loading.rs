//! 后台读取新打开文件的测试：读取期间的跳转、切换标签、关闭标签，以及磁盘上的外部修改。

use std::time::{Duration, Instant, SystemTime};

use winc_editor::LoadJob;

use super::*;

/// 按 `open_path` 的方式打开标签并发起读取，但不立即收下结果。
///
/// 返回文档 ID、读取凭证（ticket）与任务，由测试决定何时交回结果。
fn open_unloaded(app: &mut App, path: &Path) -> (DocumentId, u64, LoadJob) {
    let index = app.editor.open_tab(path);
    let job = app.editor.begin_load(index).unwrap();
    let id = job.id();
    let ticket = app.disk.loads.tickets.issue(id);
    app.disk.loads.since = Some((id, Instant::now()));
    (id, ticket, job)
}

fn deliver(app: &mut App, (id, ticket, job): (DocumentId, u64, LoadJob)) {
    app.disk
        .loads
        .results
        .0
        .send((id, ticket, job.run()))
        .unwrap();
    app.poll_loads();
}

/// 返回打开着 `path` 的标签序号。
fn tab_of(app: &App, path: &Path) -> Option<usize> {
    let path = winc_config::files::canonicalize(path).ok()?;
    app.editor
        .documents
        .iter()
        .position(|d| d.path.as_deref() == Some(path.as_path()))
}

/// 生成 `lines` 行 `line <行号>` 的样本，每行都不同，便于断言跳转到的行。
fn numbered(lines: usize) -> String {
    (0..lines).map(|i| format!("line {i}\n")).collect()
}

#[test]
fn jumps_wait_until_the_file_is_read() {
    let dir = sample_folder("loading-jump");
    let path = dir.join("long.txt");
    std::fs::write(&path, numbered(100)).unwrap();
    let mut app = app_with(&dir, false);

    let load = open_unloaded(&mut app, &path);
    let id = load.0;
    // 还在读：标签是空的，不显示「只读」，跳转先记下。
    assert!(app.editor.active().unwrap().is_pending());
    assert!(app.holding_frame(Instant::now()).is_some());
    app.jump_to(42, 2, 0);
    assert_eq!(app.disk.loads.then.len(), 1);

    deliver(&mut app, load);
    let doc = app.editor.active().unwrap();
    assert_eq!(doc.id, id);
    assert!(!doc.is_pending());
    assert_eq!(doc.text().pos_to_line_col(doc.selection.head), (42, 2));
    assert!(app.disk.loads.then.is_empty());
    assert!(app.holding_frame(Instant::now()).is_none());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_jump_waits_for_its_tab_and_is_dropped_when_the_tab_closes() {
    let dir = sample_folder("loading-switch");
    let (first, second) = (dir.join("first.txt"), dir.join("second.txt"));
    std::fs::write(&first, numbered(50)).unwrap();
    std::fs::write(&second, numbered(50)).unwrap();
    let mut app = app_with(&first, false);
    let other = app.editor.active.unwrap();

    // 切换标签后读完：内容换上，当前标签不受影响；跳转留到再次切到该标签时执行
    // （例如 `winc a.rs:10 b.rs:20` 中 `a.rs` 的跳转）。
    let load = open_unloaded(&mut app, &second);
    app.jump_to(30, 0, 0);
    app.activate_tab(other);
    deliver(&mut app, load);
    let index = tab_of(&app, &second).unwrap();
    assert!(!app.editor.documents[index].is_pending());
    assert_eq!(app.editor.documents[index].selection.head, 0);
    assert_eq!(app.editor.active, Some(other));
    assert_eq!(app.editor.active().unwrap().selection.head, 0);
    assert_eq!(app.disk.loads.then.len(), 1);
    app.activate_tab(index);
    app.run_when_loaded();
    let doc = app.editor.active().unwrap();
    assert_eq!(doc.text().pos_to_line_col(doc.selection.head), (30, 0));
    assert!(app.disk.loads.then.is_empty());

    // 关掉以后读好：什么也不做。
    app.close_tab(index);
    let third = dir.join("third.txt");
    std::fs::write(&third, numbered(50)).unwrap();
    let load = open_unloaded(&mut app, &third);
    app.jump_to(30, 0, 0);
    let index = app.editor.active.unwrap();
    app.close_tab(index);
    deliver(&mut app, load);
    assert!(tab_of(&app, &third).is_none());
    assert!(app.disk.loads.then.is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn changes_on_disk_while_reading_are_picked_up() {
    let dir = sample_folder("loading-changed");
    let dir = winc_config::files::canonicalize(&dir).unwrap();
    let path = dir.join("file.txt");
    std::fs::write(&path, "before\n").unwrap();
    let mut app = app_with(&dir, false);

    // 读到的是旧内容，交回之前文件又被改了：换上以后再对一次磁盘，换成新的。
    let (id, ticket, job) = open_unloaded(&mut app, &path);
    let read = job.run();
    std::fs::write(&path, "after, and longer\n").unwrap();
    let file = std::fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(SystemTime::now() + Duration::from_secs(1))
        .unwrap();
    app.disk.loads.results.0.send((id, ticket, read)).unwrap();
    app.poll_loads();
    let text = app
        .editor
        .active()
        .unwrap()
        .text()
        .rope()
        .unwrap()
        .to_string();
    assert_eq!(text, "after, and longer\n");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_slow_read_shows_what_is_being_read() {
    let dir = sample_folder("loading-slow");
    let path = dir.join("slow.txt");
    std::fs::write(&path, "x\n").unwrap();
    let mut app = app_with(&dir, false);

    // 超过留帧的时间还没读好：画出「正在读取」，不画空白的编辑区。
    let load = open_unloaded(&mut app, &path);
    app.disk.loads.since = Some((load.0, Instant::now() - Duration::from_secs(1)));
    assert!(app.holding_frame(Instant::now()).is_none());
    shoot(&mut app, "loading-slow");
    assert!(app.hits.placeholder.is_empty());

    // 知道读了多少时带上百分数和进度条；读到新的位置才重画。
    let id = load.0;
    let progress = load.2.progress();
    app.disk.loads.progress.insert(id, progress.clone());
    progress.begin(200);
    progress.add(84);
    assert_eq!(app.reading_progress(id), Some(0.42));
    let now = Instant::now();
    app.frame.dirty = false;
    assert_eq!(app.reading_bar(id), Some(0.0));
    assert!(app.reading_tick(now).is_some());
    assert!(app.frame.dirty);
    // 测试中没有窗口，不播放过渡：进度条直接到位。
    assert_eq!(app.reading_bar(id), Some(0.42));
    app.frame.dirty = false;
    app.reading_tick(now);
    assert!(!app.frame.dirty);
    shoot(&mut app, "loading-progress");

    deliver(&mut app, load);
    assert!(!app.editor.active().unwrap().is_pending());
    assert_eq!(app.reading_progress(id), None);
    assert!(app.disk.loads.bar.is_none());
    assert_eq!(app.reading_tick(Instant::now()), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn switching_small_markdown_notes_renders_properties_on_first_frame_without_blank_gap() {
    use winc_editor::{PreviewKind, PreviewMode};
    use winc_preview::markdown::properties::PROPERTY;

    let dir = sample_folder("loading-md-atomic-small");
    let first = dir.join("first.md");
    let second = dir.join("second.md");
    let note_a = "---\ncreated: 2026-11-15\nwhy: 第一篇笔记\n---\n\n# 第一篇\n\n正文内容 A。\n";
    let note_b = "---\ncreated: 2026-11-16\nwhy: 第二篇笔记\n---\n\n# 第二篇\n\n正文内容 B。\n";
    std::fs::write(&first, note_a).unwrap();
    std::fs::write(&second, note_b).unwrap();

    let mut app = app_with(&first, false);
    app.editor
        .set_preview_default(PreviewKind::Markdown, PreviewMode::Preview);
    app.set_preview_mode(PreviewMode::Preview);
    paint(&mut app);
    let first_idx = app.editor.active.unwrap();
    let first_id = app.editor.active().unwrap().id;
    assert!(app.preview.markdown.parsed.is_some());

    // 异步读取第二篇普通笔记：读盘期间按住上一帧；读完后立即在首帧同步完成解析，不留空白帧。
    let load = open_unloaded(&mut app, &second);
    let second_id = load.0;
    assert!(app.holding_frame(Instant::now()).is_some());

    deliver(&mut app, load);
    assert_eq!(app.preview.markdown.doc, Some(second_id));
    assert!(app.preview.markdown.parsed.is_some());
    assert!(!app.preview.markdown.is_busy());
    assert!(app.holding_frame(Instant::now()).is_none());

    // 不等待任何后台任务，直接绘制首帧即包含属性卡片与正文排版。
    paint(&mut app);
    assert!(app.preview.markdown.layout.is_some());
    assert!(app.preview.markdown.height > 0.0);
    assert!(
        app.hits
            .markdown_links
            .iter()
            .any(|(_, url)| url.starts_with(PROPERTY)),
        "首帧应当直接绘制属性卡片而非空白帧"
    );

    // 在已加载的多标签之间切换（含分栏预览）同样首帧同步就绪，不闪白。
    app.activate_tab(first_idx);
    assert_eq!(app.preview.markdown.doc, Some(first_id));
    assert!(app.preview.markdown.parsed.is_some());
    assert!(!app.preview.markdown.is_busy());
    assert!(app.holding_frame(Instant::now()).is_none());

    let second_idx = tab_of(&app, &second).unwrap();
    app.editor.documents[second_idx].set_preview_mode(PreviewMode::Split);
    app.activate_tab(second_idx);
    assert_eq!(app.preview.markdown.doc, Some(second_id));
    assert!(app.preview.markdown.parsed.is_some());
    assert!(!app.preview.markdown.is_busy());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn large_markdown_holds_previous_frame_across_first_async_parse() {
    use crate::app::preview::markdown::INLINE_FIRST_PARSE_BYTES;
    use winc_editor::{PreviewKind, PreviewMode};

    let dir = sample_folder("loading-md-hold-large");
    let first = dir.join("small.md");
    let big = dir.join("large.md");
    std::fs::write(&first, "---\ncreated: 2026-11-15\n---\n\n短笔记。\n").unwrap();
    let mut big_text = String::from("---\ncreated: 2026-11-16\nwhy: 超大文档\n---\n\n");
    while big_text.len() <= INLINE_FIRST_PARSE_BYTES + 4096 {
        big_text.push_str("## 章节标题\n\n这是一段用于测试超大 Markdown 文档异步解析留帧的常规正文段落。\n\n");
    }
    std::fs::write(&big, &big_text).unwrap();

    let mut app = app_with(&first, false);
    app.editor
        .set_preview_default(PreviewKind::Markdown, PreviewMode::Preview);
    app.set_preview_mode(PreviewMode::Preview);
    paint(&mut app);
    let first_idx = app.editor.active.unwrap();

    // 大文档读盘结束后自动衔接后台首次解析，并沿用读盘起点的时间预算保留上一帧。
    let load = open_unloaded(&mut app, &big);
    let big_id = load.0;
    let t0 = Instant::now() - Duration::from_millis(20);
    app.disk.loads.since = Some((big_id, t0));

    deliver(&mut app, load);
    assert_eq!(app.preview.markdown.doc, Some(big_id));
    assert!(app.preview.markdown.parsed.is_none());
    assert_eq!(app.preview.markdown.first_parse_since, Some((big_id, t0)));
    assert!(app.holding_frame(t0 + Duration::from_millis(50)).is_some());
    assert!(app.holding_frame(t0 + Duration::from_millis(120)).is_none());

    // 打开设置时不应被后台解析留帧卡住。
    app.settings.open = true;
    assert!(app.holding_frame(t0 + Duration::from_millis(50)).is_none());
    app.settings.open = false;

    // 首次解析完成后立即释放留帧状态。
    app.wait_for_markdown();
    assert!(app.preview.markdown.parsed.is_some());
    assert!(app.preview.markdown.first_parse_since.is_none());
    assert!(app.holding_frame(t0 + Duration::from_millis(50)).is_none());

    // 切走再切回已加载的大文档标签，同样触发后台解析留帧并在完成后释放。
    let big_idx = tab_of(&app, &big).unwrap();
    app.activate_tab(first_idx);
    assert!(app.preview.markdown.parsed.is_some());
    assert!(app.holding_frame(Instant::now()).is_none());

    app.activate_tab(big_idx);
    assert_eq!(app.preview.markdown.doc, Some(big_id));
    assert!(app.preview.markdown.parsed.is_none());
    assert!(app.preview.markdown.first_parse_since.is_some());
    assert!(app.holding_frame(Instant::now()).is_some());
    app.wait_for_markdown();
    assert!(app.preview.markdown.parsed.is_some());
    assert!(app.holding_frame(Instant::now()).is_none());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn writing_mode_preserves_scroll_property_fold_and_recent_images_across_note_switches() {
    use crate::app::preview::markdown::ImageSlot;
    use winc_editor::{PreviewKind, PreviewMode};

    // 1x1 合法 PNG，用于验证本地插图在切走再切回后首帧直接复用。
    const PNG_1X1: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8,
        0xCF, 0xC0, 0xF0, 0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    let dir = sample_folder("loading-md-preserve-state");
    let pic = dir.join("pic.png");
    std::fs::write(&pic, PNG_1X1).unwrap();
    let note_a = dir.join("a.md");
    let note_b = dir.join("b.md");
    let mut body_a = String::from("---\ncreated: 2026-11-15\nwhy: 保留状态\n---\n\n");
    for i in 0..5 {
        body_a.push_str(&format!("## 前文 {i}\n\n一段用于把图片放在滚后视口内的正文。\n\n"));
    }
    body_a.push_str("![图](pic.png)\n\n");
    for i in 0..35 {
        body_a.push_str(&format!("## 节 {i}\n\n一段足够长以产生滚动距离的测试正文。\n\n"));
    }
    std::fs::write(&note_a, &body_a).unwrap();
    std::fs::write(&note_b, "---\ncreated: 2026-11-16\n---\n\n第二篇笔记。\n").unwrap();

    let mut app = app_with(&note_a, false);
    app.editor
        .set_preview_default(PreviewKind::Markdown, PreviewMode::Preview);
    app.set_preview_mode(PreviewMode::Preview);
    for _ in 0..3 {
        app.wait_for_markdown();
        paint(&mut app);
    }

    // 滚动到图片所在视口并折叠属性面板，再模拟写作模式关闭旧文档并切去另一篇笔记。
    app.preview.markdown.scroll_to(180.0);
    app.toggle_property_panel();
    for _ in 0..3 {
        app.wait_for_markdown();
        paint(&mut app);
    }
    let canon_pic = winc_config::files::canonicalize(&pic).unwrap();
    assert!(matches!(
        app.preview.markdown.images.get(&canon_pic),
        Some(ImageSlot::Ready { pixels, .. }) if !pixels.is_empty()
    ));
    let old_a_id = app.editor.active().unwrap().id;
    assert!(app.preview.markdown.property_collapsed.contains(&old_a_id));

    app.close_documents(&[old_a_id]);
    app.open_path(&note_b);
    app.wait_for_markdown();
    paint(&mut app);
    let b_id = app.editor.active().unwrap().id;

    // 切回 a.md（分配了全新的 DocumentId）：滚动位置、属性折叠态与本地图片均首帧保留。
    app.close_documents(&[b_id]);
    app.open_path(&note_a);
    let new_a_id = app.editor.active().unwrap().id;
    assert_ne!(new_a_id, old_a_id);
    assert!((app.preview.markdown.scroll() - 180.0).abs() < 1.0);
    assert!(app.preview.markdown.property_collapsed.contains(&new_a_id));
    assert!(matches!(
        app.preview.markdown.images.get(&canon_pic),
        Some(ImageSlot::Ready { pixels, .. }) if !pixels.is_empty()
    ));
    assert!(!app.preview.markdown.is_busy());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn json_and_csv_parse_inline_for_small_files_and_hold_frame_for_large_files() {
    use winc_editor::{PreviewKind, PreviewMode, TableState};

    let dir = sample_folder("loading-other-previews");
    let small_json = dir.join("small.json");
    let big_json = dir.join("big.json");
    let small_csv = dir.join("small.csv");
    std::fs::write(&small_json, r#"{"name":"minc","items":[1,2,3]}"#).unwrap();
    let mut big = String::from("{\"rows\":[");
    for i in 0..2500 {
        if i > 0 {
            big.push(',');
        }
        big.push_str(&format!("{{\"id\":{i},\"label\":\"item-{i:04}-padding\"}}"));
    }
    big.push_str("]}");
    assert!(big.len() > crate::app::preview::json::INLINE_FIRST_JSON_BYTES);
    std::fs::write(&big_json, &big).unwrap();
    std::fs::write(&small_csv, "name,score\nalice,95\nbob,88\n").unwrap();

    let mut app = app_with(&dir, false);
    app.editor
        .set_preview_default(PreviewKind::Json, PreviewMode::Preview);
    app.editor
        .set_preview_default(PreviewKind::Table, PreviewMode::Preview);

    // 小 JSON：读盘完成后首帧直接同步解析，不闪“正在加载…”。
    let load = open_unloaded(&mut app, &small_json);
    deliver(&mut app, load);
    assert!(app.preview.json.graph.is_some());
    assert!(app.holding_frame(Instant::now()).is_none());

    // 大 JSON：读盘完成后转后台解析，并沿用读盘起点继续保留上一帧。
    let load = open_unloaded(&mut app, &big_json);
    let big_id = load.0;
    let t0 = Instant::now() - Duration::from_millis(20);
    app.disk.loads.since = Some((big_id, t0));
    deliver(&mut app, load);
    assert!(app.preview.json.graph.is_none());
    assert_eq!(app.preview.json.first_parse_since, Some((big_id, t0)));
    assert!(app.holding_frame(t0 + Duration::from_millis(50)).is_some());
    assert!(app.holding_frame(t0 + Duration::from_millis(120)).is_none());

    // 小 CSV：读盘完成后首帧直接同步建表，不闪空白。
    let load = open_unloaded(&mut app, &small_csv);
    deliver(&mut app, load);
    assert!(matches!(
        app.editor
            .active()
            .and_then(|d| d.table.as_deref())
            .map(|v| &v.state),
        Some(TableState::Ready(_))
    ));
    assert!(app.holding_frame(Instant::now()).is_none());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn indexed_note_exists_resolves_in_library_markdown_without_disk_stat() {
    let dir = sample_folder("loading-indexed-links");
    let root = winc_config::files::canonicalize(&dir).unwrap();
    std::fs::write(root.join("a.md"), "# A\n\n[B](b.md)\n").unwrap();
    std::fs::write(root.join("b.md"), "# B\n").unwrap();

    let mut app = app_with(&root.join("a.md"), false);
    app.ensure_note_links();
    app.poll_note_links();
    assert_eq!(app.indexed_note_exists(&root.join("b.md")), Some(true));
    // 未在索引中命中的路径、非 .md 附件与隐藏目录回退给文件系统确认，避免新文件误报断链。
    assert_eq!(app.indexed_note_exists(&root.join("missing.md")), None);
    assert_eq!(app.indexed_note_exists(&root.join("pic.png")), None);
    assert_eq!(app.indexed_note_exists(&root.join(".hidden/x.md")), None);

    let _ = std::fs::remove_dir_all(&dir);
}
