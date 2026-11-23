use super::*;
use crate::app::OutlineTarget;
use crate::layout::SidebarView;

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
fn notebook_documents_can_collapse_and_outline_headings_jump() {
    let (mut app, root) = note_app("minc-navigation");
    paint(&mut app);
    assert!(app.hits.tree_tools.is_empty() && app.hits.tree_more.is_none());
    assert!(app.layout.outline.h == 0.0);
    let views: Vec<_> = app
        .sidebar_tabs()
        .into_iter()
        .map(|(_, view)| view)
        .collect();
    assert_eq!(views, [SidebarView::Files, SidebarView::Outline]);
    let docs = app.panels.sidebar.navigation.documents;
    let fold = Rect::new(docs.x, docs.y, 20.0, docs.h);
    press(&mut app, fold);
    assert!(!app.notebook_documents_open());
    assert_eq!(app.layout.tree.h, 0.0);
    press(&mut app, fold);
    assert!(app.layout.tree.h > 0.0);

    let tea = root.join("Recipes/茶.md");
    app.open_path(&tea);
    paint(&mut app);

    let outline = app.sidebar_tabs()[1].0;
    press(&mut app, outline);
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
fn notebook_links_footer_and_graph_share_the_note_targets() {
    use crate::app::workspace::note_links::footer::{Action as Footer, CONTROL};
    use crate::app::workspace::note_links::graph::Target as Graph;
    use winc_preview::note_links::WIKI_TARGET;
    use winit::keyboard::{Key, NamedKey};

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

    let hit_for = |app: &App, matches: fn(&Footer) -> bool| {
        let i = app.note_links.footer_hits.iter().position(matches).unwrap();
        let key = format!("{CONTROL}{i}");
        app.hits
            .markdown_links
            .iter()
            .find(|(_, url)| *url == key)
            .unwrap()
            .0
    };
    let expand = hit_for(&app, |hit| matches!(hit, Footer::Expand(_)));
    press(&mut app, expand);
    assert_eq!(
        app.note_links
            .footer_hits
            .iter()
            .filter(|hit| matches!(hit, Footer::Cite(..)))
            .count(),
        2
    );
    let citation = hit_for(&app, |hit| matches!(hit, Footer::Cite(..)));
    press(&mut app, citation);
    let doc = app.editor.active().unwrap();
    assert_eq!(doc.path.as_deref(), Some(tea.as_path()));
    assert_eq!(
        doc.text()
            .rope()
            .unwrap()
            .slice(doc.selection.from()..doc.selection.to())
            .to_string(),
        "[[薄荷|香草]]"
    );

    app.open_path(&mint);
    for _ in 0..3 {
        app.wait_for_markdown();
        paint(&mut app);
    }
    app.scroll_markdown_to(f32::MAX);
    paint(&mut app);
    let local = hit_for(&app, |hit| matches!(hit, Footer::Graph));
    press(&mut app, local);
    assert!(app.note_graph_shown());
    assert_eq!(app.note_links.graph.center.as_deref(), Some(mint.as_path()));
    assert_eq!(app.note_links.graph.local.as_ref().unwrap().edges.len(), 1);
    let open = app
        .note_links
        .graph
        .hits
        .iter()
        .find_map(|(r, hit)| match hit {
            Graph::Node(i) if app.note_links.graph.local.as_ref().unwrap().paths[*i] == tea => {
                Some(*r)
            }
            _ => None,
        })
        .unwrap();
    press(&mut app, open);
    app.note_graph_key(&Key::Named(NamedKey::Enter), false);
    assert_eq!(
        app.editor.active().unwrap().path.as_deref(),
        Some(tea.as_path())
    );

    let sidebar = app.panels.sidebar.navigation.graph;
    press(&mut app, sidebar);
    assert!(app.note_graph_shown());
    assert!(app.note_links.graph.center.is_none());
    let index = app.note_links.index.as_ref().unwrap();
    assert!(index.backlinks(&copy).is_none());
    assert_eq!(index.graph.edges.len(), 1);
    let search = app
        .note_links
        .graph
        .hits
        .iter()
        .find(|(_, hit)| *hit == Graph::Search)
        .unwrap()
        .0;
    press(&mut app, search);
    app.overlays.picker.input.set_text("薄荷");
    app.picker_refresh();
    let picker = &mut app.overlays.picker;
    let notes = &picker.notebook.as_ref().unwrap().index.notes;
    picker.selected = picker
        .hits
        .iter()
        .position(|&i| notes[i].path == mint)
        .unwrap();
    app.accept_notebook_search();
    assert!(app.note_graph_shown());
    assert_eq!(
        app.note_links.graph.selected.as_deref(),
        Some(mint.as_path())
    );
    let _ = std::fs::remove_dir_all(root.parent().unwrap());
}

#[test]
fn notebook_hides_the_backlinks_footer_when_the_last_reference_is_removed() {
    let (mut app, root) = note_app("minc-empty-backlinks");
    let tea = root.join("Recipes/茶.md");
    let mint = root.join("薄荷.md");
    std::fs::write(&tea, "薄荷可以泡茶：[[薄荷]]。\n").unwrap();
    app.open_path(&tea);
    for _ in 0..3 {
        app.wait_for_markdown();
        paint(&mut app);
    }
    let no_footer = |app: &App| {
        let preview = &app.preview.markdown;
        let layout = preview.layout.as_ref().unwrap();
        assert_eq!(preview.height, layout.height + layout.pad * 2.0);
        assert!(app.note_links.footer_hits.is_empty());
    };
    no_footer(&app);

    // An outgoing-only note can still open its local graph from the filename breadcrumb.
    assert!(app.hits.tabs.is_empty());
    assert!(
        app.hits
            .status
            .iter()
            .all(|&(_, item)| item != crate::app::StatusItem::Settings)
    );
    let &(file, _) = app.hits.crumbs.last().unwrap();
    assert!(file.y < app.layout.editor.y);
    right_click(&mut app, (file.x + file.w / 2.0, file.y + file.h / 2.0));
    assert!(menu_commands(&app).contains(&(MenuCommand::ViewNoteGraph, true)));
    app.run_menu_command(MenuCommand::ViewNoteGraph);
    assert_eq!(app.editor.documents.len(), 1);
    assert!(app.note_graph_shown());
    assert_eq!(app.note_links.graph.center.as_deref(), Some(tea.as_path()));
    assert_eq!(app.note_links.graph.local.as_ref().unwrap().edges.len(), 1);

    app.open_path(&mint);
    for _ in 0..3 {
        app.wait_for_markdown();
        paint(&mut app);
    }
    assert!(!app.note_links.footer_hits.is_empty());
    std::fs::write(&tea, "没有链接。\n").unwrap();
    app.library_paths_changed(false, std::slice::from_ref(&tea));
    paint(&mut app);
    no_footer(&app);
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
    assert!(
        app.note_links
            .index
            .as_ref()
            .unwrap()
            .graph
            .edges
            .is_empty()
    );
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
#[ignore = "manual measurement on generated note libraries"]
fn measure_notebook_graph() {
    let (mut app, root) = note_app("minc-graph-size");
    let mut previous = 0;
    for count in [100, 1_000] {
        for id in previous..count {
            let target = id / 4;
            std::fs::write(
                root.join(format!("Note {id}.md")),
                format!("# Note {id}\n\n[[Note {target}]]\n"),
            )
            .unwrap();
        }
        previous = count;
        let start = std::time::Instant::now();
        if app.note_links.index.is_some() {
            app.library_paths_changed(true, &[]);
        }
        app.open_note_graph(None);
        let indexed = start.elapsed();
        paint(&mut app);
        let first_frame = start.elapsed();
        let body = app.note_links.graph.area;
        app.pointer.mouse = (body.x + body.w / 2.0, body.y + body.h / 2.0);
        let start = std::time::Instant::now();
        for _ in 0..6 {
            app.wheel_note_graph(winit::event::MouseScrollDelta::LineDelta(0.0, 1.0));
            paint(&mut app);
        }
        eprintln!(
            "{count} generated notes: index {indexed:?}, first paint {first_frame:?}, zoom paint pairs {:?}",
            start.elapsed() / 6
        );
        assert_eq!(
            app.note_links.index.as_ref().unwrap().graph.paths.len(),
            count + 2
        );
    }
    let _ = std::fs::remove_dir_all(root.parent().unwrap());
}

#[test]
fn notebook_saves_within_one_second_and_flushes_the_latest_edit_before_switching() {
    use std::time::{Duration, Instant};

    let (mut app, root) = note_app("minc-writing-autosave");
    let tea = root.join("Recipes/茶.md");
    let mint = root.join("薄荷.md");
    app.config.auto_save = Some(winc_config::AutoSave::Off);
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
    let (mut app, root) = note_app("minc-notebook-snapshots");
    let tea = root.join("Recipes/茶.md");
    let mint = root.join("薄荷.md");
    std::fs::write(&mint, "# 薄荷\n\n参见 [[茶]] 的冲泡步骤。\n").unwrap();
    app.open_path(&tea);
    for _ in 0..4 {
        app.wait_for_markdown();
        paint(&mut app);
    }
    shoot(&mut app, "notebook-sidebar");
    let outline = app.sidebar_tabs()[1].0;
    press(&mut app, outline);
    app.on_mouse_move(300.0, 120.0);
    paint(&mut app);
    shoot(&mut app, "notebook-outline");
    app.open_notebook_search();
    paint(&mut app);
    shoot(&mut app, "notebook-search");
    app.close_picker();
    app.open_note_graph(None);
    paint(&mut app);
    shoot(&mut app, "notebook-graph");
    app.open_path(&tea);
    for _ in 0..3 {
        app.wait_for_markdown();
        paint(&mut app);
    }
    app.scroll_markdown_to(f32::MAX);
    paint(&mut app);
    shoot(&mut app, "notebook-backlinks");
    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
}
