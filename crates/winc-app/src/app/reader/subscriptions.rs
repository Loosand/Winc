//! Subscription UI state and background jobs. Cached entries remain usable during refresh.

use std::collections::HashSet;
use std::io;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use winc_editor::{DocumentId, Reading};
use winc_i18n::t;

use super::{
    App, Done as ArticleDone, NoticeKind, PromptKind, Target, UserEvent, feed, literal, store,
};
use crate::field::TextField;

const REFRESH_INTERVAL: Duration = Duration::from_secs(30 * 60);

#[derive(Default, PartialEq, Eq)]
pub(crate) enum Page {
    #[default]
    Overview,
    Saved,
    Subscriptions,
    Entries(String),
}

enum Done {
    Cache {
        result: io::Result<Vec<feed::Feed>>,
        bad_subscription: bool,
    },
    Opened {
        from: DocumentId,
        generation: u64,
        ticket: u64,
        title: String,
        reading: Reading,
        read: io::Result<()>,
    },
}

struct ManualRefresh {
    until: Instant,
    only: Option<String>,
    error: Option<String>,
}

pub(crate) struct Subscriptions {
    pub page: Page,
    pub feeds: Vec<feed::Feed>,
    pub unread: bool,
    pub scroll: f32,
    pub generation: u64,
    pub busy: bool,
    open_ticket: u64,
    shelf_scrolls: [f32; 3],
    initialized: bool,
    refresh_at: Option<Instant>,
    manual: Option<ManualRefresh>,
    tx: mpsc::Sender<Done>,
    rx: mpsc::Receiver<Done>,
}

impl Default for Subscriptions {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            page: Page::default(),
            feeds: Vec::new(),
            unread: false,
            scroll: 0.0,
            generation: 0,
            busy: false,
            open_ticket: 0,
            shelf_scrolls: [0.0; 3],
            initialized: false,
            refresh_at: None,
            manual: None,
            tx,
            rx,
        }
    }
}

impl Subscriptions {
    pub(crate) fn refreshing(&self) -> bool {
        self.manual.is_some()
    }

    pub(crate) fn current_feed(&self) -> Option<&feed::Feed> {
        let Page::Entries(key) = &self.page else {
            return None;
        };
        self.feeds.iter().find(|f| &f.key == key)
    }

    fn set_page(&mut self, page: Page) {
        if self.page == page {
            return;
        }
        let shelf = |page: &Page| match page {
            Page::Overview => Some(0),
            Page::Saved => Some(1),
            Page::Subscriptions => Some(2),
            Page::Entries(_) => None,
        };
        if let Some(i) = shelf(&self.page) {
            self.shelf_scrolls[i] = self.scroll;
        }
        self.scroll = shelf(&page).map_or(0.0, |i| self.shelf_scrolls[i]);
        self.page = page;
        self.generation = self.generation.wrapping_add(1);
    }
}

impl App {
    pub(crate) fn reader_feed_entry(&self) -> bool {
        matches!(
            self.editor.active().and_then(|d| d.reading.as_ref()),
            Some(Reading::FeedEntry { .. })
        )
    }

