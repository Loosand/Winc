//! Disk format: one directory per normalized original URL, with independent position data.
//! A link-only record is committed before downloading so a failed import remains retryable.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;
use winc_config::files::write_atomic_bytes;

use crate::web::HttpFailure;

/// Top-level block index and the fractional offset within that block.
pub type Place = (usize, f32);

/// HTTP download callback used by the archive.
pub type Fetch = fn(&Url, u64, u64, Option<&str>) -> Result<(Url, Vec<u8>), HttpFailure>;
const PAGE_LIMIT: u64 = 4 * 1024 * 1024;
const IMAGE_LIMIT: u64 = 5 * 1024 * 1024;
const IMAGE_TOTAL: u64 = 32 * 1024 * 1024;
const IMAGE_COUNT: usize = 24;
const ONLINE_IMAGE_COUNT: usize = 64;
const IMAGE_TIME: Duration = Duration::from_secs(30);

/// Metadata stored for a saved article or a link-only record.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Article {
    /// Stable identifier derived from the original URL.
    pub key: String,
    /// Normalized original URL.
    pub url: String,
    /// Article title or the link used as a fallback.
    pub title: String,
    /// Whether the Markdown body has been archived.
    pub full: bool,
    /// Reason for a link-only record when the fetch or extraction failed.
    #[serde(default)]
    pub failure: Option<HttpFailure>,
    /// The beginning of the archived body.
    #[serde(default)]
    pub preview: String,
    added: u64,
}

pub(super) fn http_url(raw: &str) -> Option<Url> {
    let raw = raw.trim();
    if raw.len() > 4096 || raw.chars().any(char::is_control) {
        return None;
    }
    checked(Url::parse(raw).ok()?)
}

fn checked(url: Url) -> Option<Url> {
    (matches!(url.scheme(), "http" | "https")
        && url.has_host()
        && url.username().is_empty()
        && url.password().is_none())
    .then_some(url)
}

/// Parse an HTTP(S) article URL and remove its fragment for storage.
pub fn article_url(raw: &str) -> Option<Url> {
    let mut url = http_url(raw)?;
    url.set_fragment(None);
    Some(url)
}

pub(super) fn key(url: &Url) -> String {
    digest(url.as_str().as_bytes())
}

pub(super) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn valid_key(key: &str) -> bool {
    key.len() == 64
        && key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(super) fn check_key(id: &str) -> io::Result<()> {
    if valid_key(id) {
        Ok(())
    } else {
        Err(io::ErrorKind::InvalidInput.into())
    }
}

pub(super) fn resolve(base: &Url, raw: &str) -> Option<Url> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 4096 || raw.chars().any(char::is_control) {
        return None;
    }
    checked(base.join(raw).ok()?)
}

/// Keep requests bounded even when a publisher supplies a large batch of images.
pub(super) fn online_images(base: Option<&Url>) -> impl FnMut(&str) -> Option<String> {
    let mut seen = HashSet::new();
    move |src| {
        let mut url = resolve(base?, src)?;
        url.set_fragment(None);
        let url = url.to_string();
        if seen.len() == ONLINE_IMAGE_COUNT && !seen.contains(&url) {
            return None;
        }
        seen.insert(url.clone());
        Some(url)
    }
}

/// List saved articles, newest first.
///
/// # Errors
///
/// Returns an error if the archive directory cannot be enumerated.
pub fn list(root: &Path) -> io::Result<Vec<Article>> {
    let mut articles = Vec::new();
    let dirs = match fs::read_dir(root.join("articles")) {
        Ok(dirs) => dirs,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(articles),
        Err(e) => return Err(e),
    };
    for dir in dirs.flatten() {
        if let Some(id) = dir.file_name().to_str().filter(|s| valid_key(s))
            && let Some(mut article) = read_article(root, id)
        {
            if article.full && article.preview.is_empty() {
                // Older archives may omit the excerpt. Read only the start of the body.
                let mut bytes = Vec::new();
                if fs::File::open(dir.path().join("body.md"))
                    .and_then(|file| file.take(6400).read_to_end(&mut bytes))
                    .is_ok()
                {
                    article.preview = String::from_utf8_lossy(&bytes).chars().take(1600).collect();
                }
            }
            articles.push(article);
        }
    }
    sort(&mut articles);
    Ok(articles)
}

