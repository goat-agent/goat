use dom_smoothie::{Config, Readability};

const MIN_CONTENT_CHARS: usize = 200;

pub(crate) struct Extracted {
    pub content_html: String,
    pub title: Option<String>,
}

pub(crate) fn extract(html: &str, document_url: &str) -> Option<Extracted> {
    let mut readability =
        Readability::new(html, Some(document_url), Some(Config::default())).ok()?;
    let article = readability.parse().ok()?;
    let content: &str = article.content.as_ref();
    let content = content.trim();
    if content.is_empty() || article.length < MIN_CONTENT_CHARS {
        return None;
    }
    let title = normalize_title(&article.title);
    Some(Extracted {
        content_html: content.to_owned(),
        title,
    })
}

fn normalize_title(title: &str) -> Option<String> {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::extract;

    const ARTICLE: &str = r#"<!DOCTYPE html><html><head><title>Real Title</title></head>
      <body>
        <nav><a href="/">Home</a><a href="/about">About</a><a href="/login">Login</a></nav>
        <header>Site chrome that should be dropped</header>
        <article>
          <h1>The Heading</h1>
          <p>This is the first substantial paragraph of the article body. It carries the
             meaningful content a reader actually wants, with enough length to score well
             against the navigation and boilerplate around it.</p>
          <p>A second paragraph continues the discussion so the readability heuristics have
             a clear main-content block to lock onto and extract cleanly.</p>
        </article>
        <footer>Copyright and unrelated footer links go here for many words of clutter.</footer>
      </body></html>"#;

    #[test]
    fn extracts_body_and_drops_chrome() {
        let out = extract(ARTICLE, "https://example.com/post").expect("readable");
        assert!(out.content_html.contains("first substantial paragraph"));
        assert!(!out.content_html.contains("About"));
        assert!(!out.content_html.contains("Copyright and unrelated footer"));
    }

    #[test]
    fn tiny_page_is_not_extracted() {
        let out = extract(
            "<html><body><p>hi</p></body></html>",
            "https://example.com/",
        );
        assert!(out.is_none());
    }
}
