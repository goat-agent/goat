use std::time::Duration;

use goat_auth::CredentialStore;
use goat_search_provider::{
    SearchCredentialMetadata, SearchFuture, SearchProvider, SearchProviderKind,
    SearchProviderMetadata, SearchRequest, SearchResult, SearchTarget, build_query, clean_text,
    client, finish_results, search_secret, strip_tags,
};
use reqwest::Url;

const SEARCH_TIMEOUT: Duration = Duration::from_secs(15);
const ENV_VAR: &str = "BRAVE_API_KEY";

pub fn metadata() -> SearchProviderMetadata {
    SearchProviderMetadata {
        id: "brave",
        default_account: "default",
        kind: SearchProviderKind::Brave,
        credential: SearchCredentialMetadata::EnvApiKey { env_var: ENV_VAR },
        setup: "set BRAVE_API_KEY or run `goat code search login brave --key <key>`",
        builtins: &[],
    }
}

pub struct BraveProvider {
    account: String,
}

impl BraveProvider {
    pub fn new(account: impl Into<String>) -> Self {
        Self {
            account: account.into(),
        }
    }
}

impl SearchProvider for BraveProvider {
    fn metadata(&self) -> SearchProviderMetadata {
        metadata()
    }

    fn target(&self) -> SearchTarget {
        SearchTarget {
            provider: "brave".to_owned(),
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
            let token = search_secret(credentials, "brave", &self.account, Some(ENV_VAR))?;
            let mut url = Url::parse("https://api.search.brave.com/res/v1/web/search")
                .map_err(|err| goat_search_provider::SearchError::Url(err.to_string()))?;
            url.query_pairs_mut()
                .append_pair("q", &query)
                .append_pair("count", &request.max_results.to_string());
            if let Some(language) = &request.language {
                url.query_pairs_mut().append_pair("search_lang", language);
            }
            let value = client(SEARCH_TIMEOUT)?
                .get(url)
                .header("X-Subscription-Token", token)
                .header("Accept", "application/json")
                .send()
                .await
                .map_err(goat_search_provider::SearchError::Request)?
                .json::<serde_json::Value>()
                .await
                .map_err(goat_search_provider::SearchError::Request)?;
            let results = parse_brave_json(&value, request.max_results);
            Ok(finish_results(
                self.target(),
                query,
                request,
                vec!["brave_api"],
                results,
            ))
        })
    }
}

pub fn parse_brave_json(value: &serde_json::Value, max_results: usize) -> Vec<SearchResult> {
    value
        .get("web")
        .and_then(|web| web.get("results"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .take(max_results)
        .enumerate()
        .filter_map(|(index, item)| {
            let title = item.get("title").and_then(serde_json::Value::as_str)?;
            let url = item.get("url").and_then(serde_json::Value::as_str)?;
            let snippet = item
                .get("description")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            Some(SearchResult {
                title: clean_text(&strip_tags(title)),
                url: url.to_owned(),
                snippet: clean_text(&strip_tags(snippet)),
                rank: index + 1,
                provider: String::new(),
                published_at: item
                    .get("age")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned),
            })
        })
        .collect()
}
