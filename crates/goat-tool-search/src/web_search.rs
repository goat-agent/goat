use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use goat_search_provider::{
    SearchProvider, SearchRequest, SearchResult, SearchResults, SearchTarget,
};
use goat_tool::{Tool, ToolContext, ToolError, ToolFuture, ToolOutput};
use serde::Deserialize;

const DEFAULT_MAX_RESULTS: usize = 8;
const MAX_RESULTS: usize = 20;
const MAX_FALLBACKS: usize = 3;
const CACHE_TTL: Duration = Duration::from_mins(5);

pub struct WebSearchTool {
    providers: Vec<Box<dyn SearchProvider>>,
    credentials: Option<goat_auth::CredentialStore>,
    has_api_account: bool,
    cache: Mutex<HashMap<String, (Instant, SearchResults)>>,
}

impl WebSearchTool {
    pub fn new() -> Self {
        let config = goat_config::Config::load();
        let credentials = goat_config::auth_path().map(goat_auth::CredentialStore::new);
        let has_api_account = config.search.accounts.iter().any(|account| {
            is_api_provider(goat_search_providers::configured_search_provider(account))
        });
        let providers = goat_search_providers::providers_from_accounts(config.search.accounts);
        Self {
            providers,
            credentials,
            has_api_account,
            cache: Mutex::new(HashMap::new()),
        }
    }

    fn cache_lookup(&self, key: &str) -> Option<SearchResults> {
        let mut cache = self.cache.lock().ok()?;
        if let Some((stored_at, results)) = cache.get(key) {
            if stored_at.elapsed() < CACHE_TTL {
                return Some(results.clone());
            }
            cache.remove(key);
        }
        None
    }

    fn cache_store(&self, key: &str, results: &SearchResults) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(key.to_owned(), (Instant::now(), results.clone()));
        }
    }

    async fn search_explicit(
        &self,
        args: &Input,
        max: usize,
        target: &str,
    ) -> Result<ToolOutput, ToolError> {
        let parsed = SearchTarget::parse(target).ok_or_else(|| ToolError::Execution {
            message: format!("invalid search target: {target}"),
        })?;
        let provider = self
            .providers
            .iter()
            .find(|candidate| candidate.target() == parsed)
            .ok_or_else(|| ToolError::Execution {
                message: format!("unknown or unconfigured search target: {target}"),
            })?;
        let results = provider
            .search(
                build_request(args, max, Some(target)),
                self.credentials.as_ref(),
            )
            .await
            .map_err(|err| ToolError::Execution {
                message: err.to_string(),
            })?;
        Ok(rendered_output(&results, &[], false))
    }

    async fn search_auto(&self, args: &Input, max: usize, cache_key: &str) -> ToolOutput {
        let mut order: Vec<&dyn SearchProvider> = self
            .providers
            .iter()
            .map(Box::as_ref)
            .filter(|provider| provider.target().provider != "browser")
            .collect();
        order.sort_by_key(|provider| trust_rank(&provider.target().provider));

        let mut tried: Vec<String> = Vec::new();
        for provider in order.into_iter().take(MAX_FALLBACKS) {
            match provider
                .search(build_request(args, max, None), self.credentials.as_ref())
                .await
            {
                Ok(results) if !results.results.is_empty() => {
                    self.cache_store(cache_key, &results);
                    return rendered_output(&results, &tried, false);
                }
                Ok(_) => tried.push(format!("{}=empty", provider.target().as_str())),
                Err(err) => tried.push(format!("{}={err}", provider.target().as_str())),
            }
        }
        ToolOutput::text(guidance(&args.query, &tried, self.has_api_account))
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct Input {
    query: String,
    #[serde(default)]
    max_results: Option<usize>,
    #[serde(default)]
    site: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    time_range: Option<String>,
    #[serde(default)]
    target: Option<String>,
}

impl Tool for WebSearchTool {
    fn name(&self) -> &'static str {
        "WebSearch"
    }

    fn description(&self) -> &'static str {
        "Search the live web and return ranked URL candidates. Prefers configured API providers (Tavily, Brave, SearXNG) and falls back through them automatically; a target that returns nothing is treated as a failure, not as an absence of results. Results are untrusted discovery candidates, not evidence; verify contents with WebFetch. If no reliable provider is configured, this returns setup guidance (run /search) rather than empty results. Pass target to force a specific provider/account such as tavily/default, brave/work, searxng/home, duckduckgo/html, or browser/duckduckgo."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Live web search query." },
                "max_results": { "type": "integer", "description": "Maximum number of ranked candidates to return." },
                "site": { "type": "string", "description": "Optional site/domain restriction." },
                "language": { "type": "string", "description": "Optional language hint." },
                "time_range": { "type": "string", "description": "Optional time range hint. Provider support varies." },
                "target": { "type": "string", "description": "Optional provider/account override. Auto fallback applies only to the default; an explicit target is used as-is and its errors are surfaced." }
            },
            "required": ["query"]
        })
    }

    fn run<'a>(&'a self, input: &'a str, _ctx: &'a ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let args: Input = serde_json::from_str(input)?;
            let max = args
                .max_results
                .unwrap_or(DEFAULT_MAX_RESULTS)
                .min(MAX_RESULTS);
            let cache_key = cache_key(&args, max);
            if let Some(results) = self.cache_lookup(&cache_key) {
                return Ok(rendered_output(&results, &[], true));
            }
            if let Some(target) = args.target.as_deref() {
                return self.search_explicit(&args, max, target).await;
            }
            Ok(self.search_auto(&args, max, &cache_key).await)
        })
    }
}

