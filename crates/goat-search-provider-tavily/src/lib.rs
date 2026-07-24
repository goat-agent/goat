use std::time::Duration;

use goat_auth::CredentialStore;
use goat_search_provider::{
    SearchCredentialMetadata, SearchFuture, SearchProvider, SearchProviderKind,
    SearchProviderMetadata, SearchRequest, SearchResult, SearchTarget, build_query, clean_text,
    client, finish_results, search_secret,
};

const SEARCH_TIMEOUT: Duration = Duration::from_secs(15);
const ENV_VAR: &str = "TAVILY_API_KEY";

pub fn metadata() -> SearchProviderMetadata {
    SearchProviderMetadata {
        id: "tavily",
        default_account: "default",
        kind: SearchProviderKind::Tavily,
        credential: SearchCredentialMetadata::EnvApiKey { env_var: ENV_VAR },
        setup: "set TAVILY_API_KEY or run `goat code search login tavily --key <key>`",
        builtins: &[],
    }
}

pub struct TavilyProvider {
    account: String,
}

impl TavilyProvider {
    pub fn new(account: impl Into<String>) -> Self {
        Self {
            account: account.into(),
        }
    }
}

impl SearchProvider for TavilyProvider {
    fn metadata(&self) -> SearchProviderMetadata {
        metadata()
    }

    fn target(&self) -> SearchTarget {
        SearchTarget {
            provider: "tavily".to_owned(),
            account: self.account.clone(),
        }
    }

    fn search<'a>(
        &'a self,
        request: SearchRequest,
        credentials: Option<&'a CredentialStore>,
    ) -> SearchFuture<'a> {
        Box::pin(async move {
            let query = build_query(&request)?;
            let token = search_secret(credentials, "tavily", &self.account, Some(ENV_VAR))?;
            let mut body = serde_json::json!({
                "api_key": token,
                "query": query,
                "max_results": request.max_results,
            });
            if let Some(site) = &request.site {
                body["include_domains"] = serde_json::json!([site]);
            }
            let value = client(SEARCH_TIMEOUT)?
                .post("https://api.tavily.com/search")
                .json(&body)
                .send()
                .await
                .map_err(goat_search_provider::SearchError::Request)?
                .json::<serde_json::Value>()
                .await
                .map_err(goat_search_provider::SearchError::Request)?;
            let results = parse_tavily_json(&value, request.max_results);
            Ok(finish_results(
                self.target(),
                query,
                request,
                vec!["tavily_api"],
                results,
            ))
        })
    }
}

pub fn parse_tavily_json(value: &serde_json::Value, max_results: usize) -> Vec<SearchResult> {
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
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            Some(SearchResult {
                title: clean_text(title),
                url: url.to_owned(),
                snippet: clean_text(snippet),
                rank: index + 1,
                provider: String::new(),
                published_at: item
                    .get("published_date")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned),
            })
        })
        .collect()
}