    pub(super) fn press_subscription(&mut self, target: Target) {
        let generation = self.reader.subscriptions.generation;
        match target {
            Target::Overview => self.reader.subscriptions.set_page(Page::Overview),
            Target::Saved => self.reader.subscriptions.set_page(Page::Saved),
            Target::Subscriptions => self.reader.subscriptions.set_page(Page::Subscriptions),
            Target::Subscribe
                if !self.reader.subscriptions.busy && !self.reader.subscriptions.refreshing() =>
            {
                self.open_prompt(PromptKind::ReaderFeedUrl, TextField::default())
            }
            Target::RefreshFeeds => {
                let only = self
                    .reader
                    .subscriptions
                    .current_feed()
                    .map(|f| f.key.clone());
                self.refresh_subscriptions(only, true);
            }
            Target::UnreadOnly => {
                self.reader.subscriptions.unread ^= true;
                self.reader.subscriptions.scroll = 0.0;
                self.reader.subscriptions.generation =
                    self.reader.subscriptions.generation.wrapping_add(1);
            }
            Target::Feed(i) => {
                if let Some(feed) = self.reader.subscriptions.feeds.get(i) {
                    self.reader
                        .subscriptions
                        .set_page(Page::Entries(feed.key.clone()));
                }
            }
            Target::FeedItem(i) => self.open_feed_item(i),
            Target::Unsubscribe
                if !self.reader.subscriptions.busy && !self.reader.subscriptions.refreshing() =>
            {
                if let Some(feed) = self.reader.subscriptions.current_feed() {
                    let key = feed.key.clone();
                    let buttons = vec![
                        super::super::AlertButton::new(t!("reader-unsubscribe")),
                        super::super::AlertButton::cancel(t!("common-cancel")),
                    ];
                    self.show_alert(
                        t!("reader-unsubscribe"),
                        t!("reader-unsubscribe-detail"),
                        buttons,
                        None,
                        move |choice, _| {
                            (choice == 0).then(|| UserEvent::RemoveReaderFeed(key.clone()))
                        },
                    );
                }
            }
            _ => {}
        }
        if generation != self.reader.subscriptions.generation {
            self.hits.reader.clear();
            self.clamp_reader_scroll();
        }
    }

    fn feed_job(
        &mut self,
        network: bool,
        work: impl FnOnce(&std::path::Path) -> io::Result<bool> + Send + 'static,
    ) {
        if self.reader.subscriptions.busy {
            return;
        }
        let Some(root) = self.reader_root() else {
            self.notify(NoticeKind::Error, t!("reader-storage-unavailable"));
            return;
        };
        self.reader.subscriptions.busy = true;
        let tx = self.reader.subscriptions.tx.clone();
        let job = move || {
            let mut bad_subscription = false;
            let result = work(&root).and_then(|valid| {
                bad_subscription = !valid;
                feed::list(&root)
            });
            let _ = tx.send(Done::Cache {
                result,
                bad_subscription,
            });
        };
        if network {
            self.spawn_blocking_then(UserEvent::ReaderFeedsDone, job);
        } else {
            self.spawn_then(UserEvent::ReaderFeedsDone, job);
        }
        self.request_redraw();
    }

    pub(super) fn load_subscriptions(&mut self) {
        if !self.reader.subscriptions.initialized {
            self.reader.subscriptions.initialized = true;
            self.feed_job(false, |_| Ok(true));
        }
    }

    pub(in crate::app) fn subscribe_reader_url(&mut self, raw: &str) {
        let Some(url) = store::article_url(raw) else {
            self.flash(t!("reader-invalid-url"));
            return;
        };
        self.feed_job(true, move |root| {
            if feed::list(root)?.len() >= feed::MAX_FEEDS {
                return Err(io::Error::other(t!("reader-feed-limit")));
            }
            feed::subscribe(root, &url)
        });
    }

    pub(in crate::app) fn poll_subscriptions(&mut self) {
        while let Ok(done) = self.reader.subscriptions.rx.try_recv() {
            let (result, bad_subscription) = match done {
                Done::Cache {
                    result,
                    bad_subscription,
                } => (result, bad_subscription),
                Done::Opened {
                    from,
                    generation,
                    ticket,
                    title,
                    reading,
                    read,
                } => {
                    self.finish_feed_open(from, generation, ticket, title, reading, read);
                    continue;
                }
            };
            let subs = &mut self.reader.subscriptions;
            subs.busy = false;
            match result {
                Ok(mut feeds) => {
                    for feed in &mut feeds {
                        if let Some(previous) = subs.feeds.iter().find(|f| f.key == feed.key) {
                            // A read click might have arrived after the worker read its markers.
                            let mut keys = HashSet::new();
                            let mut urls = HashSet::new();
                            for entry in previous.entries.iter().filter(|e| e.read) {
                                keys.insert(entry.key.as_str());
                                urls.insert(entry.url.as_str());
                            }
                            for entry in &mut feed.entries {
                                entry.read |= keys.contains(entry.key.as_str())
                                    || urls.contains(entry.url.as_str());
                            }
                            feed.index_unread();
                        }
                    }
                    subs.feeds = feeds;
                    self.hits.reader.clear();
                    if matches!(subs.page, Page::Entries(_)) && subs.current_feed().is_none() {
                        subs.set_page(Page::Subscriptions);
                    }
                    if subs.refresh_at.is_none() && !subs.feeds.is_empty() {
                        subs.refresh_at = Some(Instant::now());
                    }
                    self.clamp_reader_scroll();
                }
                Err(e) => {
                    if let Some(manual) = &mut subs.manual {
                        manual.error = Some(e.to_string());
                    } else {
                        self.reader_error(e);
                    }
                }
            }
            if bad_subscription {
                self.notify(NoticeKind::Error, t!("reader-invalid-feed"));
            }
            self.request_redraw();
        }
    }

