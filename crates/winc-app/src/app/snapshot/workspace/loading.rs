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
