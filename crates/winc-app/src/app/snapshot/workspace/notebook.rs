use super::*;
use crate::app::OutlineTarget;
use crate::layout::SidebarView;

#[test]
fn notebook_places_icloud_badges_to_the_right_and_marks_an_opening_file() {
    use crate::app::workspace::cloud::CloudStatus;

    let (mut app, root) = note_app("minc-icloud-badges");
    let online = root.join("config.txt");
    let uploading = root.join("薄荷.md");
    app.panels
        .tree
        .cloud
        .set_status(online.clone(), CloudStatus::OnlineOnly);
    app.panels
        .tree
        .cloud
        .set_status(uploading, CloudStatus::Uploading);
    paint(&mut app);
    let badges = &app.hits.cloud_badges;
    assert_eq!(badges.len(), 2);
    assert_eq!(
        [badges[0].1, badges[1].1],
        [CloudStatus::OnlineOnly, CloudStatus::Uploading]
    );
    assert_eq!(badges[0].0.x, badges[1].0.x);
    assert!(badges[0].0.right() < app.layout.tree.right());
    assert!(app.animating());
    app.config.ui_animations = Some(false);
    assert!(!app.animating());
    let badge = badges[0].0;
    app.on_mouse_move(badge.x + badge.w / 2.0, badge.y + badge.h / 2.0);
    assert!(matches!(app.pointer.hover, Hover::CloudBadge(_)));
    assert!(app.pointer.hover.has_tooltip());
    if std::env::var_os("WINC_SNAPSHOT_DIR").is_some() {
        shoot(&mut app, "notebook-icloud-badges");
    }
    app.editor.open_tab(&online);
    assert!(app.editor.active().unwrap().is_pending());
    paint(&mut app);
    assert_eq!(app.hits.cloud_badges[0].1, CloudStatus::Downloading);
    app.config.ui_animations = Some(true);
    app.set_sidebar(false);
    paint(&mut app);
    assert!(app.hits.cloud_badges.is_empty());
    assert!(!app.animating());
    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
}

#[test]
fn notebook_folder_arrow_toggles_without_opening_the_folder() {
    let (mut app, root) = note_app("minc-folder-arrow");
    let folder = root.join("Recipes");
    let note = root.join("薄荷.md");
    let original = std::fs::read_to_string(&note).unwrap();
    app.open_path(&note);
    app.run(Command::Insert("unsaved edit\n".into()));
    paint(&mut app);

    for expanded in [true, false] {
        let row = row_of(&app, "Recipes");
        let (_, y) = tree_row_center(&app, row);
        let x = app.layout.tree.x + 18.0;
        click(&mut app, (x, y));
        paint(&mut app);
        let ws = app.editor.workspace.as_ref().unwrap();
        assert_eq!(ws.rows()[row].expanded, expanded);
        assert_ne!(ws.selected, Some(row));
        assert!(!app.library_shown());
        assert_eq!(app.editor.active().unwrap().path.as_ref(), Some(&note));
        assert!(app.editor.active().unwrap().is_dirty());
    }
    assert_eq!(std::fs::read_to_string(&note).unwrap(), original);

    let at = tree_row_center(&app, row_of(&app, "Recipes"));
    click(&mut app, at);
    assert!(app.library_shown());
    assert_eq!(app.library.folder.as_ref(), Some(&folder));
    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
}

