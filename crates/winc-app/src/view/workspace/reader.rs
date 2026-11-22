//! Reading navigation and the same folder and paper tiles as the notebook home.

use super::library::{ShelfCard, draw_grid_scrollbar, draw_virtual_card};
use super::*;
use crate::app::reader::{Target, subscriptions::Page};
use crate::app::workspace::library::{FolderPreview, PreviewLine};

type Control<'a> = (&'a str, Target, bool, bool);

pub(in crate::view) fn entry(app: &mut App, frame: &mut Frame<'_>) {
    let rect = app.layout.reader.intersect(app.layout.top_bar);
    if !app.layout.terminal.contains(center(rect).0, center(rect).1) {
        button(
            app,
            frame,
            rect,
            (t!("reader-title"), Target::Home, app.reader_active(), true),
        );
    }
}

pub(super) fn sidebar_entry(app: &mut App, frame: &mut Frame<'_>) {
    let r = app.reader_sidebar_button();
    if r.w <= 0.0 || r.h <= 0.0 {
        return;
    }
    let p = app.palette;
    let active = app.reader_active();
    let hot = app.pointer.hover == Hover::Reader(Target::Home);
    draw_button(frame, p, r, app.layout.sidebar, hot, active);
    let color = if active { p.accent } else { p.text_muted };
    let (x, y) = (r.x + 15.0, center(r).1);
    // Open book: two pages folding into the center seam.
    for side in [-1.0, 1.0] {
        let edge = x + side * 6.0;
        for (from, to) in [
            ((x, y - 4.0), (edge, y - 6.0)),
            ((edge, y - 6.0), (edge, y + 4.0)),
            ((edge, y + 4.0), (x, y + 6.0)),
        ] {
            frame.line(from, to, 1.25, color, r);
        }
    }
    frame.line((x, y - 4.0), (x, y + 6.0), 1.25, color, r);
    ui_text(frame, app, r.x + 30.0, r, t!("reader-title"), p.text, r);
    app.hits.reader.push((r, Target::Home));
}

pub(in crate::view) fn home(app: &mut App, frame: &mut Frame<'_>) {
    let area = app.layout.editor;
    let grid = app.reader_grid();
    let p = app.palette;
    let sub = &app.reader.subscriptions;
    let in_feed = sub.current_feed().is_some();
    let title = sub
        .current_feed()
        .map_or(t!("reader-title"), |feed| &feed.title);
    let x = grid.body.x;
    let width = grid.body.w;
    let title_size = app.fonts.ui * 1.65;
    let title = truncate_end(frame, title, title_size, width);
    frame.text_faux(
        FontKind::Ui,
        Faux {
            bold: true,
            italic: false,
        },
        title_size,
        x,
        area.y + 38.0,
        &title,
        p.text,
        area,
    );
    let toolbar = |y| Rect::new(x, y, width, BUTTON).intersect(area);
    let overview = sub.page == Page::Overview;
    let saved = sub.page == Page::Saved;
    let tabs = [
        (t!("reader-overview"), Target::Overview, overview, true),
        (t!("reader-saved"), Target::Saved, saved, true),
        (
            t!("reader-subscriptions"),
            Target::Subscriptions,
            !overview && !saved,
            true,
        ),
    ];
    let mut actions = Vec::with_capacity(3);
    if overview || saved {
        actions.push((t!("reader-add"), Target::Add, false, !app.reader.busy));
    }
    let busy = sub.busy || sub.refreshing();
    if !saved && !in_feed {
        actions.push((t!("reader-subscribe"), Target::Subscribe, false, !busy));
    }
    if !saved && (!sub.feeds.is_empty() || sub.refreshing()) {
        let label = if sub.refreshing() {
            t!("reader-refreshing")
        } else {
            t!("reader-refresh")
        };
        actions.push((label, Target::RefreshFeeds, false, !busy));
    }
    if in_feed {
        actions.push((t!("reader-unread"), Target::UnreadOnly, sub.unread, true));
        actions.push((t!("reader-unsubscribe"), Target::Unsubscribe, false, !busy));
    }
    if app.reader_toolbar_inline() {
        let row = toolbar(area.y + 54.0);
        let left = row.w * 0.44;
        controls(
            app,
            frame,
            Rect::new(row.x, row.y, left, row.h),
            &tabs,
            false,
        );
        controls(
            app,
            frame,
            Rect::new(row.x + left, row.y, row.w - left, row.h),
            &actions,
            true,
        );
    } else {
        controls(app, frame, toolbar(area.y + 51.0), &tabs, false);
        controls(app, frame, toolbar(area.y + 86.0), &actions, false);
    }

    let count = app.reader_card_count();
    if count == 0 {
        let text = if app.reader.subscriptions.refreshing() {
            t!("reader-refreshing")
        } else if app.reader.busy || app.reader.subscriptions.busy {
            t!("reader-working")
        } else if overview || saved {
            t!("reader-empty")
        } else if in_feed && app.reader.subscriptions.unread {
            t!("reader-caught-up")
        } else if in_feed {
            t!("reader-no-items")
        } else {
            t!("reader-no-subscriptions")
        };
        let row = Rect::new(x, grid.body.y, width, 30.0).intersect(area);
        ui_text(frame, app, x, row, text, p.text_muted, row);
        return;
    }

    let scroll = drawn_scroll(app.reader.subscriptions.scroll.min(grid.max_scroll(count)));
    let card_area = Rect::new(area.x, grid.body.y - 4.0, area.w, grid.body.h + 4.0);
    for index in grid.visible(count, scroll) {
        let Some(target) = app.reader_card(index) else {
            continue;
        };
        let rect = grid.card(index, scroll, matches!(target, Target::Feed(_)));
        let visible = rect.intersect(grid.body);
        if visible.w <= 0.0 || visible.h <= 0.0 {
            continue;
        }
        let Some((title, card, footer)) = tile(app, target) else {
            continue;
        };
        let hot = app.pointer.hover == Hover::Reader(target)
            || matches!((target, app.pointer.hover), (Target::Article(i), Hover::Reader(Target::Remove(j))) if i == j);
        let clip = rect.inset(-6.0, -6.0).intersect(card_area);
        draw_virtual_card(app, frame, rect, clip, title, card, hot);
        if let Some(mut footer) = footer {
            if !hot {
                footer.remove = None;
            }
            paper_footer(app, frame, rect, visible, footer);
        }
        app.hits.reader.push((visible, target));
    }
    draw_grid_scrollbar(app, frame);
}

