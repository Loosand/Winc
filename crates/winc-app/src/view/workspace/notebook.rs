//! Compact file navigation for local writing windows.

use super::*;
use crate::app::workspace::notebook::SidebarTarget;
use winc_i18n::t;
use winc_render::Faux;

pub(super) fn draw_navigation(app: &App, frame: &mut Frame<'_>) {
    let p = app.palette;
    let nav = &app.panels.sidebar.navigation;
    let clip = app.layout.sidebar;
    let hot = |target| app.pointer.hover == Hover::NotebookNav(target);
    let title = nav.folder_menu;
    let Some(root) = app.library_root() else {
        draw_empty_sidebar(app, frame);
        return;
    };
    let name = winc_editor::display_name(root);
    let size = app.fonts.ui;
    let weight = Faux {
        bold: true,
        italic: false,
    };
    let name = crate::layout::fit_text(&name, (title.w - 30.0).max(0.0), false, |text| {
        frame.measure_faux(FontKind::Ui, weight, size, text)
    });
    let folders_open = app.notebook_menu_open(SidebarTarget::FolderMenu);
    let tools_open = app.notebook_menu_open(SidebarTarget::TreeMenu);
    let switching = matches!(app.file_hover(), Some(crate::app::DropZone::NotebookSwitch));
    draw_button(
        frame,
        p,
        title,
        clip,
        hot(SidebarTarget::FolderMenu) || folders_open,
        false,
    );
    if switching {
        frame.rect_clipped(title, p.accent.alpha(0.13), RADIUS_ROW, clip);
    }
    frame.text_faux(
        FontKind::Ui,
        weight,
        size,
        title.x + 7.0,
        ui_baseline(app, title, size),
        &name,
        p.text,
        title,
    );
    let (x, y) = (title.right() - 12.0, center(title).1);
    for (dy, direction) in [(-2.5, icons::Direction::Up), (2.5, icons::Direction::Down)] {
        icons::chevron(frame, (x, y + dy), 5.0, direction, p.text_muted, title);
    }

    let search = nav.search;
    let menu = nav.tree_menu;
    draw_button(frame, p, search, clip, hot(SidebarTarget::Search), false);
    icons::magnifier(frame, center(search), p.text_muted, search);
    draw_button(
        frame,
        p,
        menu,
        clip,
        hot(SidebarTarget::TreeMenu),
        tools_open,
    );
    icons::filter(frame, center(menu), p.text_muted, menu);

    let r = nav.new_file;
    if r.h > 0.0 {
        let y = app.layout.tree.bottom();
        frame.rect_clipped(
            Rect::new(clip.x + 8.0, y, clip.w - 16.0, 1.0),
            p.separator,
            0.0,
            clip,
        );
        draw_button(frame, p, r, clip, hot(SidebarTarget::NewFile), false);
        icons::plus(frame, (r.x + 13.0, center(r).1), 9.0, p.text_muted, r);
        ui_text(
            frame,
            app,
            r.x + 27.0,
            r,
            t!("workspace-tree-tool-new-file"),
            p.text_muted,
            r,
        );
    }
}

pub(super) fn draw_back_button(app: &App, frame: &mut Frame<'_>) {
    let r = app.panels.sidebar.navigation.back;
    let p = app.palette;
    draw_button(
        frame,
        p,
        r,
        app.layout.sidebar,
        app.pointer.hover == Hover::NotebookNav(SidebarTarget::BackToFiles),
        false,
    );
    icons::close(frame, center(r), 9.0, p.text_muted, r);
}

fn draw_empty_sidebar(app: &App, frame: &mut Frame<'_>) {
    let p = app.palette;
    let nav = &app.panels.sidebar.navigation;
    let clip = app.layout.sidebar;
    let open = nav.open_folder;
    if open.h <= 0.0 {
        return;
    }
    let hot = |target| app.pointer.hover == Hover::NotebookNav(target);
    let (cx, _) = center(open);
    let tile = Rect::new(cx - 18.0, open.y - 46.0, 36.0, 36.0);
    frame.rect_clipped(tile, p.hover, RADIUS_ROW, clip);
    icons::folder_small(frame, center(tile), p.folder, clip);
    let text = t!("workspace-notebook-open-folder");
    let button = open;
    frame.rect_clipped(button, p.hover, RADIUS_ROW, clip);
    draw_button(
        frame,
        p,
        button,
        clip,
        hot(SidebarTarget::OpenFolder),
        false,
    );
    let label = ellipsize(frame, text, app.fonts.ui, (button.w - 12.0).max(0.0));
    let left = cx - frame.measure(FontKind::Ui, app.fonts.ui, &label) / 2.0;
    ui_text(frame, app, left, button, &label, p.text, button);

    for &(row, i) in &nav.recent {
        let Some(path) = app.state.recent.get(i) else {
            continue;
        };
        draw_button(frame, p, row, clip, hot(SidebarTarget::Recent(i)), false);
        icons::folder_small(frame, (row.x + 13.0, center(row).1), p.folder, row);
        let name = app.recent_title(path);
        let name = ellipsize(frame, &name, app.fonts.ui, (row.w - 30.0).max(0.0));
        ui_text(frame, app, row.x + 26.0, row, &name, p.text_muted, row);
    }
    let more = nav.more;
    draw_button(frame, p, more, clip, hot(SidebarTarget::MoreFolders), false);
    icons::more(frame, (more.x + 13.0, center(more).1), p.text_muted, more);
    ui_text(
        frame,
        app,
        more.x + 26.0,
        more,
        t!("workspace-notebook-more-folders"),
        p.text_muted,
        more,
    );
}

pub(super) fn draw_folder_actions(app: &App, frame: &mut Frame<'_>) {
    let (x, y) = app.mouse();
    let Some((index, add, more)) = app.notebook_folder_actions(x, y) else {
        return;
    };
    let p = app.palette;
    let clip = app.layout.tree;
    for (r, target) in [
        (add, SidebarTarget::FolderNew(index)),
        (more, SidebarTarget::FolderContext(index)),
    ] {
        draw_button(
            frame,
            p,
            r,
            clip,
            app.pointer.hover == Hover::NotebookNav(target),
            false,
        );
    }
    icons::plus(frame, center(add), 8.0, p.text_muted, add);
    icons::more(frame, center(more), p.text_muted, more);
}