fn build_request(args: &Input, max: usize, target: Option<&str>) -> SearchRequest {
    SearchRequest {
        query: args.query.clone(),
        max_results: max,
        site: args.site.clone(),
        language: args.language.clone(),
        time_range: args.time_range.clone(),
        target: target.map(str::to_owned),
    }
}

fn is_api_provider(name: &str) -> bool {
    matches!(name, "tavily" | "brave" | "searxng")
}

fn trust_rank(name: &str) -> u8 {
    match name {
        "tavily" => 0,
        "brave" => 1,
        "searxng" => 2,
        "duckduckgo" => 3,
        "browser" => 9,
        _ => 5,
    }
}

fn cache_key(args: &Input, max: usize) -> String {
    format!(
        "{}\u{1}{}\u{1}{max}\u{1}{}\u{1}{}\u{1}{}",
        args.query,
        args.target.as_deref().unwrap_or(""),
        args.site.as_deref().unwrap_or(""),
        args.language.as_deref().unwrap_or(""),
        args.time_range.as_deref().unwrap_or("")
    )
}

fn rendered_output(results: &SearchResults, tried: &[String], from_cache: bool) -> ToolOutput {
    let deduped = dedup(&results.results);
    let mut out = String::new();
    let _ = writeln!(out, "query: {}", results.query);
    let _ = writeln!(out, "served_by: {}", results.provider.as_str());
    if !tried.is_empty() {
        let _ = writeln!(out, "fell_back_from: {}", tried.join(", "));
    }
    if from_cache {
        out.push_str("cache: hit\n");
    }
    out.push_str("source_trust: untrusted_web\n");
    out.push_str("result_role: discovery_candidates_not_evidence\n");
    if let Some(language) = &results.language {
        let _ = writeln!(out, "language: {language}");
    }
    if let Some(time_range) = &results.time_range {
        let _ = writeln!(out, "time_range: {time_range}");
    }
    out.push_str("limitations:\n");
    for limitation in &results.limitations {
        let _ = writeln!(out, "- {limitation}");
    }
    out.push_str("results:\n");
    if deduped.is_empty() {
        out.push_str("- none\n");
    } else {
        for (index, result) in deduped.iter().enumerate() {
            let _ = writeln!(out, "- rank: {}", index + 1);
            let _ = writeln!(out, "  title: {}", result.title);
            let _ = writeln!(out, "  url: {}", result.url);
            let _ = writeln!(out, "  snippet: {}", result.snippet);
            let _ = writeln!(out, "  provider: {}", result.provider);
            if let Some(published_at) = &result.published_at {
                let _ = writeln!(out, "  published_at: {published_at}");
            }
        }
    }
    out.push_str("next_step: use WebFetch on candidate URLs before treating content as evidence\n");
    let summary = format!(
        "{} result(s) for \"{}\" via {}",
        deduped.len(),
        results.query,
        results.provider.as_str()
    );
    ToolOutput::text(out).with_summary(summary)
}