#[test]
fn notebook_toolbar_steps_through_visited_files_and_folders() {
    use crate::app::workspace::library::LibraryTarget;

    let (mut app, root) = note_app("minc-path-history");
    let folder = root.join("Recipes");
    let tea = folder.join("茶.md");
    let mint = root.join("薄荷.md");
    paint(&mut app);
    assert!(!app.can_step_workspace_path(false) && !app.can_step_workspace_path(true));
    let folder_card = app
        .hits
        .library
        .iter()
        .find_map(|&(r, target)| match target {
            LibraryTarget::Card(i) if app.library.cards[i].path == folder => Some(r),
            _ => None,
        })
        .unwrap();
    press(&mut app, folder_card);
    app.open_path(&tea);
    app.open_path(&mint);
    paint(&mut app);
    assert!(app.hits.crumbs.is_empty());
    assert!(app.hits.document_title.w > 0.0);
    let [back, forward] = app.hits.workspace_history;
    let controls = app.caption_controls();
    assert!(controls.contains(&back) && controls.contains(&forward));
    let title = app.hits.document_title;
    right_click(&mut app, (title.x + title.w / 2.0, title.y + title.h / 2.0));
    assert!(matches!(
        app.overlays.context_menu.as_ref().map(|menu| &menu.target),
        Some(MenuTarget::Tab(id)) if *id == app.editor.active().unwrap().id
    ));
    app.dismiss_popup();

    press(&mut app, back);
    assert_eq!(app.editor.active().unwrap().path.as_ref(), Some(&tea));
    press(&mut app, back);
    assert!(app.library_shown());
    assert_eq!(app.library.folder.as_ref(), Some(&folder));
    press(&mut app, back);
    assert_eq!(app.library.folder.as_ref(), Some(&root));
    assert!(!app.can_step_workspace_path(false));
    press(&mut app, back);
    assert_eq!(app.library.folder.as_ref(), Some(&root));
    press(&mut app, forward);
    assert_eq!(app.library.folder.as_ref(), Some(&folder));

    std::fs::remove_file(&tea).unwrap();
    press(&mut app, forward);
    assert_eq!(app.editor.active().unwrap().path.as_ref(), Some(&mint));
    assert!(!app.can_step_workspace_path(true));
    press(&mut app, back);
    assert_eq!(app.library.folder.as_ref(), Some(&folder));
    app.open_path(&root.join("config.txt"));
    assert!(!app.can_step_workspace_path(true));
    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
}

#[test]
fn notebook_sidebar_toggle_stays_beside_the_window_buttons() {
    use crate::app::animation::{Clock, SIDEBAR_ANIMATION};
    use crate::app::workspace::tree::SidebarMotion;
    use crate::theme::{BUTTON_GAP, TAB_GAP};

    let (mut app, root) = note_app("minc-sidebar-toggle");
    app.restore_panel_width();
    app.open_path(&root.join("薄荷.md"));
    paint(&mut app);
    let toggle = app.layout.sidebar_button;
    let nav = &app.panels.sidebar.navigation;
    let lights = crate::layout::traffic_lights_end();
    if lights > 0.0 {
        let icon_left = toggle.x + (toggle.w - crate::icons::SIDEBAR_WIDTH) / 2.0;
        assert_eq!(icon_left, lights + 8.0);
    }
    assert_eq!(nav.tree_menu.right(), app.layout.sidebar.right() - 8.0);
    assert_eq!(nav.search.right() + BUTTON_GAP, nav.tree_menu.x);
    assert!(nav.search.x >= toggle.right() + BUTTON_GAP);
    assert_eq!(nav.tree_menu.y, toggle.y);
    let controls = app.caption_controls();
    assert!(controls.contains(&nav.tree_menu) && controls.contains(&nav.search));

    for open in [false, true] {
        press(&mut app, toggle);
        assert_eq!(app.sidebar_open(), open);
        app.panels.sidebar.motion = Some(SidebarMotion {
            from: if open { 0.0 } else { 1.0 },
            clock: Clock::stopped(SIDEBAR_ANIMATION, SIDEBAR_ANIMATION / 4),
        });
        paint(&mut app);
        assert!((0.0..1.0).contains(&app.sidebar_shown()));
        assert_eq!(app.layout.sidebar_button, toggle);
        assert_eq!(app.pointer.hover, Hover::SidebarButton);

        app.panels.sidebar.motion = None;
        paint(&mut app);
        assert_eq!(app.layout.sidebar_button, toggle);
        assert_eq!(app.pointer.hover, Hover::SidebarButton);
        assert!(app.layout.tabs.x >= toggle.right() + TAB_GAP);
    }

    app.panels.sidebar.width = crate::theme::DEFAULT_PANEL_WIDTH;
    paint(&mut app);
    let at = (app.layout.splitter.x + 1.0, 200.0);
    click(&mut app, at);
    click(&mut app, at);
    assert_eq!(app.panels.sidebar.width, crate::theme::MIN_PANEL_WIDTH);
    assert_eq!(app.layout.sidebar_button, toggle);

    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
}

