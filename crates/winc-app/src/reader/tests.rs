use super::*;

// A controlled HTTP fixture for the article and feed caches.
pub(super) mod site {
    use std::io::{BufRead, BufReader, Write};
    use std::net::{SocketAddr, TcpListener};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    pub(crate) const IMAGE: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\xff\xff\xff\x00\x00\x00!\xf9\x04\x01\x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;";
    pub(crate) const FEED: &str = r#"<rss version="2.0" xmlns:content="http://purl.org/rss/1.0/modules/content/"><channel>
      <title>Field Notes</title>
      <item><title>Copy with full text</title><link>/full</link><guid isPermaLink="false">full</guid>
      <description>A teaser</description><content:encoded><![CDATA[<p>Full feed body.</p><img src="/blog/cover.gif">]]></content:encoded></item>
      <item><title>Only a summary</title><guid isPermaLink="false">summary</guid><link>/unavailable</link>
      <description><![CDATA[<p>Just a summary.</p>]]></description></item>
      <item><title>Duplicate ID</title><guid>full</guid><link>/duplicate</link></item>
      <item><title>Duplicate URL</title><guid>different</guid><link>/full</link></item>
      </channel></rss>"#;

    pub(crate) struct Site {
        address: SocketAddr,
        stop: Arc<AtomicBool>,
        pub broken: Arc<AtomicBool>,
        worker: Option<JoinHandle<()>>,
    }

    impl Site {
        pub(crate) fn start() -> Option<Self> {
            if !crate::web::command("curl")
                .arg("--version")
                .output()
                .is_ok_and(|o| o.status.success())
            {
                return None;
            }
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            listener.set_nonblocking(true).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let broken = Arc::new(AtomicBool::new(false));
            let stopped = stop.clone();
            let failed = broken.clone();
            let worker = thread::spawn(move || {
                while !stopped.load(Ordering::Relaxed) {
                    let Ok((mut socket, _)) = listener.accept() else {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    };
                    socket
                        .set_read_timeout(Some(Duration::from_secs(10)))
                        .unwrap();
                    let mut reader = BufReader::new(&mut socket);
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() {
                        continue;
                    }
                    let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
                    let mut has_origin = false;
                    loop {
                        line.clear();
                        if !reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                            break;
                        }
                        if let Some((header, value)) = line.split_once(':')
                            && header.eq_ignore_ascii_case("referer")
                        {
                            has_origin = value.trim() == format!("http://{address}/");
                        }
                    }
                    let article = format!(r#"<head><title>Site title</title></head><nav>Not the article</nav>
                    <article><h1>Field Notes</h1><p>These are the words I want to quote.</p>
                    <img src="cover.gif" alt="cover"><img src="missing.png" alt="missing chart">
                    <pre><code class="language-rust">let x = 42;</code></pre>{}</article>"#,
                        (0..85).map(|n| format!("<p>Paragraph {n}: this fixture is long enough to resume at a specific position.</p>")).collect::<String>());
                    let (status, extra, body): (_, _, &[u8]) = match path.as_str() {
                        "/article" => ("302 Found", "Location: /blog/story\r\n", b""),
                        "/blog/story" => {
                            ("200 OK", "Content-Type: text/html\r\n", article.as_bytes())
                        }
                        "/blog/cover.gif" if has_origin => {
                            ("200 OK", "Content-Type: image/gif\r\n", IMAGE)
                        }
                        "/blog/cover.gif" => ("403 Forbidden", "", b"This CDN requires a referrer"),
                        "/feed" if !failed.load(Ordering::Relaxed) => (
                            "200 OK",
                            "Content-Type: application/rss+xml\r\n",
                            FEED.as_bytes(),
                        ),
                        "/denied" => ("403 Forbidden", "", b"Not the article"),
                        _ => ("404 Not Found", "", b""),
                    };
                    let _ = write!(
                        socket,
                        "HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(body);
                }
            });
            Some(Self {
                address,
                stop,
                broken,
                worker: Some(worker),
            })
        }

