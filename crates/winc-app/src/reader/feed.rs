//! RSS 2/RDF and Atom caches, independent from the saved-article store.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use roxmltree::Node;
use serde::{Deserialize, Serialize};
use url::Url;

use super::{literal, store};

const MAX_ENTRIES: usize = 200;
const MAX_TEXT: usize = 5 * 1024 * 1024;
const ATOM: &str = "http://www.w3.org/2005/Atom";
const RSS_CONTENT: &str = "http://purl.org/rss/1.0/modules/content/";
const XML: &str = "http://www.w3.org/XML/1998/namespace";

/// A cached RSS or Atom subscription.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feed {
    /// Stable identifier derived from the subscription URL.
    pub key: String,
    /// Original subscription URL.
    pub url: String,
    /// Publisher-provided title.
    pub title: String,
    /// Known entries, newest batch first.
    pub entries: Vec<Entry>,
    /// Whether the last refresh failed.
    pub failed: bool,
    /// Derived in entry order when the cache or read state changes.
    #[serde(skip)]
    pub unread: Vec<usize>,
}

/// A cached item published in a feed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Stable identifier derived from the publisher's item ID or URL.
    pub key: String,
    /// Original article URL.
    pub url: String,
    /// Publisher-provided title.
    pub title: String,
    /// Whether the publisher supplied the full body.
    pub full: bool,
    /// Whether the item has been marked read locally.
    #[serde(skip)]
    pub read: bool,
    base: String,
    plain: bool,
    body: Arc<str>,
}

impl Feed {
    pub(super) fn index_unread(&mut self) {
        self.unread.clear();
        self.unread.extend(
            self.entries
                .iter()
                .enumerate()
                .filter_map(|(i, e)| (!e.read).then_some(i)),
        );
    }
}

impl Entry {
    pub(super) fn markdown(
        &self,
        image: impl FnMut(&str) -> Option<String>,
        missing: &str,
    ) -> String {
        self.markdown_for(&self.body, image, missing)
    }

    /// Convert the cached body or summary to Markdown with absolute image URLs.
    pub fn online_markdown(&self, missing: &str) -> String {
        let base = store::http_url(self.base()).or_else(|| store::http_url(&self.url));
        self.markdown(store::online_images(base.as_ref()), missing)
    }

    fn markdown_for(
        &self,
        body: &str,
        image: impl FnMut(&str) -> Option<String>,
        missing: &str,
    ) -> String {
        if self.plain {
            return body.lines().map(literal).collect::<Vec<_>>().join("\n");
        }
        let base = store::http_url(&self.base).or_else(|| store::http_url(&self.url));
        winc_preview::markdown::from_html::fragment_to_markdown(
            body,
            |href| store::resolve(base.as_ref()?, href).map(|u| u.to_string()),
            image,
            missing,
        )
    }

    /// Archive this feed entry or retrieve its original article.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid URL or a storage failure.
    pub fn save(
        &self,
        root: &Path,
        missing: &str,
        fetch: store::Fetch,
    ) -> io::Result<store::Article> {
        let draft = store::EntryDraft {
            url: &self.url,
            title: &self.title,
            full: self.full,
            base: self.base(),
        };
        store::save_rendered_entry(
            root,
            &draft,
            |download| self.markdown(download, missing),
            missing,
            fetch,
        )
    }

    pub(super) fn base(&self) -> &str {
        &self.base
    }
}

fn child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children().find(|n| n.has_tag_name(name))
}

fn base_of(node: Node<'_, '_>, base: &Url) -> Url {
    let mut parts = node
        .ancestors()
        .filter_map(|n| n.attribute((XML, "base")))
        .collect::<Vec<_>>();
    let mut base = base.clone();
    while let Some(part) = parts.pop() {
        if let Some(url) = store::resolve(&base, part) {
            base = url;
        }
    }
    base
}

fn text(node: Node<'_, '_>) -> String {
    node.children()
        .filter(|n| n.is_text())
        .filter_map(|n| n.text())
        .collect::<String>()
}

fn title(node: Node<'_, '_>, fallback: &str) -> String {
    let value = child(node, "title").map_or_else(String::new, text);
    let line = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let line = if line.is_empty() { fallback } else { &line };
    line.chars().take(240).collect()
}

fn atom_url(node: Node<'_, '_>, base: &Url) -> Option<Url> {
    let links = || {
        node.children().filter(|n| {
            n.has_tag_name("link") && n.attribute("rel").is_none_or(|r| r == "alternate")
        })
    };
    let open = |n: Node<'_, '_>| {
        n.attribute("href")
            .and_then(|href| store::resolve(&base_of(n, base), href))
    };
    links()
        .filter(|n| {
            n.attribute("type")
                .is_none_or(|t| t == "text/html" || t == "application/xhtml+xml")
        })
        .find_map(open)
        .or_else(|| links().find_map(open))
        .or_else(|| store::http_url(&text(child(node, "id")?)))
}

