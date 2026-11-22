//! Extract a reading copy from an HTML document without executing or loading the page.

use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{Handle, NodeData, RcDom};

use super::{Span, Writer};

/// The title and Markdown of the extracted main content.
pub struct ArticleHtml {
    /// The article heading, falling back to the page title.
    pub title: String,
    /// Main content as Markdown.
    pub markdown: String,
}

/// Extract a semantic article or a container with substantial prose, not the entire page.
/// The caller controls every link and image destination. Failed images are shown as text.
pub fn extract_article(
    html: &str,
    link: impl Fn(&str) -> Option<String>,
    image: impl FnMut(&str) -> Option<String>,
    missing: &str,
) -> Option<ArticleHtml> {
    let dom = html5ever::parse_document(RcDom::default(), Default::default()).one(html);
    let mut candidates = Candidates::default();
    score(&dom.document, 0, false, &mut candidates);
    let (_, node) = candidates
        .content
        .or(candidates.article)
        .or(candidates.main)
        .or(candidates.prose)?;
    let heading = first(&node, "h1", 0);
    let title = heading
        .as_ref()
        .map(text)
        .filter(|t| !t.is_empty())
        .or_else(|| {
            first(&dom.document, "title", 0)
                .map(|n| text(&n))
                .filter(|t| !t.is_empty())
        })
        .or_else(|| first(&dom.document, "h1", 0).map(|n| text(&n)))
        .unwrap_or_default();
    let markdown = convert(
        &node,
        link,
        image,
        missing,
        Some(Page {
            heading: heading.as_ref(),
        }),
    );
    (!markdown.trim().is_empty()).then_some(ArticleHtml { title, markdown })
}

/// Convert HTML already delimited by its publisher, such as a feed entry's content.
pub fn fragment_to_markdown(
    html: &str,
    link: impl Fn(&str) -> Option<String>,
    image: impl FnMut(&str) -> Option<String>,
    missing: &str,
) -> String {
    let dom = html5ever::parse_document(RcDom::default(), Default::default()).one(html);
    convert(&dom.document, link, image, missing, None)
}

const DEPTH: usize = 96;

fn name(node: &Handle) -> &str {
    match &node.data {
        NodeData::Element { name, .. } => &name.local,
        _ => "",
    }
}

fn attr(node: &Handle, attr: &str) -> Option<String> {
    let NodeData::Element { attrs, .. } = &node.data else {
        return None;
    };
    attrs
        .borrow()
        .iter()
        .find(|a| a.name.local.as_ref() == attr)
        .map(|a| a.value.to_string())
}

fn skip(node: &Handle) -> bool {
    let tag = name(node);
    super::is_skipped(tag)
        || matches!(
            tag,
            "nav" | "footer" | "aside" | "form" | "button" | "noscript" | "iframe" | "svg"
        )
        || attr(node, "hidden").is_some()
        || attr(node, "aria-hidden").as_deref() == Some("true")
        || attr(node, "role").is_some_and(|v| {
            matches!(
                v.as_str(),
                "navigation" | "complementary" | "contentinfo" | "banner"
            )
        })
        || [attr(node, "id"), attr(node, "class")]
            .into_iter()
            .flatten()
            .any(|v| {
                v.split_ascii_whitespace().any(|s| {
                    matches!(
                        s,
                        "comments"
                            | "related-posts"
                            | "share-buttons"
                            | "cookie-banner"
                            | "article-teaser"
                            | "post-excerpt"
                            | "article-excerpt"
                    )
                })
            })
}

// Extra cleanup only for full pages, not for publisher-delimited feed bodies.
fn skip_chrome(node: &Handle) -> bool {
    attr(node, "data-interactive").is_some()
        || (matches!(name(node), "div" | "section" | "ul" | "p")
            && [attr(node, "id"), attr(node, "class")]
                .into_iter()
                .flatten()
                .any(|v| {
                    v.split_ascii_whitespace()
                        .any(|s| matches!(s, "navigation" | "footer" | "tags" | "markdown-links"))
                }))
}

#[derive(Default)]
struct Candidates {
    content: Option<(usize, Handle)>,
    article: Option<(usize, Handle)>,
    main: Option<(usize, Handle)>,
    prose: Option<(usize, Handle)>,
}