pub(super) fn sort(articles: &mut [Article]) {
    articles.sort_by(|a, b| b.added.cmp(&a.added).then_with(|| a.key.cmp(&b.key)));
}

pub(super) fn dir(root: &Path, id: &str) -> PathBuf {
    root.join("articles").join(id)
}

fn read_article(root: &Path, id: &str) -> Option<Article> {
    check_key(id).ok()?;
    let a: Article = read_json(&dir(root, id).join("article.json"))?;
    (a.key == id && article_url(&a.url).is_some_and(|u| key(&u) == id)).then_some(a)
}

pub(super) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

pub(super) fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    write_atomic_bytes(path, &serde_json::to_vec(value)?)
}

/// Read an archive record, its optional Markdown body, and its saved position.
pub fn open(root: &Path, id: &str) -> Option<(Article, Option<String>, Place)> {
    let article = read_article(root, id)?;
    let path = dir(root, id);
    let markdown = article
        .full
        .then(|| fs::read_to_string(path.join("body.md")).ok())
        .flatten();
    let place: Place = read_json(&path.join("position.json")).unwrap_or_default();
    Some((article, markdown, (place.0, place.1.clamp(0.0, 1.0))))
}

/// Persist the position for an existing article.
///
/// # Errors
///
/// Returns an error for an invalid key or if the position cannot be written.
pub fn set_place(root: &Path, id: &str, place: Place) -> io::Result<()> {
    check_key(id)?;
    let dir = dir(root, id);
    if !dir.join("article.json").is_file() {
        return Ok(());
    }
    write_json(&dir.join("position.json"), &place)
}

/// Remove an archived article and its assets.
///
/// # Errors
///
/// Returns an error for an invalid key or if the archive cannot be removed.
pub fn remove(root: &Path, id: &str) -> io::Result<()> {
    check_key(id)?;
    fs::remove_dir_all(dir(root, id))
}

