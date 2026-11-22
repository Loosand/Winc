//! Read-later workspace and its bridge into the ordinary Markdown editor.

mod feed;
mod store;
pub(crate) mod subscriptions;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use winc_core::Lines;
use winc_editor::{Command, Document, DocumentId, PreviewKind, Reading};
use winc_i18n::t;
use winc_render::Rect;

use super::scroll::ScrollTarget;
use super::workspace::library::PreviewLine;
use super::{App, NoticeKind, PromptKind, UserEvent};
use crate::field::TextField;
#[cfg(test)]
pub(in crate::app) use store::Article as TestArticle;
use store::{Article, Place};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    Home,
    Back,
    Add,
    Article(usize),
    Remove(usize),
    RemoveCurrent,
    Source,
    Retry,
    ReadFull,
    Quote,
    Overview,
    Saved,
    Subscriptions,
    Subscribe,
    RefreshFeeds,
    UnreadOnly,
    Feed(usize),
    FeedItem(usize),
    Unsubscribe,
    Bookmark,
}

enum Done {
    Listed(std::io::Result<Vec<Article>>),
    Imported(std::io::Result<Article>, Option<DocumentId>),
    Opened(Option<(Article, Option<String>, Place)>, DocumentId),
    FullText(Result<store::WebText, crate::web::HttpFailure>, DocumentId),
    Removed(std::io::Result<()>, String),
}

impl Done {
    fn imported(result: std::io::Result<Article>, from: Option<DocumentId>) -> Self {
        Self::Imported(
            result.map(|mut article| {
                article.prepare_card();
                article
            }),
            from,
        )
    }
}

pub(crate) struct Reader {
    pub articles: Vec<Article>,
    pub original_excerpts: HashMap<String, Vec<PreviewLine>>,
    pub busy: bool,
    pub subscriptions: subscriptions::Subscriptions,
    sender: mpsc::Sender<Done>,
    receiver: mpsc::Receiver<Done>,
    shelf_loaded: Option<Instant>,
    places: HashMap<String, Place>,
    dirty: HashMap<String, Place>,
    flush_at: Option<Instant>,
}

impl Default for Reader {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            articles: Vec::new(),
            original_excerpts: HashMap::new(),
            busy: false,
            subscriptions: Default::default(),
            sender,
            receiver,
            shelf_loaded: None,
            places: HashMap::new(),
            dirty: HashMap::new(),
            flush_at: None,
        }
    }
}

impl App {
    pub(crate) fn reader_home(&self) -> bool {
        self.editor
            .active()
            .is_some_and(|d| d.reading == Some(Reading::Reader))
    }

    pub(crate) fn reader_sidebar_button(&self) -> Rect {
        let home = self.library_home_button();
        if self.notebook_sidebar() {
            return self.panels.sidebar.navigation.reader;
        }
        let row = super::workspace::library::HOME_ROW;
        Rect::new(home.x, home.y + row, home.w, home.h).intersect(self.layout.sidebar)
    }

    pub(crate) fn reader_active(&self) -> bool {
        self.reader_home() || self.reader_story()
    }

    pub(crate) fn reader_key(&self) -> Option<&str> {
        let Reading::Article { key, .. } = self.editor.active()?.reading.as_ref()? else {
            return None;
        };
        Some(key)
    }

    pub(crate) fn reader_story(&self) -> bool {
        self.reader_key().is_some() || self.reader_feed_entry()
    }

    pub(crate) fn reader_full(&self) -> bool {
        matches!(
            self.editor.active().and_then(|d| d.reading.as_ref()),
            Some(Reading::Article { full: true, .. } | Reading::FeedEntry { full: true, .. })
        )
    }

    fn reader_source(&self) -> Option<&str> {
        if let Reading::FeedEntry { source, .. } = self.editor.active()?.reading.as_ref()? {
            Some(source)
        } else {
            self.reader_article().map(|a| a.url.as_str())
        }
    }

    fn reader_article(&self) -> Option<&Article> {
        let key = self.reader_key()?;
        self.reader.articles.iter().find(|a| a.key == key)
    }

    fn reader_root(&self) -> Option<PathBuf> {
        let path = self
            .store
            .state_path
            .clone()
            .or_else(winc_config::state_path)?;
        Some(path.parent()?.join("reader"))
    }

