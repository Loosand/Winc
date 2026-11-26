//! 笔记库的磁盘、卡片点击、滚动与保留编辑现场的集成测试。

use super::*;
use crate::app::scroll::ScrollTarget;
use crate::app::workspace::library::{Card, CardContent, Grid, LibraryTarget, PreviewLine};
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
fn home_updates_when_files_change_on_disk() {
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

    let notes = root.join("new.md");
    std::fs::write(&notes, "# New note").unwrap();
    app.apply_changes(Changes {
        paths: vec![notes.clone()],
        ..Changes::default()
    });
    paint(&mut app);
    assert!(app.library.cards.iter().any(|c| c.path == notes));
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
        folder_card.front.as_ref().map(|card| card.name.as_str()),
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
    let home = app.library_home_button();
    press(&mut app, home);
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
fn notebook_main_area_browses_all_visible_files_and_follows_the_sidebar() {
    let (mut app, root) = note_app("notebook-folder-navigation");
    let recipes = root.join("Recipes");
    let drafts = recipes.join("Drafts");
    let tea = recipes.join("茶.md");
    let config = root.join("config.txt");
    let asset = recipes.join("attachment.bin");
    std::fs::create_dir(&drafts).unwrap();
    std::fs::write(&asset, [0, 1, 2]).unwrap();
    app.apply_changes(Changes {
        paths: vec![drafts.clone(), asset.clone()],
        ..Changes::default()
    });
    paint(&mut app);

    let paths = |app: &App| {
        app.library
            .cards
            .iter()
            .map(|card| card.path.clone())
            .collect::<Vec<_>>()
    };
    assert!(app.library_shown());
    assert_eq!(app.library.folder.as_ref(), Some(&root));
    assert_eq!(
        paths(&app),
        [recipes.clone(), config.clone(), root.join("薄荷.md")]
    );
    assert!(matches!(app.library.cards[1].content, CardContent::File));
    assert!(
        matches!(&app.library.cards[0].content, CardContent::Folder(Some(info)) if info.count == 3)
    );

    let config_card = hit(&app, LibraryTarget::Card(1));
    press(&mut app, config_card);
    assert_eq!(app.editor.active().unwrap().path.as_ref(), Some(&config));
    app.run(Command::Insert("saved when browsing\n".into()));

    let row = app
        .editor
        .workspace
        .as_ref()
        .unwrap()
        .row_of(&recipes)
        .unwrap();
    let at = tree_row_center(&app, row);
    click(&mut app, at);
    paint(&mut app);
    assert_eq!(app.focus, Focus::Tree);
    assert!(app.library_shown());
    assert_eq!(app.library.folder.as_ref(), Some(&recipes));
    assert_eq!(paths(&app), [drafts.clone(), asset, tea]);
    assert!(
        std::fs::read_to_string(&config)
            .unwrap()
            .contains("saved when browsing")
    );

    let subfolder = hit(&app, LibraryTarget::Card(0));
    press(&mut app, subfolder);
    assert_eq!(app.library.folder.as_ref(), Some(&drafts));
    assert!(app.library.cards.is_empty());
    let row = app
        .editor
        .workspace
        .as_ref()
        .unwrap()
        .row_of(&recipes)
        .unwrap();
    let at = tree_row_center(&app, row);
    click(&mut app, at);
    paint(&mut app);
    assert_eq!(app.library.folder.as_ref(), Some(&recipes));

    let menu = app.panels.sidebar.navigation.folder_menu;
    press(&mut app, menu);
    app.run_menu_command(MenuCommand::SwitchNotebookWorkspace(0));
    paint(&mut app);
    assert_eq!(app.library.folder.as_ref(), Some(&root));
    assert!(app.editor.workspace.as_ref().unwrap().selected.is_none());
    assert_eq!(paths(&app).len(), 3);

    let tools = app.panels.sidebar.navigation.tree_menu;
    press(&mut app, tools);
    app.run_menu_command(MenuCommand::NotebookShowHidden);
    assert!(paths(&app).contains(&root.join(".hidden.md")));
    press(&mut app, tools);
    app.run_menu_command(MenuCommand::NotebookNameOrder(true));
    assert_eq!(
        paths(&app),
        [
            recipes,
            root.join("薄荷.md"),
            config,
            root.join(".hidden.md")
        ]
    );
    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
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

const WEBP: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../winc-preview/testdata/icon-16.webp"
));
const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 80 40"><rect width="80" height="40" rx="8" fill="teal"/><circle cx="24" cy="20" r="10" fill="white"/></svg>"#;