    fn refresh_subscriptions(&mut self, only: Option<String>, manual: bool) {
        if self.reader.subscriptions.busy
            || self.reader.subscriptions.refreshing()
            || self.reader.subscriptions.feeds.is_empty()
        {
            return;
        }
        if only.is_none() {
            self.reader.subscriptions.refresh_at = Some(Instant::now() + REFRESH_INTERVAL);
        }
        if manual {
            self.reader.subscriptions.manual = Some(ManualRefresh {
                until: Instant::now() + crate::app::workspace::library::MIN_REFRESH,
                only: only.clone(),
                error: None,
            });
        }
        let targets = self
            .reader
            .subscriptions
            .feeds
            .iter()
            .filter(|feed| only.as_ref().is_none_or(|key| key == &feed.key))
            .map(|feed| (feed.key.clone(), feed.url.clone()))
            .collect::<Vec<_>>();
        self.feed_job(true, move |root| {
            feed::refresh(root, &targets).map(|()| true)
        });
    }

    pub(super) fn subscription_tick(&mut self, now: Instant) -> Option<Instant> {
        let subs = &mut self.reader.subscriptions;
        if let Some(manual) = &subs.manual {
            if now < manual.until {
                return Some(manual.until);
            }
            if subs.busy {
                return None;
            }
            let manual = subs.manual.take().expect("manual refresh");
            let failed = subs
                .feeds
                .iter()
                .filter(|f| f.failed && manual.only.as_ref().is_none_or(|id| id == &f.key))
                .count();
            if let Some(error) = manual.error {
                self.reader_error(error);
            } else if failed > 0 {
                self.notify(
                    NoticeKind::Warning,
                    t!("reader-refresh-failed-count", count = failed),
                );
            } else {
                self.notify(NoticeKind::Done, t!("reader-refreshed"));
            }
            self.request_redraw();
        }
        let subs = &self.reader.subscriptions;
        if subs.busy || subs.feeds.is_empty() {
            return None;
        }
        if subs.refresh_at.is_some_and(|at| now >= at) {
            self.refresh_subscriptions(None, false);
        }
        self.reader.subscriptions.refresh_at
    }

    pub(in crate::app) fn remove_reader_feed(&mut self, key: String) {
        if !store::valid_key(&key) {
            return;
        }
        self.feed_job(false, move |root| {
            feed::unsubscribe(root, &key).map(|()| true)
        });
    }

