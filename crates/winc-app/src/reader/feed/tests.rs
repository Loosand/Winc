use super::*;

#[test]
fn atom_differentiates_summary_and_inline_content_and_uses_xml_base() {
    let rss = Url::parse("https://example.org/feeds/news.xml").unwrap();
    let xml = r#"<feed xmlns="http://www.w3.org/2005/Atom" xml:base="/blog/">
        <title>Example blog</title>
        <entry><id>urn:1</id><title>Content</title><link href="one"/>
          <content type="xhtml"><div xmlns="http://www.w3.org/1999/xhtml"><h2>Header</h2><p>A sentence.</p><a href="two">Next</a></div></content>
        </entry>
        <entry><id>urn:2</id><title>Summary</title><link rel="alternate" href="two"/>
          <content src="remote"/><summary type="html">&lt;p&gt;Summary only&lt;/p&gt;</summary>
        </entry>
        <entry><id>urn:2</id><title>Duplicate</title><link href="another"/></entry>
    </feed>"#;
    let feed = parse(xml, &rss, &rss).unwrap();
    assert_eq!(feed.entries.len(), 2);
    assert_eq!(feed.entries[0].url, "https://example.org/blog/one");
    assert!(feed.entries[0].full);
    let body = feed.entries[0].markdown(|_| None, "missing");
    assert!(body.contains("## Header"));
    assert!(body.contains("[Next](https://example.org/blog/two)"));
    assert!(!feed.entries[1].full);
    assert_eq!(
        feed.entries[1].markdown(|_| None, "missing").trim(),
        "Summary only"
    );
    assert!(parse("<html><title>not a feed</title></html>", &rss, &rss).is_none());
}

#[test]
fn merging_preserves_old_entries_without_duplicating_by_id_or_link() {
    let rss = Url::parse("https://example.org/feed").unwrap();
    let feed = parse(super::super::tests::site::FEED, &rss, &rss).unwrap();
    assert_eq!(feed.entries.len(), 2);
    assert!(feed.entries[0].full);
    assert!(!feed.entries[1].full);
    let mut fresh = feed.clone();
    let mut one = fresh.entries.remove(0);
    one.key = store::digest(b"changed id");
    fresh.entries = vec![one];
    let merged = merge(fresh, &feed);
    assert_eq!(merged.entries.len(), 2);
    assert_eq!(merged.entries[1].title, "Only a summary");
}