#[test]
fn notebook_menu_has_a_full_width_trigger_and_a_single_workspace_path_row() {
    use crate::app::{MenuIcon, PopupRow};

    let dir = sample_folder("minc-compact-workspace-menu");
    let root = dir
        .join("Library/Mobile Documents")
        .join("A long parent directory name that would otherwise make the dropdown much wider")
        .join("仓库");
    std::fs::create_dir_all(&root).unwrap();
    let mut app = blank_app(false);
    app.panels.sidebar.legacy_chrome = false;
    app.open_path(&root);
    let root = app.library_root().unwrap().to_path_buf();
    paint(&mut app);
    let heading = app.panels.sidebar.navigation.folder_menu;
    assert_eq!(heading.w, app.layout.sidebar.w - 16.0);
    let bar = app.layout.sidebar_header;
    assert_eq!(heading.y + heading.h / 2.0, bar.y + bar.h / 2.0);
    if std::env::var_os("WINC_SNAPSHOT_DIR").is_some() {
        app.on_mouse_move(heading.x + heading.w / 2.0, heading.y + heading.h / 2.0);
        shoot(&mut app, "notebook-workspace-hover");
    }
    click(
        &mut app,
        (heading.right() - 3.0, heading.y + heading.h / 2.0),
    );
    assert!(app.notebook_menu_open(crate::app::workspace::notebook::SidebarTarget::FolderMenu));
    app.dismiss_popup();
    let recent = dir.join("最近目录");
    let root_menu = || MenuTarget::NotebookWorkspaces {
        roots: vec![root.clone(), recent.clone()],
        typora: Some(PathBuf::from("/Applications/Typora.app")),
    };
    app.open_context_menu(root_menu(), (heading.x, heading.bottom()));
    paint(&mut app);
    let label = app.menu_label(MenuCommand::SwitchNotebookWorkspace(0));
    let path = winc_editor::display_path(&root, None);
    assert_eq!(label.title, path);
    assert!(label.subtitle.is_none());
    assert_eq!(label.tooltip.as_deref(), Some(path.as_str()));
    let menu = app.overlays.context_menu.as_ref().unwrap();
    let level = &menu.popup.as_ref().unwrap().levels[0];
    assert_eq!(level.rect.w, 268.0);
    assert!(matches!(&level.rows[0], PopupRow::Heading(_)));
    assert!(matches!(
        level.rows[1],
        PopupRow::Item {
            checked: false,
            icon: Some(MenuIcon::NotebookFolder { current: true }),
            ..
        }
    ));
    assert!(level.rows.iter().any(|row| matches!(
        row,
        PopupRow::Item {
            icon: Some(MenuIcon::NotebookFolder { current: false }),
            ..
        }
    )));
    assert_eq!(
        level.rows.iter().any(|row| matches!(
            row,
            PopupRow::Item {
                icon: Some(MenuIcon::ICloud),
                ..
            }
        )),
        cfg!(target_os = "macos")
    );
    let open_with = level
        .rows
        .iter()
        .position(|row| matches!(row, PopupRow::Submenu { .. }))
        .unwrap();
    let at = level.row_rects[open_with];
    let reuses_before = app.frame.backdrop.reuses();
    app.frame.dirty = false;
    app.on_mouse_move(at.x + 17.0, at.y + at.h / 2.0);
    assert!(app.frame.dirty, "移动到新的菜单项时应触发浮层重绘");
    paint(&mut app);
    assert!(
        app.frame.backdrop.reuses() > reuses_before,
        "工作区菜单内切换悬停与展开子菜单应复用底层静态快照"
    );
    app.frame.dirty = false;
    app.on_mouse_move(at.x + 25.0, at.y + at.h / 2.0);
    assert!(!app.frame.dirty, "在同一菜单项内微移鼠标不应触发多余重绘");
    assert!(matches!(
        app.popup().unwrap().levels[1].rows[0],
        PopupRow::Item {
            icon: Some(MenuIcon::Typora),
            ..
        }
    ));
    assert!(super::menu_commands(&app).contains(&(MenuCommand::NotebookTypora, true)));
    app.run_menu_command(MenuCommand::NotebookTypora);
    assert_eq!(app.library_root(), Some(root.as_path()));
    app.dismiss_popup();
    paint(&mut app);
    assert!(
        !app.frame.backdrop.is_valid(),
        "关闭工作区菜单后应释放底层静态快照"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn notebook_tree_hides_dot_files_and_subdirectories_without_removing_them() {
    let (mut app, root) = note_app("minc-hidden-files");
    let tea = root.join("Recipes/茶.md");
    let hidden = [
        root.join(".gitignore"),
        root.join(".obsidian/types.json"),
        root.join("Recipes/.hidden/notes.md"),
    ];
    for file in &hidden {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, "preserve").unwrap();
    }
    let visible = root.join("Recipes/tea.v2.md");
    std::fs::write(&visible, "# Tea\n").unwrap();
    paint(&mut app);
    let workspace = app.editor.workspace.as_mut().unwrap();
    workspace.refresh();
    workspace.reveal(&tea);
    assert!(workspace.row_of(&tea).is_some() && workspace.row_of(&visible).is_some());
    for file in &hidden {
        workspace.reveal(file);
        assert!(workspace.row_of(file).is_none());
        assert_eq!(std::fs::read_to_string(file).unwrap(), "preserve");
    }
    assert!(!workspace.rows().iter().any(|row| row.name.starts_with('.')));
    assert_eq!(workspace.selected_entry().unwrap().path, tea);
    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
}

#[test]
fn notebook_sidebar_reaches_search_creation_and_outline() {
    let (mut app, root) = note_app("minc-navigation");
    paint(&mut app);
    assert!(app.hits.tree_tools.is_empty() && app.hits.tree_more.is_none());
    assert!(app.layout.outline.h == 0.0);
    assert!(app.sidebar_tabs().is_empty());
    let nav = &app.panels.sidebar.navigation;
    assert!(nav.folder_menu.bottom() <= app.layout.tree.y);
    assert!(nav.new_file.y >= app.layout.tree.bottom());
    let search = nav.search;
    let menu = nav.tree_menu;
    assert_eq!(
        app.tree_row_top(1) - app.tree_row_top(0),
        app.tree_row_height()
    );
    press(&mut app, menu);
    app.run_menu_command(MenuCommand::NotebookNameOrder(true));
    let workspace = app.editor.workspace.as_ref().unwrap();
    assert_eq!(workspace.rows()[0].kind, winc_editor::EntryKind::Dir);
    assert_eq!(workspace.rows()[1].path, root.join("薄荷.md"));
    press(&mut app, menu);
    app.run_menu_command(MenuCommand::NotebookShowHidden);
    assert!(
        app.editor
            .workspace
            .as_ref()
            .unwrap()
            .row_of(&root.join(".hidden.md"))
            .is_some()
    );
    press(&mut app, menu);
    app.run_menu_command(MenuCommand::NotebookShowHidden);
    press(&mut app, search);
    assert_eq!(app.panels.sidebar.view, SidebarView::Search);
    assert_eq!(app.focus, Focus::GlobalQuery);
    let parts = app.panel_parts();
    assert_eq!(parts.query.y, app.panels.sidebar.navigation.back.y);
    assert!(parts.query.right() < app.panels.sidebar.navigation.back.x);
    assert!(parts.scope_label.y > parts.query.bottom());
    assert!(parts.tools.iter().all(|(rect, _)| rect.w == 0.0));
    app.panels.global.query.set_text("薄荷");
    let [names, contents] = app.panel_parts().scope;
    press(&mut app, names);
    app.panels.global.engine.wait();
    paint(&mut app);
    let results = &app.panels.global.engine.results;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].path, root.join("薄荷.md"));
    assert!(results[0].matches.is_empty());

    press(&mut app, contents);
    app.panels.global.engine.wait();
    paint(&mut app);
    let results = &app.panels.global.engine.results;
    assert!(
        results
            .iter()
            .any(|file| file.path == root.join("Recipes/茶.md"))
    );
    let back = app.panels.sidebar.navigation.back;
    press(&mut app, back);
    assert_eq!(app.panels.sidebar.view, SidebarView::Files);

    let folder = root.join("Recipes");
    let row = app
        .editor
        .workspace
        .as_ref()
        .unwrap()
        .row_of(&folder)
        .unwrap();
    let (x, y) = tree_row_center(&app, row);
    app.on_mouse_move(x, y);
    let (_, add, more) = app.notebook_folder_actions(x, y).unwrap();
    press(&mut app, more);
    assert_eq!(
        app.overlays.context_menu.as_ref().unwrap().target,
        MenuTarget::Tree(Some(folder.clone()))
    );
    app.dismiss_popup();
    press(&mut app, add);
    assert_eq!(app.panels.name_input.as_ref().unwrap().dir, folder);
    app.close_name_input();

    let create = app.panels.sidebar.navigation.new_file;
    press(&mut app, create);
    let field = app.panels.name_input.as_ref().unwrap();
    assert_eq!(field.dir, root);
    assert_eq!(field.field.text(), "Untitled.md");
    app.close_name_input();

    let tea = root.join("Recipes/茶.md");
    app.open_path(&tea);
    paint(&mut app);

    let menu = app.panels.sidebar.navigation.tree_menu;
    press(&mut app, menu);
    app.run_menu_command(MenuCommand::NotebookOutline);
    paint(&mut app);
    assert_eq!(app.panels.sidebar.view, SidebarView::Outline);
    assert!(app.layout.tree.h == 0.0 && app.layout.outline.h > 200.0);
    let items: Vec<_> = app
        .panels
        .outline
        .items
        .iter()
        .map(|s| (s.depth, s.name.as_str()))
        .collect();
    assert_eq!(items, [(0, "茶"), (1, "冲泡"), (2, "时间")]);
    assert!(
        app.hits
            .outline
            .iter()
            .all(|(_, hit)| matches!(hit, OutlineTarget::Row(_)))
    );
    let row = app
        .hits
        .outline
        .iter()
        .find(|(_, hit)| *hit == OutlineTarget::Row(2))
        .unwrap()
        .0;
    press(&mut app, row);
    let doc = app.editor.active().unwrap();
    assert_eq!(doc.text().line_of(doc.selection.head), 6);
    let _ = std::fs::remove_dir_all(root.parent().unwrap());
}

