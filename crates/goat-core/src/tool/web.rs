use std::path::Path;
use std::time::Duration;

use serde::Serialize;

use super::spill;
use crate::provider::{Hit, Provider};

const PAGE_BYTES: usize = 2 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);
const SKIPPED: [&str; 6] = ["script", "style", "noscript", "template", "svg", "iframe"];
const ACCEPT: &str =
    "text/markdown, text/html;q=0.9, text/plain;q=0.8, application/json;q=0.8, */*;q=0.1";

#[derive(Debug, Clone)]
pub(crate) struct Web {
    pub(crate) http: reqwest::Client,
    pub(crate) search: Option<Searcher>,
}

#[derive(Debug, Clone)]
pub(crate) struct Searcher {
    pub(crate) provider: Provider,
    pub(crate) model: Box<str>,
    pub(crate) bearer: Box<str>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Page {
    url: String,
    status: u16,
    text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Body {
    Html,
    Text,
}

pub(crate) async fn fetch(
    http: &reqwest::Client,
    raw: &str,
    spill_dir: &Path,
) -> Result<Page, String> {
    let url = reqwest::Url::parse(raw).map_err(|err| format!("{raw} is not a URL: {err}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!(
            "only http and https URLs can be fetched, not {}",
            url.scheme()
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URLs that carry credentials are refused".to_owned());
    }
    let mut response = http
        .get(url)
        .header(reqwest::header::ACCEPT, ACCEPT)
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(|err| format!("could not fetch {raw}: {err}"))?;
    let status = response.status().as_u16();
    let final_url = response.url().to_string();
    let kind = body_of(response.headers().get(reqwest::header::CONTENT_TYPE))
        .map_err(|kind| format!("{raw} is {kind}, which fetch cannot read"))?;
    let mut bytes = Vec::new();
    let mut is_cut = false;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|err| format!("could not read {raw}: {err}"))?
    {
        let room = PAGE_BYTES - bytes.len();
        if chunk.len() > room {
            bytes.extend_from_slice(&chunk[..room]);
            is_cut = true;
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    let raw_text = String::from_utf8_lossy(&bytes);
    let mut text = match kind {
        Body::Html => htmd::HtmlToMarkdown::builder()
            .skip_tags(SKIPPED.to_vec())
            .build()
            .convert(raw_text.as_ref())
            .unwrap_or_else(|_| raw_text.clone().into_owned()),
        Body::Text => raw_text.into_owned(),
    };
    if is_cut {
        text.push_str("\n[the page is over 2 MB; the rest was not read]");
    }
    Ok(Page {
        url: final_url,
        status,
        text: spill::fit(text, spill_dir),
    })
}

pub(crate) async fn search(searcher: &Searcher, query: &str) -> Result<Vec<Hit>, String> {
    searcher
        .provider
        .search(query, &searcher.model, &searcher.bearer)
        .await
        .map_err(|err| format!("web search failed: {err}"))
}

fn body_of(header: Option<&reqwest::header::HeaderValue>) -> Result<Body, String> {
    let Some(kind) = header.and_then(|value| value.to_str().ok()) else {
        return Ok(Body::Text);
    };
    let kind = kind.to_ascii_lowercase();
    if kind.contains("html") {
        return Ok(Body::Html);
    }
    let is_text = kind.starts_with("text/")
        || ["json", "xml", "javascript", "markdown", "yaml", "toml"]
            .iter()
            .any(|word| kind.contains(word));
    if is_text {
        return Ok(Body::Text);
    }
    Err(kind)
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    async fn serve(content_type: &'static str, body: String) -> String {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let url = format!("http://{}/page", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            assert!(stream.read(&mut request).await.unwrap() > 0);
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).await.unwrap();
            stream.write_all(body.as_bytes()).await.unwrap();
        });
        url
    }

    #[tokio::test]
    async fn html_comes_back_as_markdown() {
        let dir = tempfile::tempdir().unwrap();
        let url = serve(
            "text/html; charset=utf-8",
            "<html><body><h1>Tokio</h1><p>An <b>async</b> runtime.</p></body></html>".to_owned(),
        )
        .await;

        let page = fetch(&reqwest::Client::new(), &url, dir.path())
            .await
            .unwrap();

        assert_eq!(page.status, 200);
        assert_eq!(page.text, "# Tokio\n\nAn **async** runtime.");
    }

    #[tokio::test]
    async fn scripts_and_styles_are_left_out() {
        let dir = tempfile::tempdir().unwrap();
        let url = serve(
            "text/html",
            "<head><title>T</title><style>p{}</style><script>let a = 1;</script></head><body><noscript>x</noscript><p>Kept</p></body>".to_owned(),
        )
        .await;

        let page = fetch(&reqwest::Client::new(), &url, dir.path())
            .await
            .unwrap();

        assert_eq!(page.text, "T\n\nKept");
    }

    #[tokio::test]
    async fn plain_text_is_kept_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let url = serve("application/json", r#"{"a": 1}"#.to_owned()).await;

        let page = fetch(&reqwest::Client::new(), &url, dir.path())
            .await
            .unwrap();

        assert_eq!(page.text, r#"{"a": 1}"#);
    }

    #[tokio::test]
    async fn a_page_over_the_limit_is_cut() {
        let dir = tempfile::tempdir().unwrap();
        let url = serve("text/plain", "x".repeat(PAGE_BYTES + 10)).await;

        let page = fetch(&reqwest::Client::new(), &url, dir.path())
            .await
            .unwrap();

        assert!(
            page.text
                .ends_with("[the page is over 2 MB; the rest was not read]")
        );
        assert!(page.text.contains("bytes omitted"));
    }

    #[tokio::test]
    async fn binary_content_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let url = serve("application/zip", "PK".to_owned()).await;

        let problem = fetch(&reqwest::Client::new(), &url, dir.path())
            .await
            .unwrap_err();

        assert!(problem.ends_with("is application/zip, which fetch cannot read"));
    }

    #[tokio::test]
    async fn credentials_in_a_url_are_refused() {
        let dir = tempfile::tempdir().unwrap();

        let problem = fetch(
            &reqwest::Client::new(),
            "https://me:secret@example.com",
            dir.path(),
        )
        .await
        .unwrap_err();

        assert_eq!(problem, "URLs that carry credentials are refused");
    }

    #[tokio::test]
    async fn only_web_urls_are_fetched() {
        let dir = tempfile::tempdir().unwrap();

        let problem = fetch(&reqwest::Client::new(), "file:///etc/passwd", dir.path())
            .await
            .unwrap_err();

        assert_eq!(problem, "only http and https URLs can be fetched, not file");
    }
}