// Ignore linked text and headings when looking for prose. Without markup identifying
// the article, require multiple prose blocks in the same container: merely wrapping
// a menu, index of teaser cards, or nested article does not qualify a parent.
fn score(node: &Handle, depth: usize, prose: bool, candidates: &mut Candidates) -> usize {
    if depth >= DEPTH || skip(node) || skip_chrome(node) || name(node) == "a" {
        return 0;
    }
    if let NodeData::Text { contents } = &node.data {
        return if prose {
            contents
                .borrow()
                .chars()
                .filter(|c| c.is_alphanumeric())
                .count()
        } else {
            0
        };
    }
    let tag = name(node);
    let prose = prose || matches!(tag, "p" | "pre" | "blockquote" | "li");
    let mut len = 0;
    let mut paragraphs = 0;
    for child in node.children.borrow().iter() {
        let size = score(child, depth + 1, prose, candidates);
        len += size;
        paragraphs += usize::from(name(child) == "p" && size >= 32);
    }
    let role = attr(node, "role");
    let explicit_body = attr(node, "itemprop").is_some_and(|v| v == "articleBody")
        || [attr(node, "class"), attr(node, "id")]
            .into_iter()
            .flatten()
            .any(|v| {
                v.split_ascii_whitespace().any(|s| {
                    matches!(
                        s,
                        "article-body"
                            | "article-content"
                            | "entry-content"
                            | "post-content"
                            | "post-body"
                            | "markdown-body"
                    )
                })
            });
    let candidate = if explicit_body {
        (len >= 32).then_some(&mut candidates.content)
    } else if tag == "article" || role.as_deref() == Some("article") {
        (len >= 8).then_some(&mut candidates.article)
    } else if tag == "main" || role.as_deref() == Some("main") {
        (len >= 48).then_some(&mut candidates.main)
    } else if matches!(tag, "div" | "section" | "body") {
        (len >= 100 && paragraphs >= 2).then_some(&mut candidates.prose)
    } else {
        None
    };
    if let Some(slot) = candidate
        && slot.as_ref().is_none_or(|(n, _)| len > *n)
    {
        *slot = Some((len, node.clone()));
    }
    len
}

fn first(node: &Handle, wanted: &str, depth: usize) -> Option<Handle> {
    if depth >= DEPTH || skip_chrome(node) {
        return None;
    }
    let tag = name(node);
    if tag == wanted && (tag == "title" || !skip(node)) {
        return Some(node.clone());
    }
    if skip(node) && !(wanted == "title" && tag == "head") {
        return None;
    }
    node.children
        .borrow()
        .iter()
        .find_map(|n| first(n, wanted, depth + 1))
}

fn text(node: &Handle) -> String {
    let mut text = String::new();
    collect_text(node, &mut text, 0);
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn collect_text(node: &Handle, text: &mut String, depth: usize) {
    if depth >= DEPTH {
        return;
    }
    if let NodeData::Text { contents } = &node.data {
        text.push_str(&contents.borrow());
    }
    for child in node.children.borrow().iter() {
        collect_text(child, text, depth + 1);
    }
}

#[derive(Clone, Copy)]
struct Page<'a> {
    heading: Option<&'a Handle>,
}