#[test]
fn notebook_links_resolve_targets_without_a_backlinks_footer() {
    use winc_preview::note_links::WIKI_TARGET;

    let (mut app, root) = note_app("minc-link-navigation");
    let tea = root.join("Recipes/茶.md");
    let mint = root.join("薄荷.md");
    let copy = root.join("Export/Notebook/薄荷.md");
    std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
    std::fs::write(&copy, "# 薄荷副本\n").unwrap();
    std::fs::write(&tea, "# 茶\n\n[[薄荷|香草]] 和 [薄荷](../薄荷.md)。\n").unwrap();
    app.open_path(&tea);
    for _ in 0..3 {
        app.wait_for_markdown();
        paint(&mut app);
    }
    let link = app
        .hits
        .markdown_links
        .iter()
        .position(|(_, url)| url.strip_prefix(WIKI_TARGET) == Some("薄荷"))
        .expect("the Wiki link is a rendered hit");
    app.open_markdown_link(link);
    for _ in 0..3 {
        app.wait_for_markdown();
        paint(&mut app);
    }
    assert_eq!(
        app.editor.active().unwrap().path.as_deref(),
        Some(mint.as_path())
    );
    app.scroll_markdown_to(f32::MAX);
    paint(&mut app);

    // 不再在正文末尾渲染「引用此笔记」区域。
    let preview = &app.preview.markdown;
    let layout = preview.layout.as_ref().unwrap();
    assert_eq!(preview.height, layout.height());

    let index = app.note_links.index.as_ref().unwrap();
    assert!(index.backlinks(&mint).is_some());
    assert!(index.backlinks(&copy).is_none());
    let _ = std::fs::remove_dir_all(root.parent().unwrap());
}

