use super::*;
use crate::app::reader::{Target, TestArticle, tests::site::Site};

fn press(app: &mut App, target: Target) {
    paint(app);
    let (r, _) = app
        .hits
        .reader
        .iter()
        .find(|(_, t)| *t == target)
        .unwrap_or_else(|| panic!("missing reader target {target:?}"));
    click(app, (r.x + r.w / 2.0, r.y + r.h / 2.0));
}

fn submit(app: &mut App, url: &str) {
    app.bars.goto_input.as_mut().unwrap().set_text(url);
    app.submit_prompt();
}

fn select_for_quote(app: &mut App) {
    app.preview.markdown.focused = true;
    app.markdown_select_all();
    press(app, Target::Quote);
}

#[test]
fn reader_sits_under_notebook_home_and_starts_in_my_reading() {
    let dir = sample_folder("reader-sidebar-entry");
    let mut app = blank_app(false);
    app.panels.sidebar.legacy_chrome = false;
    app.store.state_path = Some(dir.join("state.json"));
    app.open_path(&dir);
    paint(&mut app);
    let r = app.reader_sidebar_button();
    assert!(r.w > 0.0 && r.y > app.library_home_button().y);
    assert!(r.bottom() <= app.panels.sidebar.navigation.search.y);
    assert_eq!(app.layout.reader.w, 0.0);
    assert_eq!(
        app.hits
            .reader
            .iter()
            .filter(|(_, t)| *t == Target::Home)
            .count(),
        1
    );
    press(&mut app, Target::Home);
    assert!(app.reader_home());
    assert!(matches!(
        app.reader.subscriptions.page,
        crate::app::reader::subscriptions::Page::Overview
    ));
    paint(&mut app);
    for tab in [Target::Overview, Target::Saved, Target::Subscriptions] {
        assert!(app.hits.reader.iter().any(|(_, t)| *t == tab));
    }
    app.frame.renderer =
        Some(Renderer::offscreen(650.0, 540.0, SCALE, app.palette.frame_bg, None, None).unwrap());
    paint(&mut app);
    let area = |target| {
        app.hits
            .reader
            .iter()
            .find(|(_, t)| *t == target)
            .unwrap()
            .0
    };
    assert!(!app.reader_toolbar_inline());
    assert!(area(Target::Add).y >= area(Target::Overview).bottom());
    let grid = app.reader_grid();
    assert!(area(Target::Add).bottom() <= grid.body.y);
    assert!(grid.card(0, 0.0, false).h < grid.body.h * 0.7);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
#[ignore = "offscreen Reading UI snapshots"]
fn shoot_reading_shelf() {
    let Some(site) = Site::start() else {
        return;
    };
    let dir = sample_folder("reader-shelf-shot");
    let mut app = blank_app(true);
    app.panels.sidebar.legacy_chrome = false;
    app.store.state_path = Some(dir.join("state.json"));
    app.open_path(&dir);
    press(&mut app, Target::Home);
    press(&mut app, Target::Subscribe);
    submit(&mut app, &site.url("/feed"));
    app.reader.articles = (0..7).map(TestArticle::example_card).collect();
    shoot(&mut app, "reading-shelf");
    let wide = app.frame.renderer.replace(
        Renderer::offscreen(650.0, 540.0, SCALE, app.palette.frame_bg, None, None).unwrap(),
    );
    shoot(&mut app, "reading-shelf-compact");
    app.frame.renderer = wide;
    press(&mut app, Target::Subscribe);
    submit(&mut app, &site.url("/excerpt-feed"));
    let index = app
        .reader
        .subscriptions
        .feeds
        .iter()
        .position(|feed| feed.url == site.url("/excerpt-feed"))
        .unwrap();
    press(&mut app, Target::Feed(index));
    shoot(&mut app, "reading-feed-excerpts");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn reader_scrolls_only_visible_cards_and_preserves_each_tab_position() {
    use winit::keyboard::{Key, NamedKey};
    let dir = sample_folder("reader-long-shelf");
    let mut app = blank_app(false);
    app.store.state_path = Some(dir.join("state.json"));
    press(&mut app, Target::Home);
    app.reader.articles = (0..4_000).map(TestArticle::example_card).collect();
    paint(&mut app);
    let hits = |app: &App| {
        app.hits
            .reader
            .iter()
            .filter_map(|(rect, target)| {
                if let Target::Article(index) = *target {
                    Some((*rect, index))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
    };
    let first = hits(&app);
    assert!(!first.is_empty() && first.len() < 64);
    assert_eq!(first[0].1, 0);
    let (target, track, _, _) = app.card_grid_scrollbar().unwrap();
    let origin = (track.x + track.w * 0.5, track.y + 16.0);
    assert_eq!(app.scroll_target_at(origin.0, origin.1), target);
    assert_eq!(
        app.hover_at(track.right() - 4.0, track.y + 12.0),
        Hover::Scrollbar(Scrollbar::CardGrid)
    );
    app.scroll_target_by(target, origin, false, (0.0, -track.h * 3.0));
    paint(&mut app);
    let offset = app.pane_scroll(target).unwrap();
    let after_wheel = hits(&app);
    assert!(!after_wheel.is_empty() && after_wheel.len() < 64);
    assert!(after_wheel[0].1 > 0);

    press(&mut app, Target::Saved);
    assert_eq!(app.pane_scroll(app.reader_scroll_target()), Some(0.0));
    app.scroll_target_by(target, origin, false, (0.0, -track.h));
    assert_eq!(
        app.pane_scroll(app.reader_scroll_target()),
        Some(0.0),
        "old momentum must not scroll a different tab"
    );
    assert!(app.card_grid_key(&Key::Named(NamedKey::End), false));
    paint(&mut app);
    assert_eq!(hits(&app).last().unwrap().1, 3_999);
    press(&mut app, Target::Overview);
    assert_eq!(app.pane_scroll(app.reader_scroll_target()), Some(offset));
    paint(&mut app);
    assert_eq!(hits(&app), after_wheel);
    assert!(!app.reader.articles[0].card_lines.is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_pasted_link_resumes_offline_and_quote_survives_removal() {
    let Some(site) = Site::start() else {
        return;
    };
    let dir = sample_folder("reader-paste-resume-quote");
    let state = dir.join("state.json");
    let url = site.url("/article");
    let mut app = blank_app(false);
    app.store.state_path = Some(state.clone());
    assert!(app.editor.root().is_none());
    press(&mut app, Target::Home);
    press(&mut app, Target::Add);
    assert_eq!(app.bars.prompt_kind, PromptKind::ReaderUrl);
    submit(&mut app, &url);
    settle(&mut app);
    assert!(app.reader_full());
    assert!(app.editor.active().unwrap().readonly);
    let id = app.reader_key().unwrap().to_owned();
    let archive = dir.join("reader/articles").join(&id);
    let md = std::fs::read_to_string(archive.join("body.md")).unwrap();
    assert!(md.contains("# Field Notes"));
    assert!(md.contains("```rust\nlet x = 42;"));
    assert!(md.contains("missing chart"));
    assert!(md.contains(winc_i18n::t!("reader-missing-image").trim_matches(['[', ']'])));
    assert!(!md.contains("Not the article"));
    assert!(
        std::fs::read_dir(&archive)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("image-"))
    );
    app.preview.markdown.scroll_to(1110.0);
    settle(&mut app);
    let scroll = app.preview.markdown.scroll();
    assert!(scroll >= 1000.0);
    app.remember_session();
    drop(app);
    drop(site);

    let mut offline = blank_app(false);
    offline.store.state_path = Some(state);
    press(&mut offline, Target::Home);
    assert_eq!(offline.reader.articles.len(), 1);
    assert!(!offline.reader.articles[0].card_lines.is_empty());
    press(&mut offline, Target::Article(0));
    settle(&mut offline);
    assert!(offline.reader_full());
    assert!(
        offline
            .hits
            .reader
            .iter()
            .any(|(_, t)| *t == Target::RemoveCurrent)
    );
    assert!((offline.preview.markdown.scroll() - scroll).abs() < 2.0);
    assert!(
        offline
            .preview
            .markdown
            .images
            .values()
            .any(|image| matches!(image, winc_typeset::ImageSlot::Ready { .. }))
    );

    // Quote to a new ordinary document, using the real selection/menu and Save As paths.
    select_for_quote(&mut offline);
    assert!(menu_commands(&offline).contains(&(MenuCommand::ReaderDraft(None), true)));
    offline.run_menu_command(MenuCommand::ReaderDraft(None));
    let draft = offline.editor.active().unwrap().id;
    let path = dir.join("draft.md");
    offline.on_picked(Pick::SaveAs(draft), Some(path.clone()));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("> These are the words I want to quote"));
    assert!(text.contains(&format!("<{url}>")));
    assert!(!text.contains(&id));

    // Quote to an already-open draft that has other content; the reader stays put.
    offline.activate_tab(
        offline
            .editor
            .documents
            .iter()
            .position(|d| matches!(&d.reading, Some(winc_editor::Reading::Article { .. })))
            .unwrap(),
    );
    settle(&mut offline);
    assert!((offline.preview.markdown.scroll() - scroll).abs() < 2.0);
    select_for_quote(&mut offline);
    assert!(menu_commands(&offline).contains(&(MenuCommand::ReaderDraft(Some(draft)), true)));
    offline.run_menu_command(MenuCommand::ReaderDraft(Some(draft)));
    let updated = offline
        .editor
        .active()
        .unwrap()
        .text()
        .rope()
        .unwrap()
        .to_string();
    assert!(updated.starts_with(&text));
    assert_eq!(
        updated
            .matches("These are the words I want to quote")
            .count(),
        2
    );
    offline.save_active();
    offline.remove_reader_article(id);
    assert!(!archive.exists());
    assert_eq!(std::fs::read_to_string(path).unwrap(), updated);
    assert!(!offline.editor.active().unwrap().readonly);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn reading_full_text_from_a_feed_does_not_bookmark_and_failure_keeps_the_excerpt() {
    let Some(site) = Site::start() else {
        return;
    };
    let dir = sample_folder("reader-feed-excerpt");
    let mut app = blank_app(false);
    app.store.state_path = Some(dir.join("state.json"));
    press(&mut app, Target::Home);
    press(&mut app, Target::Subscriptions);
    press(&mut app, Target::Subscribe);
    submit(&mut app, &site.url("/excerpt-feed"));
    press(&mut app, Target::Feed(0));
    let entries = &app.reader.subscriptions.feeds[0].entries;
    assert!(!entries[0].card_lines.is_empty());
    assert!(entries[1].card_lines.is_empty());
    press(&mut app, Target::FeedItem(0));
    let before = app.editor.active().unwrap().reading.clone();
    press(&mut app, Target::ReadFull);
    assert_eq!(app.editor.active().unwrap().reading, before);
    assert_eq!(app.notice.kind, NoticeKind::Warning);
    assert!(app.reader.articles.is_empty());
    press(&mut app, Target::Back);
    press(&mut app, Target::FeedItem(1));
    let Some(winc_editor::Reading::FeedEntry {
        markdown,
        full: false,
        ..
    }) = &app.editor.active().unwrap().reading
    else {
        panic!("title-only entry")
    };
    assert_eq!(markdown.matches("Story without a preview").count(), 1);
    press(&mut app, Target::ReadFull);
    assert!(app.reader_full() && app.reader_feed_entry());
    let Some(winc_editor::Reading::FeedEntry { markdown, .. }) =
        &app.editor.active().unwrap().reading
    else {
        panic!("web article")
    };
    assert!(markdown.contains("These are the words I want to quote."));
    assert!(markdown.contains(&format!("![cover]({})", site.url("/blog/cover.gif"))));
    assert!(!app.reader.original_excerpts[&site.url("/article")].is_empty());
    settle(&mut app);
    assert_reader_online_image(&app, &site.url("/blog/cover.gif"), false);
    assert!(app.reader.articles.is_empty());
    assert!(!dir.join("reader/articles").exists());
    press(&mut app, Target::Bookmark);
    assert!(app.reader_full() && !app.reader_feed_entry());
    assert_eq!(app.reader.articles.len(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

fn assert_reader_online_image(app: &App, url: &str, drawn: bool) {
    use crate::app::preview::markdown::image_path;
    use winc_typeset::ImageSlot;
    let cached = image_path(None, url, true).unwrap();
    match app.preview.markdown.images.get(&cached) {
        Some(ImageSlot::Ready { pixels, .. }) if !drawn || !pixels.is_empty() => {}
        Some(ImageSlot::Ready { .. }) => panic!("image decoded but not drawn: {url}"),
        Some(ImageSlot::Loading) => panic!("image request did not finish: {url}"),
        Some(ImageSlot::Failed) => panic!("image download or decoding failed: {url}"),
        None => panic!("no image was requested: {url}"),
    }
    let _ = std::fs::remove_file(cached);
}

#[test]
fn reader_renders_feed_images_without_opening_the_original_or_saving() {
    use crate::layout::Setting;
    let Some(site) = Site::start() else {
        return;
    };
    let dir = sample_folder("reader-feed-images");
    let mut app = blank_app(false);
    app.store.state_path = Some(dir.join("state.json"));
    app.config.markdown_remote_images = Some(false);
    press(&mut app, Target::Home);
    press(&mut app, Target::Subscribe);
    submit(&mut app, &site.url("/feed"));
    press(&mut app, Target::Feed(0));
    press(&mut app, Target::FeedItem(0));
    settle(&mut app);
    assert!(app.reader_full() && app.reader_feed_entry());
    assert!(app.preview.markdown.images.is_empty());
    app.settings_toggle(Setting::RemoteImages);
    settle(&mut app);
    assert_reader_online_image(&app, &site.url("/blog/cover.gif"), true);
    assert!(app.reader.articles.is_empty());
    assert!(!dir.join("reader/articles").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn subscriptions_keep_cache_on_failure_and_saved_articles_on_unsubscribe() {
    let Some(site) = Site::start() else {
        return;
    };
    let dir = sample_folder("reader-subscriptions");
    let mut app = blank_app(false);
    app.store.state_path = Some(dir.join("state.json"));
    press(&mut app, Target::Home);
    press(&mut app, Target::Subscriptions);
    press(&mut app, Target::Subscribe);
    submit(&mut app, &site.url("/feed"));
    assert_eq!(app.reader.subscriptions.feeds[0].entries.len(), 2);
    let cache = dir
        .join("reader/feeds")
        .join(format!("{}.json", app.reader.subscriptions.feeds[0].key));
    let stamp = std::time::UNIX_EPOCH + std::time::Duration::from_secs(10_000);
    std::fs::File::options()
        .write(true)
        .open(&cache)
        .unwrap()
        .set_modified(stamp)
        .unwrap();
    let original_stamp = std::fs::metadata(&cache).unwrap().modified().unwrap();
    let start = std::time::Instant::now();
    press(&mut app, Target::RefreshFeeds);
    let until = app.reader_tick(start).unwrap();
    assert!(until >= start + std::time::Duration::from_millis(500));
    assert!(app.reader.subscriptions.refreshing());
    assert!(app.notice.text.is_none());
    assert_eq!(app.reader.subscriptions.feeds[0].entries.len(), 2);
    assert_eq!(
        std::fs::metadata(&cache).unwrap().modified().unwrap(),
        original_stamp,
        "unchanged feeds must not be written again"
    );
    app.reader_tick(until);
    assert!(!app.reader.subscriptions.refreshing());
    assert_eq!(
        app.notice.text.as_deref(),
        Some(winc_i18n::t!("reader-refreshed"))
    );
    site.broken
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let start = std::time::Instant::now();
    press(&mut app, Target::RefreshFeeds);
    let retry_until = app.reader_tick(start).unwrap();
    assert!(retry_until >= start + std::time::Duration::from_millis(500));
    assert!(app.reader.subscriptions.refreshing());
    app.reader_tick(retry_until);
    assert_eq!(app.notice.kind, NoticeKind::Warning);
    assert_eq!(app.reader.subscriptions.feeds[0].entries.len(), 2);
    // Tick beyond the scheduled refresh, not just the manual button path.
    app.reader_tick(std::time::Instant::now() + std::time::Duration::from_secs(3600));
    assert!(app.reader.subscriptions.feeds[0].failed);
    press(&mut app, Target::Feed(0));
    press(&mut app, Target::UnreadOnly);
    press(&mut app, Target::FeedItem(0));
    assert!(app.reader_feed_entry() && app.reader_full());
    assert_eq!(app.reader.subscriptions.feeds[0].unread, [1]);
    press(&mut app, Target::Bookmark);
    assert!(app.reader_full() && !app.reader_feed_entry());
    press(&mut app, Target::Back);
    paint(&mut app);
    assert!(
        !app.hits
            .reader
            .iter()
            .any(|(_, t)| *t == Target::FeedItem(0))
    );
    press(&mut app, Target::FeedItem(1));
    assert!(app.reader_feed_entry() && !app.reader_full());
    press(&mut app, Target::Bookmark);
    assert!(!app.reader_full() && !app.reader_feed_entry());
    let feed_key = app.reader.subscriptions.feeds[0].key.clone();
    app.remove_reader_feed(feed_key);
    assert!(app.reader.subscriptions.feeds.is_empty());
    assert_eq!(app.reader.articles.len(), 2);
    assert_eq!(app.reader.articles.iter().filter(|a| a.full).count(), 1);
    let _ = std::fs::remove_dir_all(dir);
}
