use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::page::StopLoadingParams;
use chromiumoxide::handler::viewport::Viewport;
use chromiumoxide::{Browser, BrowserConfig};
use futures::StreamExt as _;
use goat_auth::CredentialStore;
use goat_search_provider::{
    SearchBuiltinTarget, SearchCredentialMetadata, SearchFuture, SearchProvider,
    SearchProviderKind, SearchProviderMetadata, SearchRequest, SearchResult, SearchTarget,
    build_query, clean_text, client, decode_entities, finish_results, strip_tags,
};
use reqwest::Url;

const SEARCH_TIMEOUT: Duration = Duration::from_secs(15);
const BROWSER_SEARCH_TIMEOUT: Duration = Duration::from_secs(25);

const BROWSER_BUILTINS: &[SearchBuiltinTarget] = &[SearchBuiltinTarget {
    provider: "browser",
    account: "duckduckgo",
    target: "browser/duckduckgo",
    kind: "built-in",
    setup: "no setup required",
}];

const DUCKDUCKGO_BUILTINS: &[SearchBuiltinTarget] = &[SearchBuiltinTarget {
    provider: "duckduckgo",
    account: "html",
    target: "duckduckgo/html",
    kind: "built-in",
    setup: "no setup required",
}];

pub fn browser_metadata() -> SearchProviderMetadata {
    SearchProviderMetadata {
        id: "browser",
        default_account: "duckduckgo",
        kind: SearchProviderKind::Browser {
            default_engine: "duckduckgo",
        },
        credential: SearchCredentialMetadata::None,
        setup: "no setup required",
        builtins: BROWSER_BUILTINS,
    }
}

pub fn duckduckgo_metadata() -> SearchProviderMetadata {
    SearchProviderMetadata {
        id: "duckduckgo",
        default_account: "html",
        kind: SearchProviderKind::Duckduckgo,
        credential: SearchCredentialMetadata::None,
        setup: "no setup required",
        builtins: DUCKDUCKGO_BUILTINS,
    }
}

pub struct BrowserDuckDuckGoProvider {
    account: String,
}

impl BrowserDuckDuckGoProvider {
    pub fn new(account: impl Into<String>) -> Self {
        Self {
            account: account.into(),
        }
    }
}

impl SearchProvider for BrowserDuckDuckGoProvider {
    fn metadata(&self) -> SearchProviderMetadata {
        browser_metadata()
    }

    fn target(&self) -> SearchTarget {
        SearchTarget {
            provider: "browser".to_owned(),
            account: self.account.clone(),
        }
    }

    fn search<'a>(
        &'a self,
        request: SearchRequest,
        _credentials: Option<&'a CredentialStore>,
    ) -> SearchFuture<'a> {
        Box::pin(async move {
            let query = build_query(&request)?;
            let body = fetch_duckduckgo_with_ephemeral_browser(&query).await?;
            let results = parse_duckduckgo_html(&body, request.max_results);
            Ok(finish_results(
                self.target(),
                query,
                request,
                vec!["ephemeral_no_cookie_browser", "duckduckgo_html_extraction"],
                results,
            ))
        })
    }
}

pub struct DuckDuckGoHtmlProvider {
    account: String,
}

impl DuckDuckGoHtmlProvider {
    pub fn new(account: impl Into<String>) -> Self {
        Self {
            account: account.into(),
        }
    }
}

impl SearchProvider for DuckDuckGoHtmlProvider {
    fn metadata(&self) -> SearchProviderMetadata {
        duckduckgo_metadata()
    }

    fn target(&self) -> SearchTarget {
        SearchTarget {
            provider: "duckduckgo".to_owned(),
            account: self.account.clone(),
        }
    }

    fn search<'a>(
        &'a self,
        request: SearchRequest,
        _credentials: Option<&'a CredentialStore>,
    ) -> SearchFuture<'a> {
        Box::pin(async move {
            let query = build_query(&request)?;
            let mut url = Url::parse("https://html.duckduckgo.com/html/")
                .map_err(|err| goat_search_provider::SearchError::Url(err.to_string()))?;
            url.query_pairs_mut().append_pair("q", &query);
            let body = client(SEARCH_TIMEOUT)?
                .get(url)
                .send()
                .await
                .map_err(goat_search_provider::SearchError::Request)?
                .text()
                .await
                .map_err(goat_search_provider::SearchError::Request)?;
            let results = parse_duckduckgo_html(&body, request.max_results);
            Ok(finish_results(
                self.target(),
                query,
                request,
                vec!["html_result_extraction"],
                results,
            ))
        })
    }
}

fn browser_launch_args() -> [&'static str; 22] {
    [
        "disable-background-networking",
        "enable-features=NetworkService,NetworkServiceInProcess",
        "disable-background-timer-throttling",
        "disable-backgrounding-occluded-windows",
        "disable-breakpad",
        "disable-client-side-phishing-detection",
        "disable-component-extensions-with-background-pages",
        "disable-default-apps",
        "disable-dev-shm-usage",
        "disable-features=TranslateUI",
        "disable-hang-monitor",
        "disable-ipc-flooding-protection",
        "disable-popup-blocking",
        "disable-prompt-on-repost",
        "disable-renderer-backgrounding",
        "disable-sync",
        "force-color-profile=srgb",
        "metrics-recording-only",
        "no-first-run",
        "no-default-browser-check",
        "disable-blink-features=AutomationControlled",
        "incognito",
    ]
}