#[test]
fn notebook_links_with_equally_near_targets_open_a_choice_not_a_note() {
    use winc_preview::note_links::WIKI_TARGET;

    let (mut app, root) = note_app("minc-link-choice");
    let tea = root.join("Recipes/茶.md");
    let other = root.join("Recipes/Herbs/薄荷.md");
    std::fs::create_dir_all(other.parent().unwrap()).unwrap();
    std::fs::write(&tea, "[[薄荷]]").unwrap();
    std::fs::write(&other, "# Another mint").unwrap();
    app.open_path(&tea);
    for _ in 0..3 {
        app.wait_for_markdown();
        paint(&mut app);
    }
    let link = app
        .hits
        .markdown_links
        .iter()
        .position(|(_, url)| url.strip_prefix(WIKI_TARGET) == Some("薄荷"))
        .unwrap();
    app.open_markdown_link(link);
    assert_eq!(
        app.editor.active().unwrap().path.as_deref(),
        Some(tea.as_path())
    );
    assert!(app.overlays.picker.open);
    assert_eq!(app.overlays.picker.hits.len(), 2);
    let index = app.note_links.index.as_ref().unwrap();
    assert!(index.backlinks(&root.join("薄荷.md")).is_none());
    assert!(index.backlinks(&other).is_none());
    let candidate = app
        .overlays
        .picker
        .hits
        .iter()
        .position(|&i| app.overlays.picker.notebook.as_ref().unwrap().index.notes[i].path == other)
        .unwrap();
    app.overlays.picker.selected = candidate;
    app.accept_notebook_search();
    assert_eq!(
        app.editor.active().unwrap().path.as_deref(),
        Some(other.as_path())
    );
    assert_eq!(std::fs::read_to_string(&tea).unwrap(), "[[薄荷]]");
    let _ = std::fs::remove_dir_all(root.parent().unwrap());
}