        pub(crate) fn url(&self, path: &str) -> String {
            format!("http://{}{path}", self.address)
        }
    }

    impl Drop for Site {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            self.worker.take().unwrap().join().unwrap();
        }
    }
}

#[test]
fn reader_fetch_reports_a_real_http_denial_as_a_site_response() {
    let Some(site) = site::Site::start() else {
        return;
    };
    let url = url::Url::parse(&site.url("/denied")).unwrap();
    assert!(matches!(
        crate::web::fetch_http(&url, 5, 8192),
        Err(crate::web::HttpFailure::Status(403))
    ));
}

#[test]
fn reader_image_referrer_strips_the_path_and_blocks_downgrades() {
    use crate::web::{image_redirect_protocols, image_referer};
    let page = url::Url::parse("https://site.test/story?tracking=42#comments").unwrap();
    let cdn = url::Url::parse("https://cdn.test/cover.png").unwrap();
    let referer = image_referer(&page, &cdn);
    assert_eq!(referer.as_deref(), Some("https://site.test/"));
    assert_eq!(image_redirect_protocols(referer.as_deref()), "=https");
    let insecure = url::Url::parse("http://cdn.test/cover.png").unwrap();
    assert!(image_referer(&page, &insecure).is_none());
}

#[test]
fn quote_is_standard_markdown_and_external_source() {
    let text = quote_markdown(
        "Hello *literal*\n\n> Next",
        "https://example.org/blog?a=1&b=2",
        "Source",
    )
    .unwrap();
    assert!(text.starts_with("> Hello \\*literal\\*\n> \n> \\> Next\n\n"));
    assert!(text.ends_with("<https://example.org/blog?a=1&b=2>\n"));
    assert!(quote_markdown("\n  ", "https://example.org", "Source").is_none());
}

#[test]
fn article_assets_and_feed_read_state_survive_reopening() {
    use std::sync::atomic::Ordering;

    let Some(site) = site::Site::start() else {
        return;
    };
    let root = std::env::temp_dir().join(format!("minc-reader-backend-{}", std::process::id()));
    let article = store::article_url(&site.url("/article")).unwrap();
    let saved = store::import(&root, &article, "Missing image", fetch_http_with_referer).unwrap();
    assert!(saved.full);
    store::set_place(&root, &saved.key, (1, 0.5)).unwrap();
    let (_, body, place) = store::open(&root, &saved.key).unwrap();
    assert_eq!(place, (1, 0.5));
    let body = body.unwrap();
    assert!(body.contains("These are the words I want to quote."));
    let image = format!("image-{}.png", store::digest(site::IMAGE));
    assert!(body.contains(&image));
    let path = store::image_path(&store::dir(&root, &saved.key), &image).unwrap();
    assert_eq!(std::fs::read(path).unwrap(), site::IMAGE);

    let url = store::article_url(&site.url("/feed")).unwrap();
    assert!(feed::subscribe(&root, &url).unwrap());
    let mut feeds = feed::list(&root).unwrap();
    let subscription = feeds.pop().unwrap();
    feed::mark_read(&root, &subscription.key, &subscription.entries[0]).unwrap();
    site.broken.store(true, Ordering::Relaxed);
    feed::refresh(&root, &[(subscription.key.clone(), subscription.url)]).unwrap();
    let mut feeds = feed::list(&root).unwrap();
    let cached = feeds.pop().unwrap();
    assert!(cached.failed);
    assert!(cached.entries[0].read);
    assert_eq!(cached.unread, [1]);
    feed::unsubscribe(&root, &cached.key).unwrap();
    assert!(feed::list(&root).unwrap().is_empty());
    assert!(store::open(&root, &saved.key).is_some());
    std::fs::remove_dir_all(root).unwrap();
}