/// Resolve a permitted local asset name from an article directory.
pub fn image_path(dir: &Path, name: &str) -> Option<PathBuf> {
    let digest = name.strip_prefix("image-")?.strip_suffix(".png")?;
    valid_key(digest).then(|| dir.join(name))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn create_link(root: &Path, url: &Url, title: &str) -> io::Result<(PathBuf, Article)> {
    let id = key(url);
    let local = dir(root, &id);
    let article = if let Some(mut article) = read_article(root, &id) {
        article.full &= local.join("body.md").is_file();
        article
    } else {
        let article = Article {
            key: id,
            url: url.to_string(),
            title: title.to_owned(),
            full: false,
            failure: None,
            preview: String::new(),
            added: now(),
        };
        write_json(&local.join("article.json"), &article)?;
        article
    };
    Ok((local, article))
}

/// Archive a public article and its supported images.
///
/// # Errors
///
/// Returns an error for an invalid URL or a storage failure. Fetch failures are recorded in the archive.
pub fn import(root: &Path, url: &Url, missing: &str, fetch: Fetch) -> io::Result<Article> {
    let url = article_url(url.as_str()).ok_or(io::ErrorKind::InvalidInput)?;
    grab_page(root, &url, url.as_str(), missing, fetch)
}

/// Archive a feed entry or retrieve its original article.
///
/// # Errors
///
/// Returns an error for an invalid URL or a storage failure.
pub fn save_entry(
    root: &Path,
    entry: &super::feed::Entry,
    missing: &str,
    fetch: Fetch,
) -> io::Result<Article> {
    let url = article_url(&entry.url).ok_or(io::ErrorKind::InvalidInput)?;
    if !entry.full {
        return grab_page(root, &url, &entry.title, missing, fetch);
    }
    let (local, mut article) = create_link(root, &url, &entry.title)?;
    if !article.full {
        let base = http_url(entry.base()).unwrap_or_else(|| url.clone());
        let mut images = Images::new(&local, &base, &url, fetch);
        let text = entry.markdown(|src| images.download(src), missing);
        if text.trim().is_empty() {
            return grab_page(root, &url, &entry.title, missing, fetch);
        }
        store_body(&local, &mut article, &entry.title, &text)?;
    }
    Ok(article)
}

fn grab_page(
    root: &Path,
    url: &Url,
    title: &str,
    missing: &str,
    fetch: Fetch,
) -> io::Result<Article> {
    let (local, mut article) = create_link(root, url, title)?;
    if article.full {
        return Ok(article);
    }
    let (base, html) = match fetch_html(url, fetch) {
        Ok(page) => page,
        Err(failure) => return store_failure(&local, article, failure),
    };
    let mut images = Images::new(&local, &base, &base, fetch);
    let Some(page) = winc_preview::markdown::from_html::extract_article(
        &html,
        |href| resolve(&base, href).map(|url| url.to_string()),
        |src| images.download(src),
        missing,
    ) else {
        return store_failure(&local, article, HttpFailure::NoArticle);
    };
    store_body(&local, &mut article, &page.title, &page.markdown)?;
    Ok(article)
}

fn fetch_html(url: &Url, fetch: Fetch) -> Result<(Url, String), HttpFailure> {
    let (base, bytes) = fetch(url, 20, PAGE_LIMIT, None)?;
    let base = checked(base).ok_or(HttpFailure::Invalid)?;
    let html = String::from_utf8(bytes).map_err(|_| HttpFailure::Invalid)?;
    Ok((base, html))
}

/// Article content fetched without writing to the archive.
pub struct WebText {
    /// URL after redirects.
    pub base: Url,
    /// Extracted body with absolute links and image URLs.
    pub markdown: String,
}

/// Fetch and extract an article without archiving it.
///
/// # Errors
///
/// Returns the transport or extraction failure.
pub fn read_full(url: &Url, missing: &str, fetch: Fetch) -> Result<WebText, HttpFailure> {
    let (base, html) = fetch_html(url, fetch)?;
    let page = winc_preview::markdown::from_html::extract_article(
        &html,
        |href| resolve(&base, href).map(|url| url.to_string()),
        online_images(Some(&base)),
        missing,
    )
    .ok_or(HttpFailure::NoArticle)?;
    Ok(WebText {
        base,
        markdown: page.markdown,
    })
}

fn store_failure(local: &Path, mut article: Article, failure: HttpFailure) -> io::Result<Article> {
    article.failure = Some(failure);
    write_json(&local.join("article.json"), &article)?;
    Ok(article)
}

fn store_body(local: &Path, article: &mut Article, title: &str, markdown: &str) -> io::Result<()> {
    write_atomic_bytes(&local.join("body.md"), markdown.as_bytes())?;
    if !title.trim().is_empty() {
        article.title = title.chars().take(240).collect();
    }
    article.full = true;
    article.failure = None;
    article.preview = markdown.chars().take(1600).collect();
    write_json(&local.join("article.json"), article)
}

struct Images<'a> {
    local: &'a Path,
    base: &'a Url,
    page: &'a Url,
    fetch: Fetch,
    deadline: Instant,
    remaining: u64,
    cached: HashMap<Url, Option<String>>,
}

impl<'a> Images<'a> {
    fn new(local: &'a Path, base: &'a Url, page: &'a Url, fetch: Fetch) -> Self {
        Self {
            local,
            base,
            page,
            fetch,
            deadline: Instant::now() + IMAGE_TIME,
            remaining: IMAGE_TOTAL,
            cached: HashMap::new(),
        }
    }

    fn download(&mut self, src: &str) -> Option<String> {
        let mut url = resolve(self.base, src)?;
        url.set_fragment(None);
        if let Some(path) = self.cached.get(&url) {
            return path.clone();
        }
        if self.cached.len() >= IMAGE_COUNT
            || Instant::now() >= self.deadline
            || self.remaining == 0
        {
            return None;
        }
        let seconds = (self
            .deadline
            .saturating_duration_since(Instant::now())
            .as_secs()
            + 1)
        .min(5);
        let referer = crate::web::image_referer(self.page, &url);
        let image = (self.fetch)(
            &url,
            seconds,
            IMAGE_LIMIT.min(self.remaining),
            referer.as_deref(),
        )
        .ok()
        .and_then(|(_, bytes)| {
            self.remaining = self.remaining.saturating_sub(bytes.len() as u64);
            // Validate a supported raster before linking it. SVG is not imported as an image.
            winc_preview::image::load_bytes(&bytes).ok()?;
            // The preview sniffs the actual format; a raster suffix selects that loader.
            let name = format!("image-{:x}.png", Sha256::digest(&bytes));
            write_atomic_bytes(&self.local.join(&name), &bytes).ok()?;
            Some(name)
        });
        self.cached.insert(url, image.clone());
        image
    }
}

#[cfg(test)]
mod tests;