#[test]
fn notebook_save_shortcut_clears_the_title_dot_without_a_notice() {
    use crate::keymap::{KeyCode, Mods, Os, Stroke};

    use_vscode_keys();
    let (mut app, root) = note_app("minc-writing-manual-save");
    let note = root.join("薄荷.md");
    let original = std::fs::read_to_string(&note).unwrap();
    app.open_path(&note);
    paint(&mut app);
    let title = app.hits.document_title;
    app.run(Command::Insert("manual edit\n".into()));
    paint(&mut app);
    let dirty_title = app.hits.document_title;
    assert!(dirty_title.right() > title.right());
    if std::env::var_os("WINC_SNAPSHOT_DIR").is_some() {
        shoot(&mut app, "notebook-unsaved-title");
    }
    assert_eq!(app.auto_save_tick(Instant::now()), None);
    assert_eq!(std::fs::read_to_string(&note).unwrap(), original);
    assert!(app.editor.active().unwrap().is_dirty());

    assert!(app.on_shortcut(Stroke {
        key: KeyCode::Char('s'),
        mods: if Os::CURRENT == Os::Mac {
            Mods::META
        } else {
            Mods::CTRL
        },
    }));
    app.wait_for_saves();
    paint(&mut app);
    assert!(
        std::fs::read_to_string(&note)
            .unwrap()
            .contains("manual edit")
    );
    assert!(!app.editor.active().unwrap().is_dirty());
    assert_eq!(app.hits.document_title, title);
    assert!(app.notice.text.is_none() && app.status.message.is_none());
    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
}