    pub(crate) fn reader_body_area(&self) -> Rect {
        let r = self.layout.editor;
        let header = 44.0_f32.min(r.h);
        Rect::new(r.x, r.y + header, r.w, r.h - header)
    }

    pub(in crate::app) fn reader_image_dir(&self) -> Option<PathBuf> {
        let key = self.reader_key().filter(|key| store::valid_key(key))?;
        Some(store::dir(&self.reader_root()?, key))
    }

    fn reader_error(&mut self, error: impl ToString) {
        self.notify(
            NoticeKind::Error,
            t!("reader-storage-error", error = error.to_string()),
        );
    }

    fn reader_job(&mut self, network: bool, job: impl FnOnce(PathBuf) -> Done + Send + 'static) {
        if self.reader.busy {
            return;
        }
        let Some(root) = self.reader_root() else {
            self.notify(NoticeKind::Error, t!("reader-storage-unavailable"));
            return;
        };
        self.reader.busy = true;
        let sender = self.reader.sender.clone();
        let job = move || {
            let _ = sender.send(job(root));
        };
        if network {
            self.spawn_blocking_then(UserEvent::ReaderDone, job);
        } else {
            self.spawn_then(UserEvent::ReaderDone, job);
        }
        self.request_redraw();
    }

    fn show_reader(&mut self, from_sidebar: bool) {
        if !self.reader_home() {
            let index = self
                .editor
                .documents
                .iter()
                .position(|d| d.reading == Some(Reading::Reader));
            let keep = index.map(|index| self.editor.documents[index].id);
            if !self.prepare_writing_switch(keep) {
                return;
            }
            let index = keep
                .and_then(|id| self.editor.index_of(id))
                .unwrap_or_else(|| {
                    self.editor
                        .open_reading(t!("reader-title"), Reading::Reader)
                });
            self.activate_tab(index);
        }
        self.load_subscriptions();
        if self
            .reader
            .shelf_loaded
            .is_none_or(|at| from_sidebar && at.elapsed() >= Duration::from_secs(5))
        {
            self.reader_job(false, |root| Done::Listed(store::list(&root)));
        }
    }

    pub(in crate::app) fn press_reader(&mut self, target: Target, at: (f32, f32)) {
        match target {
            Target::Home => self.show_reader(true),
            Target::Back => self.show_reader(false),
            Target::Add if self.reader_home() && !self.reader.busy => {
                self.open_prompt(PromptKind::ReaderUrl, TextField::default());
            }
            Target::Article(i) if self.reader_home() => {
                if let Some(article) = self.reader.articles.get(i) {
                    self.open_reader_article(article.key.clone());
                }
            }
            Target::Remove(i) if self.reader_home() && !self.reader.busy => {
                if let Some(article) = self.reader.articles.get(i) {
                    self.confirm_remove_article(article.key.clone());
                }
            }
            Target::RemoveCurrent if !self.reader.busy => {
                if let Some(key) = self.reader_key().map(str::to_owned) {
                    self.confirm_remove_article(key);
                }
            }
            Target::Retry if !self.reader.busy => {
                if let Some(url) = self.reader_article().map(|a| a.url.clone()) {
                    self.import_reader_url(&url);
                }
            }
            Target::Source => {
                if let Some(source) = self.reader_source()
                    && let Err(error) = crate::system::open_url(source)
                {
                    self.flash(t!(
                        "preview-markdown-link-open-failed",
                        error = error.to_string()
                    ));
                }
            }
            Target::ReadFull => self.read_feed_original(),
            Target::Quote if self.reader_full() => self.choose_reader_draft(at),
            Target::Bookmark if self.reader_feed_entry() && !self.reader.busy => {
                self.bookmark_feed_entry()
            }
            target if self.reader_home() => self.press_subscription(target),
            _ => {}
        }
        self.request_redraw();
    }

    fn confirm_remove_article(&mut self, key: String) {
        let buttons = vec![
            super::AlertButton::new(t!("reader-remove")),
            super::AlertButton::cancel(t!("common-cancel")),
        ];
        self.show_alert(
            t!("reader-remove"),
            t!("reader-remove-detail"),
            buttons,
            None,
            move |choice, _| (choice == 0).then(|| UserEvent::RemoveReaderArticle(key.clone())),
        );
    }

    pub(crate) fn reader_toolbar_inline(&self) -> bool {
        self.layout.editor.w >= 820.0
    }

    pub(crate) fn reader_scroll_target(&self) -> ScrollTarget {
        ScrollTarget::Reader(self.reader.subscriptions.generation)
    }