fn rss_url(node: Node<'_, '_>, base: &Url) -> Option<Url> {
    child(node, "link")
        .and_then(|n| store::resolve(&base_of(n, base), &text(n)))
        .or_else(|| {
            child(node, "guid")
                .filter(|n| n.attribute("isPermaLink") != Some("false"))
                .and_then(|n| store::http_url(&text(n)))
        })
}

fn atom_body(node: Node<'_, '_>) -> Option<(String, bool)> {
    if node.attribute("src").is_some() {
        return None;
    }
    let (value, plain) = match node.attribute("type").unwrap_or("text") {
        "text" | "text/plain" => (text(node), true),
        "html" | "text/html" => (text(node), false),
        "xhtml" | "application/xhtml+xml" => {
            let input = node.document().input_text();
            (
                node.children()
                    .filter(|n| n.is_element())
                    .map(|n| &input[n.range()])
                    .collect(),
                false,
            )
        }
        _ => return None,
    };
    (!value.trim().is_empty()).then_some((value, plain))
}

fn rss_body(node: Node<'_, '_>) -> Option<(String, bool)> {
    let value = text(node);
    (!value.trim().is_empty()).then_some((value, false))
}

fn parse(xml: &str, source: &Url, effective: &Url) -> Option<Feed> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let root = doc.root_element();
    let atom = root.has_tag_name((ATOM, "feed"))
        || (root.has_tag_name("feed") && root.tag_name().namespace().is_none());
    let rdf = root.has_tag_name(("http://www.w3.org/1999/02/22-rdf-syntax-ns#", "RDF"));
    let channel = if atom {
        root
    } else if root.has_tag_name("rss") || rdf {
        child(root, "channel")?
    } else {
        return None;
    };
    let items = if rdf { root } else { channel };
    let mut feed = Feed {
        key: store::key(source),
        url: source.to_string(),
        title: title(channel, source.as_str()),
        entries: Vec::new(),
        failed: false,
        unread: Vec::new(),
    };
    let mut ids = HashSet::new();
    let mut urls = HashSet::new();
    for node in items
        .children()
        .filter(|n| n.has_tag_name(if atom { "entry" } else { "item" }))
        .take(MAX_ENTRIES * 2)
    {
        let Some(mut url) = (if atom {
            atom_url(node, effective)
        } else {
            rss_url(node, effective)
        }) else {
            continue;
        };
        url.set_fragment(None);
        let id = child(node, if atom { "id" } else { "guid" })
            .map(text)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| url.to_string());
        let key = store::digest(id.trim().as_bytes());
        if ids.contains(&key) || urls.contains(url.as_str()) {
            continue;
        }
        ids.insert(key.clone());
        urls.insert(url.to_string());
        let content = if atom {
            child(node, "content").and_then(|n| atom_body(n).map(|body| (body, n)))
        } else {
            node.children()
                .find(|n| n.has_tag_name((RSS_CONTENT, "encoded")))
                .and_then(|n| rss_body(n).map(|body| (body, n)))
        };
        let full = content.is_some();
        let summary = || {
            let n = child(node, if atom { "summary" } else { "description" })?;
            let body = if atom { atom_body(n)? } else { rss_body(n)? };
            Some((body, n))
        };
        let ((body, plain), context) = content
            .or_else(summary)
            .unwrap_or(((String::new(), true), node));
        feed.entries.push(Entry {
            key,
            url: url.to_string(),
            title: title(node, url.as_str()),
            full,
            read: false,
            base: base_of(context, effective).to_string(),
            plain,
            body: body.into(),
        });
        if feed.entries.len() == MAX_ENTRIES {
            break;
        }
    }
    Some(feed)
}

fn path(root: &Path, key: &str) -> PathBuf {
    root.join("feeds").join(format!("{key}.json"))
}

fn load(root: &Path, key: &str) -> Option<Feed> {
    store::check_key(key).ok()?;
    let feed: Feed = store::read_json(&path(root, key))?;
    (feed.key == key && store::article_url(&feed.url).is_some_and(|u| store::key(&u) == key))
        .then_some(feed)
}

fn read_dir(root: &Path, feed: &str) -> PathBuf {
    root.join("feeds").join(feed)
}

fn apply_read(root: &Path, feed: &mut Feed) {
    let dir = read_dir(root, &feed.key);
    if let Ok(mut files) = fs::read_dir(&dir) {
        let limit = feed.entries.len() * 4 + 32;
        let mut marked = HashSet::new();
        for entry in files.by_ref().take(limit).flatten() {
            if let Ok(name) = entry.file_name().into_string()
                && store::valid_key(&name)
                && entry.file_type().is_ok_and(|t| t.is_file())
            {
                marked.insert(name);
            }
        }
        // Read markers outlive the capped feed. Don't walk years of markers for 200 live entries.
        let large = files.next().is_some();
        if large || !marked.is_empty() {
            let marked = |key: &str| marked.contains(key) || (large && dir.join(key).is_file());
            for entry in &mut feed.entries {
                entry.read = marked(&entry.key) || marked(&store::digest(entry.url.as_bytes()));
            }
        }
    }
    feed.index_unread();
}