#[test]
fn notebook_saves_within_one_second_and_flushes_the_latest_edit_before_switching() {
    use std::time::{Duration, Instant};

    let (mut app, root) = note_app("minc-writing-autosave");
    let tea = root.join("Recipes/茶.md");
    let mint = root.join("薄荷.md");
    app.config.auto_save = Some(winc_config::AutoSave::AfterDelay);
    app.open_path(&tea);
    let tea_id = app.editor.active().unwrap().id;
    app.run(Command::Insert("first edit\n".into()));
    let now = Instant::now();
    let deadline = now + Duration::from_secs(1);
    assert_eq!(app.auto_save_tick(now), Some(deadline));
    app.run(Command::Insert("second edit\n".into()));
    assert_eq!(
        app.auto_save_tick(now + Duration::from_millis(700)),
        Some(deadline)
    );
    assert_eq!(app.auto_save_tick(deadline), None);
    app.wait_for_saves();
    assert!(
        std::fs::read_to_string(&tea)
            .unwrap()
            .contains("second edit")
    );
    assert!(!app.editor.active().unwrap().is_dirty());

    app.run(Command::Insert("before switching\n".into()));
    app.open_path(&mint);
    assert!(app.editor.document(tea_id).is_none());
    assert_eq!(app.editor.documents.len(), 1);
    assert_eq!(
        app.editor.active().unwrap().path.as_deref(),
        Some(mint.as_path())
    );
    assert!(
        std::fs::read_to_string(&tea)
            .unwrap()
            .contains("before switching")
    );
    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
}

#[test]
fn notebook_preserves_the_open_document_if_switching_cannot_save_it() {
    let (mut app, root) = note_app("minc-writing-conflict");
    let tea = root.join("Recipes/茶.md");
    let mint = root.join("薄荷.md");
    app.open_path(&tea);
    let tea_id = app.editor.active().unwrap().id;
    app.run(Command::Insert("local edit\n".into()));
    std::fs::write(&tea, "An external process replaced this note.\n").unwrap();

    app.open_path_then(&mint, |_| panic!("the target did not open"));
    assert_eq!(app.editor.documents.len(), 1);
    let doc = app.editor.active().unwrap();
    assert_eq!(doc.id, tea_id);
    assert!(doc.is_dirty());
    assert!(doc.in_conflict());
    assert_eq!(
        std::fs::read_to_string(&tea).unwrap(),
        "An external process replaced this note.\n"
    );

    let recipes = root.join("Recipes");
    let row = app
        .editor
        .workspace
        .as_ref()
        .unwrap()
        .row_of(&recipes)
        .unwrap();
    let at = tree_row_center(&app, row);
    paint(&mut app);
    click(&mut app, at);
    assert!(!app.library_shown());
    assert_eq!(app.editor.active().unwrap().id, tea_id);
    assert!(app.editor.active().unwrap().is_dirty());
    let back = app.hits.workspace_history[0];
    press(&mut app, back);
    assert_eq!(app.editor.active().unwrap().id, tea_id);
    assert!(app.editor.active().unwrap().is_dirty());
    assert!(app.can_step_workspace_path(false));
    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
}