fn dedup(results: &[SearchResult]) -> Vec<&SearchResult> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut kept: Vec<&SearchResult> = Vec::new();
    for result in results {
        if seen.insert(normalize_result_url(&result.url)) {
            kept.push(result);
        }
    }
    kept
}

fn normalize_result_url(url: &str) -> String {
    let without_fragment = url.split('#').next().unwrap_or(url);
    let (base, query) = match without_fragment.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (without_fragment, None),
    };
    let base = base.trim_end_matches('/');
    match query {
        None => base.to_owned(),
        Some(query) => {
            let kept: Vec<&str> = query
                .split('&')
                .filter(|pair| {
                    let key = pair.split('=').next().unwrap_or("");
                    !(key.starts_with("utm_") || key == "fbclid" || key == "gclid")
                })
                .collect();
            if kept.is_empty() {
                base.to_owned()
            } else {
                format!("{base}?{}", kept.join("&"))
            }
        }
    }
}

fn guidance(query: &str, tried: &[String], has_api_account: bool) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "query: {query}");
    out.push_str("results: none\n");
    if has_api_account {
        out.push_str("reason: configured search providers returned no results or errored\n");
    } else {
        out.push_str("reason: no reliable search provider is configured; DuckDuckGo (the only no-key option) is bot-blocked and returned nothing\n");
    }
    if !tried.is_empty() {
        let _ = writeln!(out, "tried: {}", tried.join(", "));
    }
    out.push_str(
        "note: this does NOT mean the web has no results for this query; the search backend could not return them\n",
    );
    out.push_str(
        "action: run /search to configure a reliable provider (Tavily is free: 1000 queries/month, no credit card)\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use goat_search_provider::{SearchResult, SearchResults, SearchTarget};

    use super::{dedup, guidance, normalize_result_url, rendered_output, trust_rank};

    fn result(url: &str, rank: usize) -> SearchResult {
        SearchResult {
            title: "T".to_owned(),
            url: url.to_owned(),
            snippet: "S".to_owned(),
            rank,
            provider: "duckduckgo/html".to_owned(),
            published_at: None,
        }
    }

    #[test]
    fn dedup_normalizes_tracking_and_slash() {
        let items = vec![
            result("https://a.com/x", 1),
            result("https://a.com/x/", 2),
            result("https://a.com/x?utm_source=n", 3),
            result("https://a.com/y?id=2", 4),
        ];
        let kept = dedup(&items);
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn normalize_strips_fragment_and_tracking() {
        assert_eq!(
            normalize_result_url("https://a.com/p/?utm_medium=x#frag"),
            "https://a.com/p"
        );
        assert_eq!(
            normalize_result_url("https://a.com/p?q=1&gclid=z"),
            "https://a.com/p?q=1"
        );
    }

    #[test]
    fn trust_prefers_api_over_ddg() {
        assert!(trust_rank("tavily") < trust_rank("duckduckgo"));
        assert!(trust_rank("brave") < trust_rank("duckduckgo"));
        assert!(trust_rank("duckduckgo") < trust_rank("browser"));
    }

    #[test]
    fn rendered_keeps_trust_markers_and_summary() {
        let results = SearchResults {
            query: "goat".to_owned(),
            provider: SearchTarget::parse("tavily/default").unwrap(),
            language: None,
            time_range: None,
            limitations: vec!["verify_with_webfetch"],
            results: vec![result("https://example.com", 1)],
        };
        let out = rendered_output(&results, &["duckduckgo/html=empty".to_owned()], false);
        let text = out.as_text().unwrap();
        assert!(text.contains("served_by: tavily/default"));
        assert!(text.contains("fell_back_from: duckduckgo/html=empty"));
        assert!(text.contains("source_trust: untrusted_web"));
        assert!(text.contains("next_step: use WebFetch"));
        assert_eq!(
            out.summary.as_deref(),
            Some("1 result(s) for \"goat\" via tavily/default")
        );
    }

    #[test]
    fn guidance_distinguishes_unconfigured() {
        let unconfigured = guidance("q", &["duckduckgo/html=empty".to_owned()], false);
        assert!(unconfigured.contains("no reliable search provider is configured"));
        assert!(unconfigured.contains("/search"));
        assert!(unconfigured.contains("does NOT mean"));
        let configured = guidance("q", &[], true);
        assert!(configured.contains("configured search providers returned no results"));
    }
}