fn convert(
    node: &Handle,
    link: impl Fn(&str) -> Option<String>,
    mut image: impl FnMut(&str) -> Option<String>,
    missing: &str,
    page: Option<Page<'_>>,
) -> String {
    fn visit(
        node: &Handle,
        depth: usize,
        w: &mut Writer,
        link: &impl Fn(&str) -> Option<String>,
        image: &mut impl FnMut(&str) -> Option<String>,
        missing: &str,
        page: Option<Page<'_>>,
    ) {
        if depth >= DEPTH
            || skip(node)
            || page.is_some_and(|page| {
                skip_chrome(node) || page.heading.is_some_and(|h| std::rc::Rc::ptr_eq(node, h))
            })
        {
            return;
        }
        if let NodeData::Text { contents } = &node.data {
            w.text(&contents.borrow());
            return;
        }
        let tag = name(node);
        match tag {
            "img" if w.pre.is_none() => {
                let alt = attr(node, "alt").unwrap_or_default();
                let src = ["data-src", "src", "data-original"]
                    .into_iter()
                    .find_map(|key| {
                        attr(node, key).filter(|s| !s.trim().is_empty() && !s.starts_with("data:"))
                    })
                    .or_else(|| {
                        let set = attr(node, "srcset")?;
                        Some(set.split(',').next()?.split_whitespace().next()?.to_owned())
                    });
                if let Some(path) = src.as_deref().and_then(image) {
                    w.image(&alt, &path);
                } else {
                    let text = if alt.trim().is_empty() {
                        missing.to_owned()
                    } else {
                        format!("{missing} ({alt})")
                    };
                    w.text(&text);
                }
                return;
            }
            "a" if w.pre.is_none() => {
                let href = attr(node, "href")
                    .as_deref()
                    .and_then(link)
                    .unwrap_or_default();
                w.spans.push((Span::Link(href), w.inline.len()));
            }
            _ => {
                // Only pass the presentation attributes understood by the shared Markdown writer.
                let mut attrs = String::new();
                for key in ["class", "style", "start"] {
                    if let Some(mut value) = attr(node, key) {
                        use std::fmt::Write;
                        if key == "class" && matches!(tag, "pre" | "code") {
                            value.retain(|c| c != '`');
                        }
                        let value = value.replace('&', "&amp;").replace('"', "&quot;");
                        let _ = write!(attrs, " {key}=\"{value}\"");
                    }
                }
                w.open(tag, &attrs);
            }
        }
        for child in node.children.borrow().iter() {
            visit(child, depth + 1, w, link, image, missing, page);
        }
        w.close(tag);
    }
    let mut w = Writer::default();
    // Normalize the article heading before any byline or other lead-in. The reader
    // then does not supply a second heading and the body walk omits the original.
    if let Some(heading) = page.and_then(|page| page.heading) {
        visit(
            heading,
            0,
            &mut w,
            &link,
            &mut image,
            missing,
            Some(Page { heading: None }),
        );
    }
    visit(node, 0, &mut w, &link, &mut image, missing, page);
    w.finish().text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excludes_navigation_and_does_not_promote_an_explicit_teaser() {
        let html = r#"<title>Browse my blog</title><h1>Archive</h1>
            <article><div class="post-excerpt"><p>This teaser has enough words to look like a short article.</p></div></article>
            <nav><p><a href="/many">All of my posts and other navigation links</a></p></nav>"#;
        assert!(extract_article(html, |_| None, |_| None, "missing").is_none());
    }

    #[test]
    fn extracts_a_prose_container_without_semantic_article_tags() {
        let html = r#"<title>A blog post</title><div class="site-layout">
            <div class="navigation"><a href="/archive">Browse the archive</a></div>
            <div class="column"><div class="body">
                <p>Published on Thursday.</p><h1>A blog post</h1>
                <p>This is the first paragraph, with enough real prose to help identify the article itself.</p>
                <p>This is the second paragraph. Images and code also belong to the same reading copy.</p>
                <img src="/diagram.png" alt="diagram"><pre><code>example()</code></pre>
                <div data-interactive>Loading the voice samples…</div>
                <p class="tags">This entry was tagged <a href="/tag">Example</a>.</p>
                <p class="markdown-links"><a href="/post.md">Copy as Markdown</a></p>
            </div></div>
            <div class="credits"><p>Copyright and information about the website.</p></div>
        </div>"#;
        let article =
            extract_article(html, |s| Some(s.into()), |s| Some(s.into()), "missing").unwrap();
        assert_eq!(article.title, "A blog post");
        assert!(article.markdown.starts_with("# A blog post\n"));
        assert_eq!(article.markdown.matches("# A blog post").count(), 1);
        for text in [
            "This is the first paragraph",
            "![diagram](/diagram.png)",
            "example()",
        ] {
            assert!(article.markdown.contains(text), "{text}");
        }
        for text in [
            "Browse the archive",
            "Copyright",
            "Loading the voice",
            "was tagged",
            "Copy as",
        ] {
            assert!(!article.markdown.contains(text), "{text}");
        }
    }

    #[test]
    fn unstructured_index_is_not_returned_as_an_article() {
        let teaser = "A long excerpt summarizing the topic without giving away the entire article.";
        let html = format!(
            "<h1>Archive</h1>{}",
            (1..=5)
                .map(|n| format!(
                    r#"
            <div><h2><a href="/post/{n}">Story {n}</a></h2><p>{teaser}</p></div>"#
                ))
                .collect::<String>()
        );
        assert!(extract_article(&html, |_| None, |_| None, "missing").is_none());
    }

    #[test]
    fn article_body_preserves_headings_code_links_and_missing_assets() {
        let html = r#"<title>Local reading</title><div class="post-content">
            <h2>Example</h2><p>Words with sufficient prose for an explicit content container.</p>
            <a href="/continue">next</a><pre><code class="language-rust">let x = 1;</code></pre>
            <img src="/asset.jpg" alt="diagram"><script>do_not_keep_this()</script></div>"#;
        let article = extract_article(
            html,
            |href| Some(format!("https://example.org{href}")),
            |_| None,
            "missing offline",
        )
        .unwrap();
        assert_eq!(article.title, "Local reading");
        assert!(article.markdown.contains("## Example"));
        assert!(article.markdown.contains("```rust\nlet x = 1;\n```"));
        assert!(
            article
                .markdown
                .contains("[next](https://example.org/continue)")
        );
        assert!(article.markdown.contains("missing offline (diagram)"));
        assert!(!article.markdown.contains("do_not_keep_this"));
    }
}