    pub(in crate::app) fn clamp_reader_scroll(&mut self) {
        if !self.reader_home() {
            return;
        }
        let target = self.reader_scroll_target();
        let limit = self.reader_grid().max_scroll(self.reader_card_count());
        let scroll = self.reader.subscriptions.scroll;
        if scroll > limit {
            self.settle_glide_of(target);
            self.reader.subscriptions.scroll = limit;
        }
    }

    pub(crate) fn reader_card_count(&self) -> usize {
        use subscriptions::Page;
        let sub = &self.reader.subscriptions;
        match &sub.page {
            Page::Overview => sub.feeds.len() + self.reader.articles.len(),
            Page::Saved => self.reader.articles.len(),
            Page::Subscriptions => sub.feeds.len(),
            Page::Entries(_) => sub.current_feed().map_or(0, |feed| {
                if sub.unread {
                    feed.unread.len()
                } else {
                    feed.entries.len()
                }
            }),
        }
    }

    pub(crate) fn reader_card(&self, index: usize) -> Option<Target> {
        use subscriptions::Page;
        let sub = &self.reader.subscriptions;
        match &sub.page {
            Page::Overview if index >= sub.feeds.len() => {
                Some(Target::Article(index - sub.feeds.len()))
            }
            Page::Overview | Page::Subscriptions => Some(Target::Feed(index)),
            Page::Saved => Some(Target::Article(index)),
            Page::Entries(_) if !sub.unread => Some(Target::FeedItem(index)),
            Page::Entries(_) => Some(Target::FeedItem(*sub.current_feed()?.unread.get(index)?)),
        }
    }

    pub(crate) fn reader_grid(&self) -> super::workspace::library::Grid {
        use super::workspace::library::Grid;
        use subscriptions::Page;
        let r = self.layout.editor;
        let folders = matches!(
            self.reader.subscriptions.page,
            Page::Overview | Page::Subscriptions
        ) && !self.reader.subscriptions.feeds.is_empty();
        let extra = if self.reader_toolbar_inline() {
            0.0
        } else {
            25.0
        };
        Grid::for_cards(
            Rect::new(r.x, r.y + extra, r.w, (r.h - extra).max(0.0)),
            folders,
        )
        .fit_single_column(folders)
    }

    pub(in crate::app) fn import_reader_url(&mut self, raw: &str) {
        let Some(url) = store::article_url(raw) else {
            self.flash(t!("reader-invalid-url"));
            return;
        };
        let from = self.editor.active().map(|d| d.id);
        let missing = t!("reader-missing-image");
        self.reader_job(true, move |root| {
            Done::imported(
                store::import(&root, &url, missing, crate::web::fetch_http_with_referer),
                from,
            )
        });
    }

    fn read_feed_original(&mut self) {
        if self.reader.busy {
            return;
        }
        let Some(doc) = self.editor.active() else {
            return;
        };
        let Some(Reading::FeedEntry {
            source,
            full: false,
            ..
        }) = doc.reading.as_ref()
        else {
            return;
        };
        let Some(url) = store::article_url(source) else {
            return;
        };
        let id = doc.id;
        let missing = t!("reader-image-unavailable");
        self.reader_job(true, move |root| {
            if let Some(saved @ (_, Some(_), _)) = store::open(&root, &store::key(&url)) {
                return Done::Opened(Some(saved), id);
            }
            Done::FullText(
                store::read_full(&url, missing, crate::web::fetch_http_with_referer),
                id,
            )
        });
    }

    fn apply_feed_original(
        &mut self,
        id: DocumentId,
        result: Result<store::WebText, crate::web::HttpFailure>,
    ) {
        let Some(doc) = self.editor.document(id) else {
            return;
        };
        let Some(Reading::FeedEntry {
            key,
            source,
            full: false,
            ..
        }) = doc.reading.as_ref()
        else {
            return;
        };
        match result {
            Ok(store::WebText {
                base,
                mut markdown,
                excerpt,
            }) => {
                if !excerpt.is_empty() {
                    self.reader
                        .original_excerpts
                        .insert(source.clone(), excerpt);
                }
                if !markdown.starts_with("# ") {
                    markdown = format!("# {}\n\n{markdown}", literal(&doc.name()));
                }
                markdown.push_str(&format!("\n\n{}: <{source}>\n", t!("reader-source")));
                let reading = Reading::FeedEntry {
                    key: key.clone(),
                    source: source.clone(),
                    image_page: base.to_string(),
                    full: true,
                    markdown: markdown.into(),
                };
                self.editor.update_reading(id, reading);
            }
            Err(failure) if self.editor.active().is_some_and(|doc| doc.id == id) => {
                self.notify(
                    NoticeKind::Warning,
                    t!("reader-full-failed", error = failure.message()),
                );
            }
            Err(_) => {}
        }
    }

