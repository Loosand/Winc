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

pub(super) type Place = (usize, f32);
use crate::app::workspace::library::{PreviewLine, preview_lines};
use crate::web::HttpFailure;

type Fetch = fn(&Url, u64, u64, Option<&str>) -> Result<(Url, Vec<u8>), HttpFailure>;
const PAGE_LIMIT: u64 = 4 * 1024 * 1024;
const IMAGE_LIMIT: u64 = 5 * 1024 * 1024;
const IMAGE_TOTAL: u64 = 32 * 1024 * 1024;
const IMAGE_COUNT: usize = 24;
const ONLINE_IMAGE_COUNT: usize = 64;
const IMAGE_TIME: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Article {
    pub key: String,
    pub url: String,
    pub title: String,
    pub full: bool,
    #[serde(default)]
    pub failure: Option<HttpFailure>,
    #[serde(default)]
    pub preview: String,
    #[serde(skip)]
    pub card_lines: Vec<PreviewLine>,
    added: u64,
}

impl Article {
    #[cfg(test)]
    pub(in crate::app) fn example_card(index: usize) -> Self {
        let title = format!("Article {index:04}: Reading and writing");
        let url = Url::parse(&format!("https://example.test/{index}")).unwrap();
        let preview = format!(
            "# {title}\n\nNotes about offline reading.\n\n## Section\n\n> Keep the quote.\n\n```rust\nlet useful = true;\n```"
        );
        let mut article = Self {
            key: key(&url),
            url: url.to_string(),
            title,
            full: true,
            failure: None,
            preview,
            card_lines: Vec::new(),
            added: index as u64,
        };
        article.prepare_card();
        article
    }

    /// Layout-independent excerpt; prepare it on the storage worker, never during paint.
    pub(super) fn prepare_card(&mut self) {
        if self.full && !self.preview.is_empty() {
            self.card_lines = preview_lines(&self.preview);
        }
    }
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

pub(super) fn article_url(raw: &str) -> Option<Url> {
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

pub(super) fn list(root: &Path) -> io::Result<Vec<Article>> {
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
                // Existing archives predate card previews. Only read the first few KB.
                let mut bytes = Vec::new();
                if fs::File::open(dir.path().join("body.md"))
                    .and_then(|file| file.take(6400).read_to_end(&mut bytes))
                    .is_ok()
                {
                    article.preview = String::from_utf8_lossy(&bytes).chars().take(1600).collect();
                }
            }
            article.prepare_card();
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
    let a: Article = read_json(&dir(root, id).join("article.json"))?;
    (a.key == id && article_url(&a.url).is_some_and(|u| key(&u) == id)).then_some(a)
}

pub(super) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

pub(super) fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    write_atomic_bytes(path, &serde_json::to_vec(value)?)
}

pub(super) fn open(root: &Path, id: &str) -> Option<(Article, Option<String>, Place)> {
    let article = read_article(root, id)?;
    let path = dir(root, id);
    let markdown = article
        .full
        .then(|| fs::read_to_string(path.join("body.md")).ok())
        .flatten();
    let place: Place = read_json(&path.join("position.json")).unwrap_or_default();
    Some((article, markdown, (place.0, place.1.clamp(0.0, 1.0))))
}

pub(super) fn set_place(root: &Path, id: &str, place: Place) -> io::Result<()> {
    let dir = dir(root, id);
    if !dir.join("article.json").is_file() {
        return Ok(());
    }
    write_json(&dir.join("position.json"), &place)
}

pub(super) fn remove(root: &Path, id: &str) -> io::Result<()> {
    fs::remove_dir_all(dir(root, id))
}

pub(in crate::app) fn image_path(dir: &Path, name: &str) -> Option<PathBuf> {
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
            card_lines: Vec::new(),
            added: now(),
        };
        write_json(&local.join("article.json"), &article)?;
        article
    };
    Ok((local, article))
}

pub(super) fn import(root: &Path, url: &Url, missing: &str, fetch: Fetch) -> io::Result<Article> {
    grab_page(root, url, url.as_str(), missing, fetch)
}

pub(super) fn save_entry(
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

pub(super) struct WebText {
    pub base: Url,
    pub markdown: String,
    pub excerpt: Vec<PreviewLine>,
}

/// Read the public original in this tab only. Resolve image URLs for the preview;
/// only bookmarking archives the images with the article.
pub(super) fn read_full(url: &Url, missing: &str, fetch: Fetch) -> Result<WebText, HttpFailure> {
    let (base, html) = fetch_html(url, fetch)?;
    let page = winc_preview::markdown::from_html::extract_article(
        &html,
        |href| resolve(&base, href).map(|url| url.to_string()),
        online_images(Some(&base)),
        missing,
    )
    .ok_or(HttpFailure::NoArticle)?;
    let lead: String = page.markdown.chars().take(1600).collect();
    Ok(WebText {
        base,
        markdown: page.markdown,
        excerpt: preview_lines(&lead),
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
