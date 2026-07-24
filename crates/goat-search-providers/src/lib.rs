use goat_auth::CredentialStore;
use goat_search_provider::{
    SearchAccountConfig, SearchBuiltinTarget, SearchCredentialMetadata, SearchError,
    SearchProvider, SearchProviderKind, SearchProviderMetadata, SearchRequest, SearchResults,
    SearchTarget, SearchTargetMetadata,
};

const DEFAULT_TARGET: &str = "browser/duckduckgo";

pub fn default_search_target() -> &'static str {
    DEFAULT_TARGET
}

pub fn metadata() -> Vec<SearchProviderMetadata> {
    vec![
        goat_search_provider_duckduckgo::browser_metadata(),
        goat_search_provider_duckduckgo::duckduckgo_metadata(),
        goat_search_provider_searxng::metadata(),
        goat_search_provider_brave::metadata(),
        goat_search_provider_tavily::metadata(),
    ]
}

pub fn search_providers() -> Vec<SearchProviderMetadata> {
    metadata()
}

pub fn search_provider(id: &str) -> Option<SearchProviderMetadata> {
    metadata().into_iter().find(|provider| provider.id == id)
}

pub fn search_builtin_targets() -> Vec<SearchTargetMetadata<'static>> {
    metadata()
        .into_iter()
        .flat_map(|provider| {
            provider
                .builtins
                .iter()
                .map(move |builtin| builtin_target(provider.credential, builtin))
        })
        .collect()
}

fn builtin_target(
    credential: SearchCredentialMetadata,
    builtin: &SearchBuiltinTarget,
) -> SearchTargetMetadata<'static> {
    SearchTargetMetadata {
        provider: builtin.provider,
        account: builtin.account,
        target: builtin.target,
        kind: builtin.kind,
        setup: builtin.setup,
        credential,
    }
}

pub fn is_builtin_search_target(target: &str) -> bool {
    search_builtin_targets()
        .into_iter()
        .any(|builtin| builtin.target == target)
}

pub fn build_search_account_config(
    provider: &str,
    account: &str,
    endpoint: Option<&str>,
    engine: Option<&str>,
) -> Result<SearchAccountConfig, String> {
    let metadata =
        search_provider(provider).ok_or_else(|| format!("unknown search provider: {provider}"))?;
    match metadata.kind {
        SearchProviderKind::Browser { default_engine } => {
            let engine = engine.unwrap_or(default_engine);
            if engine != default_engine {
                return Err(format!("unsupported browser search engine: {engine}"));
            }
            Ok(SearchAccountConfig::Browser {
                account: account.to_owned(),
                engine: engine.to_owned(),
            })
        }
        SearchProviderKind::Duckduckgo => Ok(SearchAccountConfig::Duckduckgo {
            account: account.to_owned(),
        }),
        SearchProviderKind::Searxng => Ok(SearchAccountConfig::Searxng {
            account: account.to_owned(),
            endpoint: endpoint
                .ok_or_else(|| "searxng requires --endpoint".to_owned())?
                .to_owned(),
        }),
        SearchProviderKind::Brave => Ok(SearchAccountConfig::Brave {
            account: account.to_owned(),
        }),
        SearchProviderKind::Tavily => Ok(SearchAccountConfig::Tavily {
            account: account.to_owned(),
        }),
    }
}

pub fn configured_search_target(account: &SearchAccountConfig) -> SearchTargetMetadata<'_> {
    let provider = configured_search_provider(account);
    let metadata = search_provider(provider).expect("configured search provider metadata");
    SearchTargetMetadata {
        provider,
        account: configured_search_account(account),
        target: "",
        kind: "configured",
        setup: metadata.setup,
        credential: metadata.credential,
    }
}

pub fn configured_search_provider(account: &SearchAccountConfig) -> &'static str {
    match account {
        SearchAccountConfig::Duckduckgo { .. } => "duckduckgo",
        SearchAccountConfig::Browser { .. } => "browser",
        SearchAccountConfig::Searxng { .. } => "searxng",
        SearchAccountConfig::Brave { .. } => "brave",
        SearchAccountConfig::Tavily { .. } => "tavily",
    }
}

pub fn configured_search_account(account: &SearchAccountConfig) -> &str {
    match account {
        SearchAccountConfig::Duckduckgo { account }
        | SearchAccountConfig::Browser { account, .. }
        | SearchAccountConfig::Searxng { account, .. }
        | SearchAccountConfig::Brave { account }
        | SearchAccountConfig::Tavily { account } => account,
    }
}

pub struct SearchRegistry {
    providers: Vec<Box<dyn SearchProvider>>,
    default_target: SearchTarget,
    credentials: Option<CredentialStore>,
}

impl SearchRegistry {
    pub fn from_parts(
        default_target: Option<&str>,
        accounts: Vec<SearchAccountConfig>,
        credentials: Option<CredentialStore>,
    ) -> Self {
        let default_target = default_target
            .and_then(SearchTarget::parse)
            .unwrap_or_else(|| SearchTarget::parse(DEFAULT_TARGET).expect("valid default target"));
        let providers = providers_from_accounts(accounts);
        let default_target = if providers
            .iter()
            .any(|provider| provider.target() == default_target)
        {
            default_target
        } else {
            SearchTarget::parse(DEFAULT_TARGET).expect("valid default target")
        };
        Self {
            providers,
            default_target,
            credentials,
        }
    }

    pub async fn search(&self, request: SearchRequest) -> Result<SearchResults, SearchError> {
        let target = match request.target.as_deref() {
            Some(raw) => SearchTarget::parse(raw)
                .ok_or_else(|| SearchError::InvalidTarget(raw.to_owned()))?,
            None => self.default_target.clone(),
        };
        let provider = self
            .providers
            .iter()
            .find(|provider| provider.target() == target)
            .ok_or_else(|| SearchError::UnknownTarget(target.as_str()))?;
        provider.search(request, self.credentials.as_ref()).await
    }
}

pub fn providers_from_accounts(accounts: Vec<SearchAccountConfig>) -> Vec<Box<dyn SearchProvider>> {
    let mut providers: Vec<Box<dyn SearchProvider>> = vec![
        Box::new(goat_search_provider_duckduckgo::BrowserDuckDuckGoProvider::new("duckduckgo")),
        Box::new(goat_search_provider_duckduckgo::DuckDuckGoHtmlProvider::new("html")),
    ];
    for account in accounts {
        match account {
            SearchAccountConfig::Duckduckgo { account } => providers.push(Box::new(
                goat_search_provider_duckduckgo::DuckDuckGoHtmlProvider::new(account),
            )),
            SearchAccountConfig::Browser { account, engine } => {
                if engine == "duckduckgo" {
                    providers.push(Box::new(
                        goat_search_provider_duckduckgo::BrowserDuckDuckGoProvider::new(account),
                    ));
                }
            }
            SearchAccountConfig::Searxng { account, endpoint } => providers.push(Box::new(
                goat_search_provider_searxng::SearxngProvider::new(account, endpoint),
            )),
            SearchAccountConfig::Brave { account } => providers.push(Box::new(
                goat_search_provider_brave::BraveProvider::new(account),
            )),
            SearchAccountConfig::Tavily { account } => providers.push(Box::new(
                goat_search_provider_tavily::TavilyProvider::new(account),
            )),
        }
    }
    providers
}