    fn open_reader_article(&mut self, key: String) {
        let Some(from) = self.editor.active().map(|d| d.id) else {
            return;
        };
        if let Some(i) = self.reader_tab(&key) {
            self.activate_tab(i);
        } else {
            self.read_reader_article(key, from);
        }
    }

    fn read_reader_article(&mut self, key: String, from: DocumentId) {
        self.reader_job(false, move |root| {
            Done::Opened(store::open(&root, &key), from)
        });
    }

    pub(in crate::app) fn remove_reader_article(&mut self, key: String) {
        if store::valid_key(&key) {
            self.reader_job(false, move |root| {
                Done::Removed(store::remove(&root, &key), key)
            });
        }
    }

    fn reader_tab(&self, key: &str) -> Option<usize> {
        self.editor
            .documents
            .iter()
            .position(|d| matches!(&d.reading, Some(Reading::Article { key: k, .. }) if k == key))
    }

    pub(super) fn poll_reader(&mut self) {
        while let Ok(done) = self.reader.receiver.try_recv() {
            self.reader.busy = false;
            match done {
                Done::Listed(Ok(articles)) => {
                    self.reader.articles = articles;
                    self.reader.shelf_loaded = Some(Instant::now());
                    self.hits.reader.clear();
                    self.clamp_reader_scroll();
                }
                Done::Imported(Ok(article), from) => {
                    let key = article.key.clone();
                    let full = article.full;
                    if let Some(a) = self.reader.articles.iter_mut().find(|a| a.key == key) {
                        *a = article;
                    } else {
                        self.reader.articles.push(article);
                        store::sort(&mut self.reader.articles);
                    }
                    self.hits.reader.clear();
                    if !full {
                        self.flash(t!("reader-link-saved"));
                    }
                    if let Some(from) =
                        from.filter(|id| self.editor.active().is_some_and(|d| d.id == *id))
                    {
                        self.read_reader_article(key, from);
                    }
                }
                Done::Opened(Some((article, body, place)), from) => {
                    if self.editor.active().is_some_and(|d| d.id == from) {
                        self.show_article(article, body, place);
                    }
                }
                Done::Opened(None, _) => self.flash(t!("reader-article-not-found")),
                Done::FullText(result, id) => self.apply_feed_original(id, result),
                Done::Removed(Ok(()), key) => {
                    self.reader.articles.retain(|a| a.key != key);
                    self.hits.reader.clear();
                    self.clamp_reader_scroll();
                    self.reader.places.remove(&key);
                    self.reader.dirty.remove(&key);
                    if let Some(i) = self.reader_tab(&key) {
                        self.close_documents(&[self.editor.documents[i].id]);
                    }
                }
                Done::Listed(Err(e)) | Done::Imported(Err(e), _) | Done::Removed(Err(e), _) => {
                    self.reader_error(e)
                }
            }
            self.request_redraw();
        }
    }

    fn show_article(&mut self, article: Article, body: Option<String>, place: Place) {
        let keep = self
            .reader_tab(&article.key)
            .map(|i| self.editor.documents[i].id);
        if !self.prepare_writing_switch(keep) {
            return;
        }
        let full = body.is_some();
        let mut text = String::new();
        if body.as_ref().is_none_or(|s| !s.starts_with("# ")) {
            let title = if article.title == article.url {
                store::http_url(&article.url)
                    .and_then(|u| u.host_str().map(str::to_owned))
                    .unwrap_or_else(|| article.title.clone())
            } else {
                article.title.clone()
            };
            text = format!("# {}\n\n", literal(&title));
        }
        if let Some(body) = body {
            text.push_str(&body);
        } else {
            text.push_str(t!("reader-link-only-detail"));
            if let Some(failure) = article.failure {
                text.push_str("\n\n");
                text.push_str(&literal(&failure.message()));
            }
        }
        text.push_str(&format!("\n\n{}: <{}>\n", t!("reader-source"), article.url));
        self.reader
            .places
            .entry(article.key.clone())
            .or_insert(place);
        let reading = Reading::Article {
            key: article.key.clone(),
            markdown: text.into(),
            full,
        };
        let i = if let Some(i) = self.reader_tab(&article.key) {
            let id = self.editor.documents[i].id;
            self.editor.documents[i].title = Some(article.title);
            self.editor.update_reading(id, reading);
            i
        } else {
            self.editor.open_reading(&article.title, reading)
        };
        self.activate_tab(i);
    }

