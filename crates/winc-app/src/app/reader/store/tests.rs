use super::*;

fn local(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("minc-reader-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    path
}

fn offline(_: &Url, _: u64, _: u64, _: Option<&str>) -> Result<(Url, Vec<u8>), HttpFailure> {
    Err(HttpFailure::Status(403))
}

fn html(url: &Url, _: u64, _: u64, _: Option<&str>) -> Result<(Url, Vec<u8>), HttpFailure> {
    Ok((
        url.clone(),
        b"<article><h1>Example</h1><p>This is the saved content.</p></article>".to_vec(),
    ))
}

#[test]
fn failed_import_stores_only_the_link_then_retry_reuses_the_same_record() {
    let root = local("retry");
    let url = article_url("https://example.com/story#intro").unwrap();
    let record = import(&root, &url, "missing", offline).unwrap();
    assert_eq!(record.url, "https://example.com/story");
    assert!(!record.full);
    assert!(matches!(
        open(&root, &record.key).unwrap().0.failure,
        Some(HttpFailure::Status(403))
    ));
    assert!(open(&root, &record.key).unwrap().1.is_none());
    let ready = import(&root, &url, "missing", html).unwrap();
    assert!(ready.full);
    assert_eq!(ready.key, record.key);
    let duplicate = import(&root, &url, "missing", offline).unwrap();
    assert!(duplicate.full);
    assert_eq!(list(&root).unwrap().len(), 1);
    assert!(
        open(&root, &record.key)
            .unwrap()
            .1
            .unwrap()
            .contains("saved content")
    );
    remove(&root, &record.key).unwrap();
    assert!(list(&root).unwrap().is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn reading_urls_are_http_only_and_never_contain_credentials() {
    for url in [
        "file:///notes.md",
        "javascript:alert(1)",
        "https://a:b@example.com",
        "https://example.com/\nabc",
    ] {
        assert!(http_url(url).is_none());
    }
    let base = http_url("https://example.com/a/b").unwrap();
    assert_eq!(
        resolve(&base, "../fig.png").unwrap().as_str(),
        "https://example.com/fig.png"
    );
    assert!(resolve(&base, "//a:b@example.com/image.png").is_none());
}