struct PaperFooter {
    label: &'static str,
    unread: bool,
    remove: Option<usize>,
}

fn tile(app: &App, target: Target) -> Option<(&str, ShelfCard<'_>, Option<PaperFooter>)> {
    let sub = &app.reader.subscriptions;
    match target {
        Target::Feed(i) => {
            let feed = sub.feeds.get(i)?;
            let detail = if feed.failed {
                t!("reader-refresh-failed").to_owned()
            } else {
                t!("reader-feed-count", count = feed.unread.len())
            };
            let count = feed.entries.len();
            let info = FolderPreview {
                count,
                papers: count.clamp(1, 3),
                seeds: [19, 11, 5],
                front: Some((
                    feed.entries
                        .first()
                        .map_or_else(|| feed.title.clone(), |e| e.title.clone()),
                    vec![PreviewLine::Text(detail)],
                )),
            };
            Some((&feed.title, ShelfCard::Folder(info), None))
        }
        Target::Article(i) => {
            let article = app.reader.articles.get(i)?;
            let label = if article.full {
                t!("reader-full")
            } else {
                t!("reader-link-only")
            };
            let footer = PaperFooter {
                label,
                unread: false,
                remove: (!app.reader.busy).then_some(i),
            };
            Some((
                &article.title,
                ShelfCard::Paper(&article.card_lines, None),
                Some(footer),
            ))
        }
        Target::FeedItem(i) => {
            let entry = sub.current_feed()?.entries.get(i)?;
            let web = (!entry.full)
                .then(|| app.reader.original_excerpts.get(&entry.url))
                .flatten();
            let footer = PaperFooter {
                label: if web.is_some() {
                    t!("reader-web-excerpt")
                } else {
                    entry.content_label()
                },
                unread: !entry.read,
                remove: None,
            };
            let hint = if entry.full {
                None
            } else {
                Some(t!("reader-feed-card-hint"))
            };
            let lines = web.map_or(entry.card_lines.as_ref(), Vec::as_slice);
            Some((&entry.title, ShelfCard::Paper(lines, hint), Some(footer)))
        }
        _ => None,
    }
}

fn paper_footer(app: &mut App, frame: &mut Frame<'_>, rect: Rect, clip: Rect, footer: PaperFooter) {
    let y = rect.bottom() - 33.0;
    let whole = Rect::new(rect.x + 12.0, y, (rect.w - 24.0).max(0.0), 25.0);
    let p = app.palette;
    frame.rect_clipped(
        Rect::new(whole.x, y - 2.0, whole.w, 1.0),
        p.border,
        0.0,
        clip,
    );
    let mut left = whole.x;
    if footer.unread {
        frame.rect_clipped(
            Rect::new(left, center(whole).1 - 2.5, 5.0, 5.0),
            p.accent,
            2.5,
            clip,
        );
        left += 10.0;
    }
    let right = if let Some(i) = footer.remove {
        let remove = t!("reader-remove");
        let w = (frame.measure(FontKind::Ui, app.fonts.ui, remove) + 12.0).min(whole.w * 0.4);
        let control = Rect::new(whole.right() - w, y, w, whole.h);
        // Use the original footer position, not the clipped bottom, while scrolling.
        let space = control.intersect(clip);
        if space.h >= control.h * 0.65 {
            button(app, frame, space, (remove, Target::Remove(i), false, true));
        }
        control.x - 5.0
    } else {
        whole.right()
    };
    let area = Rect::new(left, y, (right - left).max(0.0), whole.h).intersect(clip);
    if area.w > 0.0 && area.h > 0.0 {
        let label = truncate_end(frame, footer.label, app.fonts.ui_small, area.w);
        ui_text_small(frame, app, left, whole, &label, p.text_muted, area);
    }
}

