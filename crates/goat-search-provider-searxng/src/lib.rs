use std::time::Duration;

use goat_auth::CredentialStore;
use goat_search_provider::{
    SearchCredentialMetadata, SearchFuture, SearchProvider, SearchProviderKind,
    SearchProviderMetadata, SearchRequest, SearchResult, SearchTarget, build_query, clean_text,
    client, finish_results,
};
use reqwest::Url;

const SEARCH_TIMEOUT: Duration = Duration::from_secs(15);

pub fn metadata() -> SearchProviderMetadata {
    SearchProviderMetadata {
        id: "searxng",
        default_account: "default",
        kind: SearchProviderKind::Searxng,
        credential: SearchCredentialMetadata::None,
        setup: "run `goat code search login searxng --endpoint <url>`",
        builtins: &[],
    }
}

pub struct SearxngProvider {
    account: String,
    endpoint: String,
}

impl SearxngProvider {
    pub fn new(account: impl Into<String>, endpoint: impl Into<String>) -> Self {
        Self {
            account: account.into(),
            endpoint: endpoint.into(),
        }
    }
}

impl SearchProvider for SearxngProvider {
    fn metadata(&self) -> SearchProviderMetadata {
        metadata()
    }

    fn target(&self) -> SearchTarget {
        SearchTarget {
            provider: "searxng".to_owned(),
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
            let base = Url::parse(&self.endpoint)
                .map_err(|err| goat_search_provider::SearchError::Url(err.to_string()))?;
            if !matches!(base.scheme(), "http" | "https") {
                return Err(goat_search_provider::SearchError::Url(format!(
                    "unsupported searxng endpoint scheme: {}",
                    base.scheme()
                )));
            }
            let mut url = base
                .join("search")
                .map_err(|err| goat_search_provider::SearchError::Url(err.to_string()))?;
            url.query_pairs_mut()
                .append_pair("q", &query)
                .append_pair("format", "json");
            if let Some(language) = &request.language {
                url.query_pairs_mut().append_pair("language", language);
            }
            if let Some(time_range) = &request.time_range {
                url.query_pairs_mut().append_pair("time_range", time_range);
            }
            let value = client(SEARCH_TIMEOUT)?
                .get(url)
                .send()
                .await
                .map_err(goat_search_provider::SearchError::Request)?
                .json::<serde_json::Value>()
                .await
                .map_err(goat_search_provider::SearchError::Request)?;
            let results = parse_searxng_json(&value, request.max_results);
            Ok(finish_results(
                self.target(),
                query,
                request,
                vec!["searxng_json"],
                results,
            ))
        })
    }
}

pub fn parse_searxng_json(value: &serde_json::Value, max_results: usize) -> Vec<SearchResult> {
    value
        .get("results")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .take(max_results)
        .enumerate()
        .filter_map(|(index, item)| {
            let title = item.get("title").and_then(serde_json::Value::as_str)?;
            let url = item.get("url").and_then(serde_json::Value::as_str)?;
            let snippet = item
                .get("content")
                .or_else(|| item.get("snippet"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            Some(SearchResult {
                title: clean_text(title),
                url: url.to_owned(),
                snippet: clean_text(snippet),
                rank: index + 1,
                provider: String::new(),
                published_at: item
                    .get("publishedDate")
                    .or_else(|| item.get("published_at"))
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned),
            })
        })
        .collect()
}