    fn open_feed_item(&mut self, index: usize) {
        let Some(feed) = self.reader.subscriptions.current_feed() else {
            return;
        };
        let Some(entry) = feed.entries.get(index).cloned() else {
            return;
        };
        if store::http_url(&entry.url).is_none() {
            return;
        }
        let feed_key = feed.key.clone();
        let key = format!("{feed_key}:{}", entry.key);
        let subs = &mut self.reader.subscriptions;
        subs.open_ticket = subs.open_ticket.wrapping_add(1);
        let (generation, ticket) = (subs.generation, subs.open_ticket);
        if entry.read
            && let Some(i) = self.feed_tab(&key)
        {
            self.activate_tab(i);
            return;
        }
        let Some(from) = self.editor.active().map(|doc| doc.id) else {
            return;
        };
        let root = self.reader_root();
        let tx = self.reader.subscriptions.tx.clone();
        let label = entry.content_label();
        let (missing, no_excerpt, original) = (
            t!("reader-image-unavailable"),
            t!("reader-feed-no-excerpt-detail"),
            t!("reader-source"),
        );
        self.spawn_then(UserEvent::ReaderFeedsDone, move || {
            let read = match root {
                Some(root) if !entry.read => feed::mark_read(&root, &feed_key, &entry),
                _ => Ok(()),
            };
            // Keep remote destinations for the Markdown preview; do not fetch any images here.
            let body = entry.online_markdown(missing);
            let full = entry.full && !body.is_empty();
            let title = literal(&entry.title);
            let same_as_title = |s: &str| s.split_whitespace().eq(title.split_whitespace());
            let title_only =
                same_as_title(&body) || body.trim().strip_prefix("# ").is_some_and(same_as_title);
            let source = &entry.url;
            let markdown = if body.trim().is_empty() || (!full && title_only) {
                format!("# {title}\n\n{no_excerpt}\n\n{original}: <{source}>\n")
            } else {
                format!("# {title}\n\n{label}\n\n{body}\n\n{original}: <{source}>\n")
            };
            let reading = Reading::FeedEntry {
                key,
                image_page: entry.url.clone(),
                source: entry.url,
                markdown: markdown.into(),
                full,
            };
            let _ = tx.send(Done::Opened {
                from,
                generation,
                ticket,
                title: entry.title,
                reading,
                read,
            });
        });
    }

    fn feed_tab(&self, key: &str) -> Option<usize> {
        self.editor.documents.iter().position(
            |doc| matches!(&doc.reading, Some(Reading::FeedEntry { key: k, .. }) if k == key),
        )
    }

    fn finish_feed_open(
        &mut self,
        from: DocumentId,
        generation: u64,
        ticket: u64,
        title: String,
        reading: Reading,
        read: io::Result<()>,
    ) {
        let Reading::FeedEntry { key, source, .. } = &reading else {
            return;
        };
        let Some((feed_id, entry_id)) = key.split_once(':') else {
            return;
        };
        match read {
            Ok(()) => {
                if let Some(feed) = self
                    .reader
                    .subscriptions
                    .feeds
                    .iter_mut()
                    .find(|f| f.key == feed_id)
                    && let Some(entry) = feed
                        .entries
                        .iter_mut()
                        .find(|entry| entry.key == entry_id || &entry.url == source)
                    && !entry.read
                {
                    entry.read = true;
                    feed.index_unread();
                    self.hits.reader.clear();
                }
            }
            Err(e) => self.reader_error(e),
        }
        self.clamp_reader_scroll();
        let subs = &self.reader.subscriptions;
        if subs.generation == generation
            && subs.open_ticket == ticket
            && self.editor.active().is_some_and(|doc| doc.id == from)
        {
            let i = self
                .feed_tab(key)
                .unwrap_or_else(|| self.editor.open_reading(&title, reading));
            self.activate_tab(i);
        }
        self.request_redraw();
    }

    pub(super) fn bookmark_feed_entry(&mut self) {
        let Some(Reading::FeedEntry { key, source, .. }) =
            self.editor.active().and_then(|d| d.reading.as_ref())
        else {
            return;
        };
        let Some((feed, item)) = key.split_once(':') else {
            return;
        };
        let entry = self
            .reader
            .subscriptions
            .feeds
            .iter()
            .find(|f| f.key == feed)
            .and_then(|f| f.entries.iter().find(|e| e.key == item))
            .cloned();
        let Some(entry) = entry else {
            self.import_reader_url(&source.clone());
            return;
        };
        let from = self.editor.active().map(|d| d.id);
        let missing = t!("reader-missing-image");
        self.reader_job(true, move |root: PathBuf| {
            ArticleDone::imported(
                store::save_entry(&root, &entry, missing, crate::web::fetch_http_with_referer),
                from,
            )
        });
    }
}
