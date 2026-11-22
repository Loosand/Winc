//! 笔记库的磁盘、卡片点击、滚动与保留编辑现场的集成测试。

use super::*;
use crate::app::scroll::ScrollTarget;
use crate::app::workspace::library::{CardContent, Grid, LibraryTarget, PreviewLine};
use crate::theme::TREE_ROW;
use winc_editor::Changes;

fn hit(app: &App, target: LibraryTarget) -> Rect {
    app.hits
        .library
        .iter()
        .find(|(_, t)| *t == target)
        .unwrap()
        .0
}

fn press(app: &mut App, rect: Rect) {
    click(app, (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0));
    paint(app);
}

#[test]
fn home_shows_current_folders_files_and_holds_refresh_feedback() {
    use std::time::{Duration, Instant};
    let dir = sample_folder("home-refresh");
    let root = winc_config::files::canonicalize(&dir).unwrap();
    let recent = root.join("recent.md");
    let favorite = root.join("favorite.md");
    std::fs::write(&recent, "# Recent note").unwrap();
    std::fs::write(&favorite, "# Favorite note").unwrap();
    let mut app = blank_app(false);
    let state = root.join("state.json");
    app.store.state_path = Some(state.clone());
    app.open_path(&root);
    app.update_state(|state| {
        state.recent_files = vec![recent.clone()];
        state.note_favorites = vec![favorite.clone()];
    });
    paint(&mut app);
    assert!(app.library.cards.iter().any(|c| c.path == recent));
    assert!(app.library.cards.iter().any(|c| c.path == favorite));

    let at = hit(&app, LibraryTarget::Refresh);
    let start = Instant::now();
    press(&mut app, at);
    let until = app.library.manual_refresh.unwrap();
    assert!(until >= start + Duration::from_millis(500));
    assert_eq!(
        app.library_refresh_tick(until - Duration::from_millis(1)),
        Some(until)
    );
    assert!(app.notice.text.is_none());
    app.library_refresh_tick(until);
    assert_eq!(
        app.notice.text.as_deref(),
        Some(winc_i18n::t!("workspace-library-refreshed"))
    );
    assert!(app.library.manual_refresh.is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
#[ignore = "offscreen folder grid benchmark"]
fn measure_library_folder_grid() {
    let dir = sample_folder("library-grid-bench");
    let root = dir.join("folders");
    let notes = dir.join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    let text = "# 写作计划\n\n这里是文档开头。悬停时纸页会从文件夹里升起。\n\n## 这周\n\n- 写一篇文章\n- 整理旧笔记\n";
    for i in 0..24 {
        let folder = root.join(format!("文件夹 {i:02}"));
        std::fs::create_dir_all(&folder).unwrap();
        for n in 0..3 {
            std::fs::write(folder.join(format!("笔记 {i:02}-{n}.md")), text).unwrap();
        }
        std::fs::write(notes.join(format!("笔记 {i:02}.md")), text).unwrap();
    }
    for (name, path) in [("notes", notes.as_path()), ("folders", root.as_path())] {
        let mut app = app_with(path, true);
        paint(&mut app);
        let visible = app
            .hits
            .library
            .iter()
            .filter(|(_, target)| matches!(target, LibraryTarget::Card(_)))
            .count();
        let measure = |app: &mut App, state, animated| {
            let mut frames = Vec::new();
            for i in 0..18 {
                if animated {
                    app.library.freeze_folder_motion(0, true, 25 + i * 11);
                }
                // Fractional logical offsets stress Retina pixel alignment and match trackpad scrolling.
                app.library.scroll = if state == "scroll" {
                    i as f32 * 6.35
                } else {
                    0.0
                };
                let start = std::time::Instant::now();
                paint(app);
                frames.push(start.elapsed());
            }
            frames.sort_unstable();
            eprintln!(
                "{name:7} {state:7} ({visible} cards): median {:?}, p95 {:?} per two offscreen frames",
                frames[9], frames[17]
            );
        };
        measure(&mut app, "still", false);
        measure(&mut app, "scroll", false);
        if name == "folders" {
            measure(&mut app, "hover", true);
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
#[ignore = "截图，手动运行"]
fn shoot_library_folder_motion() {
    let dir = sample_folder("library-motion");
    let root = dir.join("数字花园");
    let folder = root.join("博客");
    std::fs::create_dir_all(&folder).unwrap();
    for title in ["01 写作计划", "02 读书笔记", "03 随笔"] {
        std::fs::write(folder.join(format!("{title}.md")), format!("# {title}\n\n这里是文档开头。悬停时纸页会从文件夹里升起。\n\n## 这周\n\n- 写一篇文章\n- 整理旧笔记\n")).unwrap();
    }
    std::fs::write(
        root.join("写作方法.md"),
        "# 写作方法\n\n给自己留几篇好的参考。\n",
    )
    .unwrap();
    let mut app = app_with(&root, false);
    paint(&mut app);
    let card = hit(&app, LibraryTarget::Card(0));
    for (n, (open, time)) in [true, false]
        .into_iter()
        .flat_map(|open| (0..=10).map(move |i| (open, i * 38)))
        .enumerate()
    {
        let y = if open {
            card.y + card.h / 2.0
        } else {
            card.y - 12.0
        };
        app.on_mouse_move(card.x + card.w / 2.0, y);
        app.library.freeze_folder_motion(0, open, time);
        paint(&mut app);
        shoot(&mut app, &format!("folder-motion-{n:02}"));
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn library_browses_real_folders_and_returns_to_the_same_notes() {
    let dir = sample_folder("library-browse");
    let notes = dir.join("笔记库");
    std::fs::create_dir(&notes).unwrap();
    let root = winc_config::files::canonicalize(&notes).unwrap();
    let folder = root.join("博客");
    let note = folder.join("随笔.md");
    std::fs::create_dir_all(&folder).unwrap();
    let source = "---\ntags: [日记]\n---\n\n# 随笔\n\n第一段。\n下一行。\n\n第二段。\n";
    std::fs::write(&note, source).unwrap();
    std::fs::write(root.join(".private.md"), "hidden").unwrap();
    std::fs::write(root.join("说明.MD"), "这是根目录的说明。").unwrap();

    let mut app = blank_app(false);
    App::apply_preview_state(&mut app.editor, &app.state);
    app.open_path(&root);
    paint(&mut app);
    assert!(app.library_shown());
    assert_eq!(app.library.folder.as_deref(), Some(root.as_path()));
    assert!(app.editor.documents.is_empty());
    assert!(app.library_home_button().bottom() <= app.layout.tree.y);
    assert_eq!(app.library.cards.len(), 2);
    assert_eq!(app.library.cards[0].name, "博客");
    let CardContent::Folder(Some(folder_card)) = &app.library.cards[0].content else {
        panic!("缺少文件夹预览")
    };
    assert_eq!((folder_card.count, folder_card.papers), (1, 1));
    assert_eq!(
        folder_card.front.as_ref().map(|(name, _)| name.as_str()),
        Some("随笔")
    );
    assert_eq!(app.library.cards[1].name, "说明");
    assert!(matches!(app.library.cards[1].content, CardContent::Note(_)));

    let folder_card = hit(&app, LibraryTarget::Card(0));
    let paper = hit(&app, LibraryTarget::Card(1));
    assert!(folder_card.y < paper.y && (folder_card.bottom() - paper.bottom()).abs() < 1.0);
    assert!((paper.h / paper.w - 1.2).abs() < 0.01);
    press(&mut app, folder_card);
    assert_eq!(
        app.editor.root(),
        Some(root.as_path()),
        "浏览不会切换笔记库"
    );
    assert_eq!(app.library.folder.as_deref(), Some(folder.as_path()));
    let CardContent::Note(lines) = &app.library.cards[0].content else {
        panic!("缺少笔记预览")
    };
    assert_eq!(
        lines.as_slice(),
        [
            PreviewLine::Heading("随笔".into()),
            PreviewLine::Text("第一段。下一行。".into()),
            PreviewLine::Text("第二段。".into())
        ]
    );
    assert_eq!(app.library.cards.len(), 1);
    let parent = hit(&app, LibraryTarget::Parent);
    press(&mut app, parent);
    assert_eq!(app.library.folder.as_deref(), Some(root.as_path()));

    // 在主页使用现有的文件树浏览文件夹，仍能展开和定位原目录。
    let row = row_of(&app, "博客");
    let tree = app.layout.tree;
    press(
        &mut app,
        Rect::new(
            tree.x + 10.0,
            tree.y + row as f32 * TREE_ROW,
            80.0,
            TREE_ROW,
        ),
    );
    assert_eq!(app.library.folder.as_deref(), Some(folder.as_path()));
    let note_card = hit(&app, LibraryTarget::Card(0));
    press(&mut app, note_card);
    assert!(app.markdown_live());
    let doc = app.editor.active().unwrap();
    assert_eq!(doc.path.as_ref(), Some(&note));
    let id = doc.id;
    app.run(Command::Insert("未保存".into()));
    assert!(app.editor.active().unwrap().is_dirty());

    let home = app.library_home_button();
    press(&mut app, home);
    assert!(app.library_shown());
    assert_eq!(app.library.folder.as_deref(), Some(root.as_path()));
    assert_eq!(app.editor.tab_count(), 2);
    assert!(app.editor.document(id).unwrap().is_dirty());
    press(&mut app, home);
    assert_eq!(app.editor.tab_count(), 2, "重复点击主页不重复开标签");
    let index = app.editor.index_of(id).unwrap();
    let tab = app.hits.tabs.iter().find(|h| h.index == index).unwrap().tab;
    press(&mut app, tab);
    assert_eq!(app.editor.active().unwrap().id, id);
    assert!(app.editor.active().unwrap().is_dirty());
    assert_eq!(
        std::fs::read_to_string(note).unwrap(),
        source,
        "浏览不会改写笔记"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn folder_card_controls_open_the_menu_and_create_a_real_note_in_the_folder() {
    let dir = sample_folder("folder-controls");
    let root = dir.join("笔记库");
    let folder = root.join("博客");
    std::fs::create_dir_all(folder.join("深处")).unwrap();
    std::fs::write(folder.join("深处/旧笔记.md"), "里面的稿子").unwrap();
    std::fs::write(folder.join("untitled.md"), "不能覆盖这篇").unwrap();
    let note = folder.join("untitled-2.md");
    let mut app = blank_app(false);
    App::apply_preview_state(&mut app.editor, &app.state);
    app.open_path(&root);
    paint(&mut app);
    let CardContent::Folder(Some(info)) = &app.library.cards[0].content else {
        panic!("缺少文件夹预览")
    };
    assert_eq!((info.count, info.papers), (2, 2));
    assert!(
        !app.hits
            .library
            .iter()
            .any(|(_, t)| *t == LibraryTarget::FolderMenu(0))
    );

    let card = hit(&app, LibraryTarget::Card(0));
    let x = card.x + card.w / 2.0;
    let y = card.y + card.h / 2.0;
    app.on_mouse_move(x, y);
    paint(&mut app);
    let menu = hit(&app, LibraryTarget::FolderMenu(0));
    press(&mut app, menu);
    assert!(
        matches!(&app.overlays.context_menu.as_ref().unwrap().target,
        MenuTarget::Tree(Some(path)) if path.ends_with("博客"))
    );
    app.overlays.context_menu = None;
    app.on_mouse_move(x, y);
    paint(&mut app);
    let plus = hit(&app, LibraryTarget::NewInFolder(0));
    press(&mut app, plus);
    assert_eq!(
        app.editor.active().unwrap().path.as_deref(),
        Some(winc_editor::canonical_path(&note).as_path())
    );
    assert!(app.markdown_live());
    assert_eq!(std::fs::read_to_string(&note).unwrap(), "");
    assert_eq!(
        std::fs::read_to_string(folder.join("untitled.md")).unwrap(),
        "不能覆盖这篇"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn library_scrolls_only_visible_cards_and_refreshes_after_disk_changes() {
    let dir = sample_folder("library-scroll");
    let notes = dir.join("笔记库");
    std::fs::create_dir(&notes).unwrap();
    let root = winc_config::files::canonicalize(&notes).unwrap();
    for i in 0..48 {
        std::fs::write(root.join(format!("笔记{i:02}.md")), format!("第 {i} 篇\n")).unwrap();
    }
    let mut app = app_with(&root, false);
    paint(&mut app);
    let grid = Grid::new(app.layout.editor, &app.library.cards);
    assert!(grid.cols > 1);
    assert!(app.hits.library.len() < 48);
    assert!(hit(&app, LibraryTarget::Card(0)).h > 0.0);
    let target = app.scroll_target_at(grid.body.x + 10.0, grid.body.y + 10.0);
    assert_eq!(target, ScrollTarget::Library);
    app.scroll_target_by(target, (grid.body.x, grid.body.y), false, (0.0, -100_000.0));
    paint(&mut app);
    assert!(app.library.scroll > 0.0);
    assert!(hit(&app, LibraryTarget::Card(47)).h > 0.0);

    let last = root.join("笔记47.md");
    std::fs::write(&last, "改过的摘要。\n").unwrap();
    app.apply_changes(Changes {
        paths: vec![last.clone()],
        ..Changes::default()
    });
    assert!(
        matches!(&app.library.cards.last().unwrap().content, CardContent::Note(lines)
        if matches!(lines.as_slice(), [PreviewLine::Text(text)] if text == "改过的摘要。"))
    );
    std::fs::remove_file(&last).unwrap();
    app.apply_changes(Changes {
        paths: vec![last],
        ..Changes::default()
    });
    paint(&mut app);
    assert_eq!(app.library.cards.len(), 47);
    assert!(app.library.scroll <= app.pane_max(target));
    assert!(
        !app.hits
            .library
            .iter()
            .any(|(_, t)| *t == LibraryTarget::Card(47))
    );
    let _ = std::fs::remove_dir_all(dir);
}