pub(in crate::view) fn article(app: &mut App, frame: &mut Frame<'_>) {
    let area = app.layout.editor;
    let body = app.reader_body_area();
    let p = app.palette;
    let header = Rect::new(area.x, area.y, area.w, body.y - area.y);
    frame.rect(header, p.editor_bg);
    frame.rect(
        Rect::new(header.x, header.bottom() - 1.0, header.w, 1.0),
        p.border,
    );
    let selected = app.preview.markdown.doc == app.editor.active().map(|d| d.id)
        && app.preview.markdown.selection.is_some_and(|(a, b)| a != b);
    let mut actions = vec![
        (t!("reader-back"), Target::Back, false, true),
        (t!("reader-source"), Target::Source, false, true),
    ];
    if app.reader_full() {
        actions.push((t!("reader-quote"), Target::Quote, false, selected));
    } else if !app.reader_feed_entry() {
        let label = if app.reader.busy {
            t!("reader-refreshing")
        } else {
            t!("reader-retry")
        };
        actions.push((label, Target::Retry, false, !app.reader.busy));
    }
    if app.reader_feed_entry() {
        if !app.reader_full() {
            let label = if app.reader.busy {
                t!("reader-reading-full")
            } else {
                t!("reader-read-full")
            };
            actions.push((label, Target::ReadFull, false, !app.reader.busy));
        }
        actions.push((
            t!("reader-save-offline"),
            Target::Bookmark,
            false,
            !app.reader.busy,
        ));
    }
    if app.reader_key().is_some() {
        actions.push((
            t!("reader-remove"),
            Target::RemoveCurrent,
            false,
            !app.reader.busy,
        ));
    }
    let toolbar = Rect::new(
        header.x + 16.0,
        header.y + (header.h - BUTTON) / 2.0,
        (header.w - 32.0).max(0.0),
        BUTTON,
    )
    .intersect(header);
    controls(app, frame, toolbar, &actions, false);
}

fn controls(
    app: &mut App,
    frame: &mut Frame<'_>,
    area: Rect,
    actions: &[Control<'_>],
    trailing: bool,
) {
    let widths: Vec<_> = actions
        .iter()
        .map(|(label, target, ..)| {
            let mut width = frame.measure(FontKind::Ui, app.fonts.ui, label);
            let stable_labels = match target {
                Target::RefreshFeeds => Some([t!("reader-refresh"), t!("reader-refreshing")]),
                Target::Retry => Some([t!("reader-retry"), t!("reader-refreshing")]),
                Target::ReadFull => Some([t!("reader-read-full"), t!("reader-reading-full")]),
                _ => None,
            };
            if let Some(labels) = stable_labels {
                for label in labels {
                    width = width.max(frame.measure(FontKind::Ui, app.fonts.ui, label));
                }
            }
            width + 22.0
        })
        .collect();
    let gap = 4.0 * actions.len().saturating_sub(1) as f32;
    let total = widths.iter().sum::<f32>();
    let scale = ((area.w - gap).max(0.0) / total.max(1.0)).min(1.0);
    let mut x = if trailing {
        area.right() - total * scale - gap
    } else {
        area.x
    };
    for (&action, width) in actions.iter().zip(widths) {
        let w = (width * scale).floor();
        button(app, frame, Rect::new(x, area.y, w, area.h), action);
        x += w + 4.0;
    }
}

fn button(
    app: &mut App,
    frame: &mut Frame<'_>,
    rect: Rect,
    (label, target, active, enabled): Control<'_>,
) {
    if rect.w <= 0.0 || rect.h <= 0.0 {
        return;
    }
    let p = app.palette;
    let hot = enabled && app.pointer.hover == Hover::Reader(target);
    let primary = enabled
        && (matches!(target, Target::Add | Target::Quote | Target::ReadFull)
            || target == Target::Bookmark && app.reader_full());
    let emphasis = active || primary;
    if emphasis {
        let fill = p.accent.alpha(if hot { 0.20 } else { 0.12 });
        frame.rect_clipped(rect, fill, RADIUS_ROW, rect);
    } else {
        draw_button(frame, p, rect, rect, hot, false);
    }
    let label = truncate_end(frame, label, app.fonts.ui, (rect.w - 8.0).max(0.0));
    let text = frame.measure(FontKind::Ui, app.fonts.ui, &label);
    let color = if !enabled {
        p.text_dim
    } else if emphasis {
        p.accent
    } else if target_matches_quiet_action(target) {
        p.text_muted
    } else {
        p.text
    };
    ui_text(
        frame,
        app,
        rect.x + (rect.w - text) / 2.0,
        rect,
        &label,
        color,
        rect,
    );
    if enabled {
        app.hits.reader.push((rect, target));
    }
}

fn target_matches_quiet_action(target: Target) -> bool {
    matches!(
        target,
        Target::Remove(_) | Target::RemoveCurrent | Target::Unsubscribe
    )
}
