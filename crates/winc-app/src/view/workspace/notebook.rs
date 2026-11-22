//! The notebook's navigation and current folder switcher above the file tree.

use super::*;
use crate::app::workspace::notebook::SidebarTarget;
use winc_i18n::t;

pub(super) fn draw_navigation(app: &App, frame: &mut Frame<'_>) {
    super::library::draw_home_button(app, frame);
    let p = app.palette;
    let nav = &app.panels.sidebar.navigation;
    let clip = app.layout.sidebar;
    let r = nav.search;
    let hot = |target| app.pointer.hover == Hover::NotebookNav(target);
    draw_button(frame, p, r, clip, hot(SidebarTarget::Search), false);
    icons::magnifier(frame, (r.x + 15.0, center(r).1), p.text_muted, r);
    ui_text(
        frame,
        app,
        r.x + 30.0,
        r,
        t!("workspace-notebook-search"),
        p.text,
        r,
    );

    let graph = nav.graph;
    draw_button(
        frame,
        p,
        graph,
        clip,
        hot(SidebarTarget::Graph),
        app.note_graph_shown(),
    );
    super::note_graph::icon(
        frame,
        (graph.x + 15.0, center(graph).1),
        p.text_muted,
        graph,
    );
    ui_text(
        frame,
        app,
        graph.x + 30.0,
        graph,
        t!("workspace-note-graph"),
        p.text,
        graph,
    );

    let r = nav.documents;
    let switching = matches!(app.file_hover(), Some(crate::app::DropZone::NotebookSwitch));
    draw_button(frame, p, r, clip, hot(SidebarTarget::Documents), false);
    if switching {
        frame.rect_clipped(r, p.accent.alpha(0.13), 5.0, clip);
    }
    let turn = if app.notebook_documents_open() {
        std::f32::consts::FRAC_PI_2
    } else {
        0.0
    };
    icons::chevron_turned(frame, (r.x + 10.0, center(r).1), 6.0, turn, p.text_dim, r);
    let menu = nav.folder_menu;
    let heading_clip = r.intersect(Rect::new(r.x, r.y, (menu.x - r.x).max(0.0), r.h));
    ui_text_small(
        frame,
        app,
        r.x + 22.0,
        r,
        t!("workspace-library-notes"),
        if switching { p.accent } else { p.text_muted },
        heading_clip,
    );
    draw_button(frame, p, menu, clip, hot(SidebarTarget::FolderMenu), false);
    if menu.w >= 64.0
        && let Some(root) = app.library_root()
    {
        let name = winc_editor::display_name(root);
        let label = ellipsize(frame, &name, app.fonts.ui_small, menu.w - 24.0);
        let width = frame.measure(FontKind::Ui, app.fonts.ui_small, &label);
        ui_text_small(
            frame,
            app,
            menu.right() - 19.0 - width,
            menu,
            &label,
            p.text_dim,
            menu,
        );
        icons::chevron_turned(
            frame,
            (menu.right() - 9.0, center(menu).1),
            6.0,
            std::f32::consts::FRAC_PI_2,
            p.text_dim,
            menu,
        );
    } else {
        icons::more(frame, center(menu), p.text_dim, menu);
    }
}