async fn fetch_duckduckgo_with_ephemeral_browser(
    query: &str,
) -> Result<String, goat_search_provider::SearchError> {
    let profile = tempfile::Builder::new()
        .prefix("goat-websearch-")
        .tempdir()
        .map_err(|err| goat_search_provider::SearchError::Browser(err.to_string()))?;
    let config = BrowserConfig::builder()
        .new_headless_mode()
        .user_data_dir(profile.path())
        .viewport(None::<Viewport>)
        .launch_timeout(BROWSER_SEARCH_TIMEOUT)
        .request_timeout(BROWSER_SEARCH_TIMEOUT)
        .disable_default_args()
        .args(browser_launch_args())
        .build()
        .map_err(goat_search_provider::SearchError::Browser)?;
    let (mut browser, mut handler) = Browser::launch(config)
        .await
        .map_err(|err| goat_search_provider::SearchError::Browser(err.to_string()))?;
    let handler_task = tokio::spawn(async move { while handler.next().await.is_some() {} });
    let result = async {
        let mut url = Url::parse("https://html.duckduckgo.com/html/")
            .map_err(|err| goat_search_provider::SearchError::Url(err.to_string()))?;
        url.query_pairs_mut().append_pair("q", query);
        let page = browser
            .new_page("about:blank")
            .await
            .map_err(|err| goat_search_provider::SearchError::Browser(err.to_string()))?;
        match tokio::time::timeout(BROWSER_SEARCH_TIMEOUT, page.goto(url.to_string())).await {
            Ok(Ok(_)) => {}
            Ok(Err(err)) => {
                return Err(goat_search_provider::SearchError::Browser(err.to_string()));
            }
            Err(_) => {
                let _ = page.execute(StopLoadingParams::default()).await;
            }
        }
        page.content()
            .await
            .map_err(|err| goat_search_provider::SearchError::Browser(err.to_string()))
    }
    .await;
    let _ = browser.close().await;
    handler_task.abort();
    result
}

pub fn parse_duckduckgo_html(html: &str, max_results: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();
    let mut index = 0usize;
    while results.len() < max_results {
        let Some(link_rel) = html[index..].find("result__a") else {
            break;
        };
        let link_pos = index + link_rel;
        let Some(tag_start_rel) = html[..link_pos].rfind("<a") else {
            index = link_pos + "result__a".len();
            continue;
        };
        let Some(tag_end_rel) = html[tag_start_rel..].find('>') else {
            break;
        };
        let tag_end = tag_start_rel + tag_end_rel + 1;
        let tag = &html[tag_start_rel..tag_end];
        let Some(raw_href) = attr_value(tag, "href") else {
            index = tag_end;
            continue;
        };
        let Some(end_rel) = html[tag_end..].find("</a>") else {
            break;
        };
        let end = tag_end + end_rel;
        let title = clean_text(&strip_tags(&html[tag_end..end]));
        let url = normalize_duckduckgo_url(&raw_href);
        if title.is_empty() || url.is_empty() {
            index = end + 4;
            continue;
        }
        let snippet = find_snippet(&html[end..]).unwrap_or_default();
        let rank = results.len() + 1;
        results.push(SearchResult {
            title,
            url,
            snippet,
            rank,
            provider: String::new(),
            published_at: None,
        });
        index = end + 4;
    }
    results
}

fn find_snippet(after: &str) -> Option<String> {
    let pos = after.find("result__snippet")?;
    let open = after[pos..].find('>')? + pos + 1;
    let close = after[open..]
        .find("</a>")
        .or_else(|| after[open..].find("</div>"))?
        + open;
    let snippet = clean_text(&strip_tags(&after[open..close]));
    (!snippet.is_empty()).then_some(snippet)
}

fn normalize_duckduckgo_url(raw: &str) -> String {
    let decoded = decode_entities(raw);
    let normalized = if decoded.starts_with("//") {
        format!("https:{decoded}")
    } else {
        decoded
    };
    if let Ok(url) = Url::parse(&normalized) {
        if url.domain() == Some("duckduckgo.com")
            && url.path() == "/l/"
            && let Some(target) = url
                .query_pairs()
                .find_map(|(key, value)| (key == "uddg").then_some(value))
        {
            return target.into_owned();
        }
        return url.to_string();
    }
    normalized
}

fn attr_value(attrs: &str, name: &str) -> Option<String> {
    let lower = attrs.to_ascii_lowercase();
    let needle = format!("{name}=");
    let start = lower.find(&needle)? + needle.len();
    let quote = attrs[start..].chars().next()?;
    if quote == '"' || quote == '\'' {
        let rest = &attrs[start + quote.len_utf8()..];
        let end = rest.find(quote)?;
        Some(rest[..end].to_owned())
    } else {
        let rest = &attrs[start..];
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        Some(rest[..end].trim_end_matches('>').to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::parse_duckduckgo_html;

    #[test]
    fn parses_duckduckgo_results() {
        let html = r#"
            <div class="result">
              <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa&amp;rut=x">Example A</a>
              <a class="result__snippet">Snippet <b>A</b></a>
            </div>
            <div class="result">
              <a class='result__a' href="https://example.org/b">Example B</a>
              <div class="result__snippet">Snippet B</div>
            </div>
        "#;
        let results = parse_duckduckgo_html(html, 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Example A");
        assert_eq!(results[0].url, "https://example.com/a");
        assert_eq!(results[0].snippet, "Snippet A");
        assert_eq!(results[1].url, "https://example.org/b");
    }
}
