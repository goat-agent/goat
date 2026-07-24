use std::{future::Future, pin::Pin};

use goat_auth::CredentialStore;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum SearchAccountConfig {
    Duckduckgo {
        account: String,
    },
    Browser {
        account: String,
        #[serde(default = "default_browser_search_engine")]
        engine: String,
    },
    Searxng {
        account: String,
        endpoint: String,
    },
    Brave {
        account: String,
    },
    Tavily {
        account: String,
    },
}

fn default_browser_search_engine() -> String {
    "duckduckgo".to_owned()
}

impl SearchAccountConfig {
    pub fn target(&self) -> String {
        match self {
            Self::Duckduckgo { account } => format!("duckduckgo/{account}"),
            Self::Browser { account, .. } => format!("browser/{account}"),
            Self::Searxng { account, .. } => format!("searxng/{account}"),
            Self::Brave { account } => format!("brave/{account}"),
            Self::Tavily { account } => format!("tavily/{account}"),
        }
    }
}

pub type SearchFuture<'a> =
    Pin<Box<dyn Future<Output = Result<SearchResults, SearchError>> + Send + 'a>>;

#[derive(Debug, Clone)]
pub struct SearchRequest {
    pub query: String,
    pub max_results: usize,
    pub site: Option<String>,
    pub language: Option<String>,
    pub time_range: Option<String>,
    pub target: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SearchResults {
    pub query: String,
    pub provider: SearchTarget,
    pub language: Option<String>,
    pub time_range: Option<String>,
    pub limitations: Vec<&'static str>,
    pub results: Vec<SearchResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchTarget {
    pub provider: String,
    pub account: String,
}

impl SearchTarget {
    pub fn parse(s: &str) -> Option<Self> {
        let (provider, account) = s.split_once('/')?;
        if provider.is_empty() || account.is_empty() {
            return None;
        }
        Some(Self {
            provider: provider.to_owned(),
            account: account.to_owned(),
        })
    }

    pub fn as_str(&self) -> String {
        format!("{}/{}", self.provider, self.account)
    }
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub rank: usize,
    pub provider: String,
    pub published_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchCredentialMetadata {
    None,
    EnvApiKey { env_var: &'static str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchProviderKind {
    Browser { default_engine: &'static str },
    Duckduckgo,
    Searxng,
    Brave,
    Tavily,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchTargetMetadata<'a> {
    pub provider: &'a str,
    pub account: &'a str,
    pub target: &'a str,
    pub kind: &'static str,
    pub setup: &'static str,
    pub credential: SearchCredentialMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchBuiltinTarget {
    pub provider: &'static str,
    pub account: &'static str,
    pub target: &'static str,
    pub kind: &'static str,
    pub setup: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchProviderMetadata {
    pub id: &'static str,
    pub default_account: &'static str,
    pub kind: SearchProviderKind,
    pub credential: SearchCredentialMetadata,
    pub setup: &'static str,
    pub builtins: &'static [SearchBuiltinTarget],
}

pub trait SearchProvider: Send + Sync {
    fn metadata(&self) -> SearchProviderMetadata;
    fn target(&self) -> SearchTarget;
    fn search<'a>(
        &'a self,
        request: SearchRequest,
        credentials: Option<&'a CredentialStore>,
    ) -> SearchFuture<'a>;
}

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("query is empty")]
    EmptyQuery,
    #[error("invalid search target: {0}")]
    InvalidTarget(String),
    #[error("unknown search target: {0}")]
    UnknownTarget(String),
    #[error("invalid search provider config: {0}")]
    InvalidProvider(String),
    #[error("missing search credential for {0}/{1}")]
    MissingCredential(String, String),
    #[error("browser-backed search failed: {0}")]
    Browser(String),
    #[error("invalid search url: {0}")]
    Url(String),
    #[error("search request failed: {0}")]
    Request(reqwest::Error),
}

pub fn build_query(request: &SearchRequest) -> Result<String, SearchError> {
    let mut query = request.query.trim().to_owned();
    if query.is_empty() {
        return Err(SearchError::EmptyQuery);
    }
    if let Some(site) = &request.site
        && !site.trim().is_empty()
    {
        query = format!("site:{} {query}", site.trim());
    }
    Ok(query)
}

pub fn client(timeout: std::time::Duration) -> Result<reqwest::Client, SearchError> {
    reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::limited(4))
        .user_agent("goat-code WebSearch")
        .build()
        .map_err(SearchError::Request)
}

pub fn clean_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    decode_entities(&out)
}

pub fn decode_entities(text: &str) -> String {
    text.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

pub fn search_secret(
    credentials: Option<&CredentialStore>,
    provider: &str,
    account: &str,
    env_var: Option<&str>,
) -> Result<String, SearchError> {
    let key = goat_auth::CredentialKey::search(provider, account);
    credentials
        .and_then(|store| store.resolve(&key, env_var))
        .map(|credential| credential.bearer().to_owned())
        .ok_or_else(|| SearchError::MissingCredential(provider.to_owned(), account.to_owned()))
}

pub fn finish_results(
    target: SearchTarget,
    query: String,
    request: SearchRequest,
    mut limitations: Vec<&'static str>,
    mut results: Vec<SearchResult>,
) -> SearchResults {
    let target_label = target.as_str();
    for result in &mut results {
        result.provider.clone_from(&target_label);
    }
    limitations.push("results_are_untrusted_candidates");
    limitations.push("verify_with_webfetch");
    SearchResults {
        query,
        provider: target,
        language: request.language,
        time_range: request.time_range,
        limitations,
        results,
    }
}