    pub(in crate::app) fn reader_place(&self) -> Option<Place> {
        self.reader.places.get(self.reader_key()?).copied()
    }

    pub(crate) fn track_reader_place(&mut self, place: Place) {
        if self.panes.drawing_background || !self.reader_full() {
            return;
        }
        let Some(key) = self.reader_key() else {
            return;
        };
        let place = (place.0, (place.1.clamp(0.0, 1.0) * 1000.0).round() / 1000.0);
        if self.reader.places.get(key) == Some(&place) {
            return;
        }
        let key = key.to_owned();
        self.reader.places.insert(key.clone(), place);
        self.reader.dirty.insert(key, place);
        self.reader.flush_at = Some(Instant::now() + Duration::from_millis(600));
    }

    pub(in crate::app) fn reader_tick(&mut self, now: Instant) -> Option<Instant> {
        if self.reader.flush_at.is_some_and(|at| now >= at) {
            self.flush_reader_positions();
        }
        self.reader
            .flush_at
            .into_iter()
            .chain(self.subscription_tick(now))
            .min()
    }

    pub(in crate::app) fn flush_reader_positions(&mut self) {
        self.reader.flush_at = None;
        let dirty = std::mem::take(&mut self.reader.dirty);
        if let Some(root) = self.reader_root() {
            let mut error = None;
            for (id, place) in dirty {
                if let Err(e) = store::set_place(&root, &id, place) {
                    self.reader.dirty.insert(id, place);
                    error.get_or_insert(e);
                }
            }
            if let Some(e) = error {
                self.reader_error(e);
            }
        }
    }

    fn choose_reader_draft(&mut self, at: (f32, f32)) {
        if self.preview.markdown.doc != self.editor.active().map(|d| d.id) {
            return;
        }
        let Some(text) = self.markdown_selected_text() else {
            self.flash(t!("reader-select-quote"));
            return;
        };
        let Some(source) = self.reader_source() else {
            return;
        };
        let Some(quote) = quote_markdown(&text, source) else {
            return;
        };
        self.open_context_menu(
            super::input::context_menu::MenuTarget::ReaderQuote(quote),
            at,
        );
    }

    pub(in crate::app) fn append_reader_quote(&mut self, id: Option<DocumentId>, quote: String) {
        if let Some(id) = id {
            let Some(i) = self
                .editor
                .index_of(id)
                .filter(|&i| draft_target(&self.editor.documents[i]))
            else {
                self.flash(t!("reader-draft-unavailable"));
                return;
            };
            self.activate_tab(i);
        } else {
            if self.editor.all_locked() {
                self.flash(t!("reader-draft-unavailable"));
                return;
            }
            self.new_file();
        }
        let Some(doc) = self.editor.active_mut() else {
            return;
        };
        let end = doc.text().end_pos();
        doc.set_selections(vec![winc_core::Selection::point(end)], 0);
        self.select(end, end);
        let gap = if end == 0 { "" } else { "\n\n" };
        self.run(Command::Paste(format!("{gap}{quote}")));
    }
}

pub(in crate::app) use store::image_path;

pub(in crate::app) fn draft_target(doc: &Document) -> bool {
    !doc.is_pending()
        && doc.edits_text()
        && doc.preview_kind() == Some(PreviewKind::Markdown)
        && doc.readonly_reason().is_none()
}

fn literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii_punctuation()
            && !matches!(c, ',' | ':' | ';' | '?' | '\'' | '"' | '/' | '=' | '@')
        {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn quote_markdown(text: &str, url: &str) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    let quoted = text
        .trim_matches(['\r', '\n'])
        .lines()
        .map(|line| format!("> {}", literal(line.trim_end())))
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!("{quoted}\n\n{}: <{url}>\n", t!("reader-source")))
}

#[cfg(test)]
pub(in crate::app) mod tests;