#[test]
fn library_previews_images_svg_and_json_inside_and_outside_folders() {
    use crate::app::workspace::library::{AttachmentKind, AttachmentPreview, JsonExcerpt};
    use winc_preview::json::Value;

    let dir = sample_folder("library-attachments");
    let root = winc_config::files::canonicalize(&dir).unwrap();
    let images = root.join("Images");
    let data = root.join("Data");
    let webp = images.join("cover.webp");
    let json = data.join("manifest.json");
    let svg = root.join("banner.SVG");
    let broken = root.join("broken.gif");
    std::fs::create_dir(&images).unwrap();
    std::fs::create_dir(&data).unwrap();
    std::fs::write(&webp, WEBP).unwrap();
    std::fs::write(&svg, SVG).unwrap();
    std::fs::write(&json, r#"{"Title":"Library","items":[1,2],"ready":true}"#).unwrap();
    std::fs::write(&broken, b"corrupt").unwrap();

    let mut app = blank_app(false);
    app.panels.sidebar.legacy_chrome = false;
    app.open_path(&root);
    paint(&mut app);
    for (index, path, kind) in [
        (0, &json, AttachmentKind::Json),
        (1, &webp, AttachmentKind::Raster),
    ] {
        let CardContent::Folder(Some(info)) = &app.library.cards[index].content else {
            panic!("expected a folder preview");
        };
        let front = info.front.as_ref().unwrap();
        assert_eq!(&front.path, path);
        assert!(matches!(front.content, CardContent::Attachment(found) if found == kind));
    }
    assert!(
        matches!(app.library.attachment_preview(&webp), Some(AttachmentPreview::Image(p)) if p.width == 16 && p.height == 16)
    );
    assert!(
        matches!(app.library.attachment_preview(&svg), Some(AttachmentPreview::Image(p)) if p.width == 2 * p.height && p.rgba.as_chunks::<4>().0.iter().any(|pixel| pixel[3] > 0))
    );
    assert!(app.library.attachment_preview(&broken).is_none());

    let data_card = hit(&app, LibraryTarget::Card(0));
    press(&mut app, data_card);
    let Some(AttachmentPreview::Json(JsonExcerpt::Fields { rows, .. })) =
        app.library.attachment_preview(&json)
    else {
        panic!("expected a structured JSON card");
    };
    assert_eq!(rows[0].key, "Title");
    assert_eq!(rows[0].value, Value::String("\"Library\"".into()));
    assert_eq!(rows[1].value, Value::Array(2));
    assert_eq!(rows[2].value, Value::Literal("true"));

    std::fs::write(&json, r#"{"count":42}"#).unwrap();
    app.apply_changes(Changes {
        paths: vec![json.clone()],
        ..Changes::default()
    });
    paint(&mut app);
    assert!(
        matches!(app.library.attachment_preview(&json), Some(AttachmentPreview::Json(JsonExcerpt::Fields { rows, .. })) if rows[0].key == "count" && rows[0].value == Value::Number("42".into()))
    );

    app.browse_library();
    paint(&mut app);
    let images_card = hit(&app, LibraryTarget::Card(1));
    press(&mut app, images_card);
    assert!(matches!(
        app.library.attachment_preview(&webp),
        Some(AttachmentPreview::Image(_))
    ));
    assert!(app.library.attachment_preview(&svg).is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
#[ignore = "manual library attachment snapshots"]
fn shoot_library_attachments() {
    let dir = sample_folder("library-attachment-shots");
    let root = dir.join("数字花园");
    let images = root.join("图像附件");
    std::fs::create_dir_all(&images).unwrap();
    std::fs::write(images.join("01.webp"), WEBP).unwrap();
    std::fs::write(root.join("示意图.svg"), SVG).unwrap();
    std::fs::write(root.join("资源清单.json"), r#"{"name":"数字花园","version":2,"published":true,"format":"webp","dimensions":{"width":1280,"height":720},"images":["01.webp","示意图.svg"]}"#).unwrap();
    let mut app = blank_app(false);
    app.panels.sidebar.legacy_chrome = false;
    app.restore_panel_width();
    app.open_path(&root);
    shoot(&mut app, "library-attachments");
    for i in 1..=24 {
        let name = format!("图像备注 {i:02}");
        std::fs::write(
            images.join(format!("{name}.md")),
            format!("# {name}\n\n记录构图、色彩和导出设置。\n"),
        )
        .unwrap();
    }
    let folder = hit(&app, LibraryTarget::Card(0));
    press(&mut app, folder);
    shoot(&mut app, "library-folder-contents");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn library_opens_with_complete_previews_and_keeps_them_while_scrolling_and_refreshing() {
    const NOTE_COUNT: usize = 49;
    let dir = sample_folder("library-scroll");
    let notes = dir.join("笔记库");
    std::fs::create_dir(&notes).unwrap();
    let root = winc_config::files::canonicalize(&notes).unwrap();
    for i in 0..NOTE_COUNT {
        std::fs::write(root.join(format!("笔记{i:02}.md")), format!("第 {i} 篇\n")).unwrap();
    }
    let image = root.join("笔记00.webp");
    std::fs::write(&image, WEBP).unwrap();
    let mut app = app_with(&root, false);

    // Inspect the first published cards before a paint can trigger any more work.
    let has_excerpt = |card: &Card, expected: &str| {
        matches!(&card.content, CardContent::Note(lines)
            if matches!(lines.as_slice(), [PreviewLine::Text(text)] if text == expected))
    };
    assert!(has_excerpt(&app.library.cards[0], "第 0 篇"));
    assert_eq!(app.library.cards.len(), NOTE_COUNT + 1);
    let last = root.join(format!("笔记{:02}.md", NOTE_COUNT - 1));
    assert_eq!(app.library.cards.last().unwrap().path, last);
    assert!(has_excerpt(app.library.cards.last().unwrap(), "第 48 篇"));
    assert!(app.library.attachment_preview(&image).is_some());

    paint(&mut app);
    let grid = Grid::new(app.layout.editor, &app.library.cards);
    assert!(grid.cols > 1);
    assert!(app.hits.library.len() < NOTE_COUNT);
    let origin = (grid.body.x + 10.0, grid.body.y + 10.0);
    let target = app.scroll_target_at(origin.0, origin.1);
    assert_eq!(target, ScrollTarget::Library);
    let bottom = app.pane_max(target);
    app.scroll_target_by(target, origin, false, (0.0, -100_000.0));
    paint(&mut app);
    assert_eq!(app.library.scroll, bottom);
    assert_eq!(app.library.cards.len(), NOTE_COUNT + 1);
    assert!(hit(&app, LibraryTarget::Card(NOTE_COUNT)).h > 0.0);

    std::fs::write(&last, "改过的摘要。\n").unwrap();
    app.apply_changes(Changes {
        paths: vec![last.clone()],
        ..Changes::default()
    });
    assert_eq!(app.library.scroll, bottom);
    assert!(has_excerpt(
        app.library.cards.last().unwrap(),
        "改过的摘要。"
    ));
    assert!(has_excerpt(&app.library.cards[0], "第 0 篇"));
    assert!(app.library.attachment_preview(&image).is_some());

    std::fs::remove_file(&last).unwrap();
    app.apply_changes(Changes {
        paths: vec![last],
        ..Changes::default()
    });
    assert_eq!(app.library.cards.len(), NOTE_COUNT);
    assert!(app.library.scroll <= app.pane_max(target));
    app.scroll_target_by(target, origin, false, (0.0, 100_000.0));
    assert_eq!(app.library.scroll, 0.0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn switching_expanded_folders_keeps_them_open_and_current_click_preserves_the_grid() {
    const NOTE_COUNT: usize = 25;
    let dir = sample_folder("library-tree-switch");
    let root = winc_config::files::canonicalize(&dir).unwrap();
    let a = root.join("A");
    let b = root.join("B");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    for i in 0..NOTE_COUNT {
        std::fs::write(a.join(format!("{i:02}.md")), "An A note").unwrap();
    }
    std::fs::write(b.join("note.md"), "A B note").unwrap();

    let mut app = blank_app(false);
    app.panels.sidebar.legacy_chrome = false;
    app.open_path(&root);
    paint(&mut app);
    let row = |app: &App, folder: &Path| {
        app.editor
            .workspace
            .as_ref()
            .unwrap()
            .row_of(folder)
            .unwrap()
    };
    let expanded = |app: &App, folder: &Path| {
        app.editor.workspace.as_ref().unwrap().rows()[row(app, folder)].expanded
    };
    let click_folder = |app: &mut App, folder: &Path| {
        let index = row(app, folder);
        app.scroll_tree_to(index, false);
        paint(app);
        let at = tree_row_center(app, index);
        click(app, at);
    };

    for folder in [&a, &b, &a] {
        click_folder(&mut app, folder);
        paint(&mut app);
        assert_eq!(app.library.folder.as_ref(), Some(folder));
        assert!(expanded(&app, folder));
    }
    assert!(expanded(&app, &b));
    assert_eq!(app.library.cards.len(), NOTE_COUNT);
    app.set_pane_scroll(ScrollTarget::Library, app.pane_max(ScrollTarget::Library));
    let scroll = app.library.scroll;
    assert!(scroll > 0.0);

    click_folder(&mut app, &a);
    assert!(!expanded(&app, &a));
    assert!(expanded(&app, &b));
    assert_eq!(app.library.folder.as_ref(), Some(&a));
    assert_eq!(app.library.cards.len(), NOTE_COUNT);
    assert_eq!(app.library.scroll, scroll);

    click_folder(&mut app, &a);
    assert!(expanded(&app, &a));
    assert_eq!(app.library.scroll, scroll);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn pending_note_cards_render_skeletons_and_open_immediately_on_click() {
    let (mut app, root) = note_app("library-progressive-pending");
    let note = root.join("薄荷.md");
    paint(&mut app);

    // 模拟第一阶段已拿到目录项但摘要尚未扫完时的 PendingNote 卡片：可正常绘制且立即可点开。
    let note_index = app
        .library
        .cards
        .iter()
        .position(|c| c.path == note)
        .unwrap();
    app.library.cards[note_index].content = CardContent::PendingNote;
    paint(&mut app);
    let card_rect = hit(&app, LibraryTarget::Card(note_index));
    assert!(card_rect.w > 0.0 && card_rect.h > 0.0);

    press(&mut app, card_rect);
    assert!(!app.library_shown());
    assert_eq!(app.editor.active().unwrap().path.as_ref(), Some(&note));
    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
}