#[test]
fn notebook_restores_only_the_active_file_from_a_clean_legacy_session() {
    use winc_config::{Session, Tab};

    let (mut app, root) = note_app("minc-writing-session");
    let tea = root.join("Recipes/茶.md");
    let mint = root.join("薄荷.md");
    app.state.set_session(Session {
        folder: root.clone(),
        tabs: vec![Tab::File(tea), Tab::File(mint.clone())],
        active: Some(1),
        ..Session::default()
    });
    app.open_folder_now(&root);
    assert_eq!(app.editor.documents.len(), 1);
    assert_eq!(
        app.editor.active().unwrap().path.as_deref(),
        Some(mint.as_path())
    );
    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
}

#[test]
#[ignore = "manual notebook UI snapshots"]
fn shoot_notebook_navigation() {
    let mut empty = blank_app(false);
    empty.panels.sidebar.legacy_chrome = false;
    empty.restore_panel_width();
    empty.restore_sidebar_for_window();
    empty.state.recent = vec![
        PathBuf::from("/tmp/写作笔记"),
        PathBuf::from("/tmp/项目资料"),
    ];
    empty.relayout();
    shoot(&mut empty, "notebook-sidebar-empty");
    let (mut app, root) = note_app("minc-notebook-snapshots");
    app.restore_panel_width();
    app.relayout();
    let tea = root.join("Recipes/茶.md");
    let mint = root.join("薄荷.md");
    std::fs::write(&mint, "# 薄荷\n\n参见 [[茶]] 的冲泡步骤。\n").unwrap();
    app.refresh_library();
    shoot(&mut app, "notebook-library-root");
    let heading = app.panels.sidebar.navigation.folder_menu;
    press(&mut app, heading);
    shoot(&mut app, "notebook-workspace-menu");
    app.dismiss_popup();
    app.open_path(&tea);
    for _ in 0..4 {
        app.wait_for_markdown();
        paint(&mut app);
    }
    shoot(&mut app, "notebook-sidebar");
    let toggle = app.layout.sidebar_button;
    press(&mut app, toggle);
    shoot(&mut app, "notebook-sidebar-collapsed");
    press(&mut app, toggle);
    let menu = app.panels.sidebar.navigation.tree_menu;
    press(&mut app, menu);
    app.run_menu_command(MenuCommand::NotebookOutline);
    app.on_mouse_move(300.0, 120.0);
    paint(&mut app);
    shoot(&mut app, "notebook-outline");
    app.show_files();
    let search = app.panels.sidebar.navigation.search;
    press(&mut app, search);
    shoot(&mut app, "notebook-search");
    app.show_files();
    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
}

#[test]
fn clicking_blank_sidebar_area_returns_to_library_home() {
    let (mut app, root) = note_app("minc-sidebar-blank-home");
    let folder = root.join("Recipes");
    let note = root.join("薄荷.md");

    app.open_path(&note);
    paint(&mut app);
    assert!(!app.library_shown());

    let tree = app.layout.tree;
    let blank = (tree.x + 40.0, tree.bottom() - 12.0);
    assert!(app.tree_blank_at(blank.1));

    // 从笔记点击侧栏空白处：回到笔记库主页并取消文件树选中。
    click(&mut app, blank);
    paint(&mut app);
    assert!(app.library_shown());
    assert_eq!(app.library.folder.as_ref(), Some(&root));
    assert_eq!(app.editor.workspace.as_ref().unwrap().selected, None);

    // 从子目录点击侧栏空白处：同样回到笔记库根目录主页，双击不弹出新建输入框。
    let row = row_of(&app, "Recipes");
    let at = tree_row_center(&app, row);
    click(&mut app, at);
    paint(&mut app);
    assert_eq!(app.library.folder.as_ref(), Some(&folder));

    app.pointer.last_click = None;
    click(&mut app, blank);
    click(&mut app, blank);
    paint(&mut app);
    assert!(app.library_shown());
    assert_eq!(app.library.folder.as_ref(), Some(&root));
    assert_eq!(app.editor.workspace.as_ref().unwrap().selected, None);
    assert!(app.panels.name_input.is_none());

    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
}