/// Load cached subscriptions with their read state.
///
/// # Errors
///
/// Returns an error if the feed directory cannot be enumerated.
pub fn list(root: &Path) -> io::Result<Vec<Feed>> {
    let files = match fs::read_dir(root.join("feeds")) {
        Ok(files) => files,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut feeds = Vec::new();
    for file in files.flatten() {
        if let Some(id) = file
            .file_name()
            .to_str()
            .and_then(|s| s.strip_suffix(".json"))
            .filter(|s| store::valid_key(s))
            && let Some(mut feed) = load(root, id)
        {
            apply_read(root, &mut feed);
            feeds.push(feed);
        }
    }
    feeds.sort_by(|a, b| a.title.cmp(&b.title).then_with(|| a.key.cmp(&b.key)));
    Ok(feeds)
}

/// Add a feed, returning whether the subscription exists.
///
/// # Errors
///
/// Returns an error for an invalid URL or if the cache cannot be written.
pub fn subscribe(root: &Path, url: &Url) -> io::Result<bool> {
    let url = store::article_url(url.as_str()).ok_or(io::ErrorKind::InvalidInput)?;
    let key = store::key(&url);
    if load(root, &key).is_some() {
        return Ok(true);
    }
    let Some(feed) = fetch(&url) else {
        return Ok(false);
    };
    store::write_json(&path(root, &key), &feed)?;
    Ok(true)
}

fn fetch(url: &Url) -> Option<Feed> {
    let (effective, bytes) = crate::web::fetch_http(url, 12, 4 * 1024 * 1024).ok()?;
    let effective = store::http_url(effective.as_str())?;
    parse(std::str::from_utf8(&bytes).ok()?, url, &effective)
}

fn merge(mut fresh: Feed, old: &Feed) -> Feed {
    let mut seen = HashSet::new();
    let mut urls = HashSet::new();
    for entry in &fresh.entries {
        seen.insert(entry.key.clone());
        urls.insert(entry.url.clone());
    }
    let mut size: usize = fresh.entries.iter().map(|e| e.body.len()).sum();
    for entry in &old.entries {
        if fresh.entries.len() >= MAX_ENTRIES || size.saturating_add(entry.body.len()) > MAX_TEXT {
            break;
        }
        if seen.insert(entry.key.clone()) && urls.insert(entry.url.clone()) {
            size += entry.body.len();
            fresh.entries.push(entry.clone());
        }
    }
    fresh
}

/// Refresh the selected cached feeds and retain previously seen entries.
///
/// # Errors
///
/// Returns an error if the refreshed cache cannot be written.
pub fn refresh(root: &Path, targets: &[(String, String)]) -> io::Result<()> {
    for (key, source) in targets {
        let Some(url) = store::article_url(source).filter(|url| store::key(url) == *key) else {
            continue;
        };
        let fresh = fetch(&url);
        // Keep cached content on failure and don't resurrect a removed subscription.
        let Some(mut old) = load(root, key) else {
            continue;
        };
        if let Some(fresh) = fresh {
            if fresh == old {
                continue;
            }
            let updated = merge(fresh, &old);
            if updated != old {
                store::write_json(&path(root, key), &updated)?;
            }
        } else if !old.failed {
            old.failed = true;
            store::write_json(&path(root, key), &old)?;
        }
    }
    Ok(())
}

/// Persist a read marker for a cached entry.
///
/// # Errors
///
/// Returns an error for invalid keys or if the marker cannot be written.
pub fn mark_read(root: &Path, feed: &str, entry: &Entry) -> io::Result<()> {
    store::check_key(feed)?;
    store::check_key(&entry.key)?;
    let dir = read_dir(root, feed);
    fs::create_dir_all(&dir)?;
    // Independent markers avoid replacing content during a background refresh.
    fs::File::create(dir.join(&entry.key))?;
    fs::File::create(dir.join(store::digest(entry.url.as_bytes())))?;
    Ok(())
}

/// Remove the feed cache and its read markers.
///
/// # Errors
///
/// Returns an error for an invalid key or a storage failure.
pub fn unsubscribe(root: &Path, key: &str) -> io::Result<()> {
    store::check_key(key)?;
    fs::remove_file(path(root, key))?;
    let _ = fs::remove_dir_all(read_dir(root, key));
    Ok(())
}

#[cfg(test)]
mod tests;
