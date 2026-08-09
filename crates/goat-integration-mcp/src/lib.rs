use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use goat_agent_tool::{ToolName, ToolRegistry};
use goat_auth::{Credential, CredentialStore};
use goat_integration::query::WatchVocabulary;
use goat_integration::{
    BindingMap, CompiledWatch, Integration, IntegrationAuth, IntegrationBinding, IntegrationError,
    IntegrationMetadata, IntegrationResult, IntegrationRuntime, WatchSpec,
};
use goat_mcp::{HttpEndpoint, McpError, McpSession};
use goat_types::IntegrationId;
use serde_json::Value;

mod auth;
mod toolset;

pub use auth::{ResolvedAuth, header_value};
pub use toolset::{CachedTool, code_tools, normalized, pick_tool};

pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_mins(2);
pub const MAX_RESULT_BYTES: usize = 96 * 1024;

pub type HeaderFn = fn(&Value) -> HashMap<String, String>;
pub type DescribeIdentityFn = fn(&Value) -> IntegrationResult<String>;

#[derive(Clone, Copy)]
pub struct IdentityProbe {
    pub tool: &'static str,
    pub describe: DescribeIdentityFn,
}
pub type CompileWatchFn =
    fn(&IntegrationBinding, &IntegrationRuntime, &WatchSpec) -> IntegrationResult<CompiledWatch>;
pub type DefaultWatchFn = fn(&IntegrationBinding) -> Vec<WatchSpec>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceUrl {
    Fixed(&'static str),
    FromHost {
        default: &'static str,
        path: &'static str,
    },
}

impl ServiceUrl {
    pub fn resolve(&self, config: &Value) -> IntegrationResult<String> {
        match self {
            Self::Fixed(url) => Ok((*url).to_owned()),
            Self::FromHost { default, path } => {
                let host = config
                    .get("host")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|host| !host.is_empty());
                let Some(host) = host else {
                    return Ok(format!("{}{path}", default.trim_end_matches('/')));
                };
                let host = host.trim_end_matches('/');
                if !host.starts_with("https://") && !host.starts_with("http://") {
                    return Err(IntegrationError::Config(format!(
                        "`host` must start with http:// or https://, got `{host}`"
                    )));
                }
                Ok(format!("{host}{path}"))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthScheme {
    Raw,
    Bearer,
    Custom(&'static str),
    Basic,
}

#[derive(Clone, Copy, Debug)]
pub struct CredentialSpec {
    pub auth: IntegrationAuth,
    pub scheme: AuthScheme,
    pub label: &'static str,
    pub env_var: Option<&'static str>,
    pub scopes: &'static [&'static str],
}

#[derive(Clone, Copy, Debug)]
pub struct ToolPolicy {
    pub prefix: &'static str,
    pub enable: Enable,
    pub deny: &'static [NameRule],
}

#[derive(Clone, Copy, Debug)]
pub enum Enable {
    All,
    Only(&'static [&'static str]),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameRule {
    Prefix(&'static str),
    Suffix(&'static str),
}

impl ToolPolicy {
    #[must_use]
    pub const fn all(prefix: &'static str) -> Self {
        Self {
            prefix,
            enable: Enable::All,
            deny: &[],
        }
    }

    #[must_use]
    pub const fn only(prefix: &'static str, names: &'static [&'static str]) -> Self {
        Self {
            prefix,
            enable: Enable::Only(names),
            deny: &[],
        }
    }

    #[must_use]
    pub const fn deny(mut self, rules: &'static [NameRule]) -> Self {
        self.deny = rules;
        self
    }
}

pub struct McpService {
    pub name: &'static str,
    pub id: IntegrationId,
    pub display: &'static str,
    pub url: ServiceUrl,
    pub setup: &'static str,
    pub credential: CredentialSpec,
    pub headers: Option<HeaderFn>,
    pub call_timeout: Duration,
    pub truncation_hint: &'static str,
    pub tools: ToolPolicy,
    pub identity: Option<IdentityProbe>,
    pub vocabulary: Option<&'static WatchVocabulary>,
    pub compile: Option<CompileWatchFn>,
    pub defaults: Option<DefaultWatchFn>,
}

impl McpService {
    #[must_use]
    pub const fn new(
        name: &'static str,
        display: &'static str,
        url: ServiceUrl,
        setup: &'static str,
    ) -> Self {
        Self {
            name,
            id: IntegrationId::from_static(name),
            display,
            url,
            setup,
            credential: CredentialSpec {
                auth: IntegrationAuth::OAuth,
                scheme: AuthScheme::Raw,
                label: "",
                env_var: None,
                scopes: &[],
            },
            headers: None,
            call_timeout: DEFAULT_CALL_TIMEOUT,
            truncation_hint: "narrow the request and call again",
            tools: ToolPolicy::all(""),
            identity: None,
            vocabulary: None,
            compile: None,
            defaults: None,
        }
    }

    #[must_use]
    pub const fn secret(mut self, label: &'static str, scheme: AuthScheme) -> Self {
        self.credential.auth = IntegrationAuth::Secret;
        self.credential.label = label;
        self.credential.scheme = scheme;
        self
    }

    #[must_use]
    pub const fn oauth(mut self, scopes: &'static [&'static str]) -> Self {
        self.credential.auth = IntegrationAuth::OAuth;
        self.credential.scopes = scopes;
        self
    }

    #[must_use]
    pub const fn token_scheme(mut self, scheme: AuthScheme) -> Self {
        self.credential.scheme = scheme;
        self
    }

    #[must_use]
    pub const fn env_var(mut self, env_var: &'static str) -> Self {
        self.credential.env_var = Some(env_var);
        self
    }

    #[must_use]
    pub const fn headers(mut self, headers: HeaderFn) -> Self {
        self.headers = Some(headers);
        self
    }

    #[must_use]
    pub const fn call_timeout(mut self, timeout: Duration) -> Self {
        self.call_timeout = timeout;
        self
    }

    #[must_use]
    pub const fn truncation_hint(mut self, hint: &'static str) -> Self {
        self.truncation_hint = hint;
        self
    }

    #[must_use]
    pub const fn tools(mut self, policy: ToolPolicy) -> Self {
        self.tools = policy;
        self
    }

    #[must_use]
    pub const fn identity(mut self, probe: IdentityProbe) -> Self {
        self.identity = Some(probe);
        self
    }

    #[must_use]
    pub const fn watch(
        mut self,
        vocabulary: &'static WatchVocabulary,
        compile: CompileWatchFn,
    ) -> Self {
        self.vocabulary = Some(vocabulary);
        self.compile = Some(compile);
        self
    }

    #[must_use]
    pub const fn defaults(mut self, defaults: DefaultWatchFn) -> Self {
        self.defaults = Some(defaults);
        self
    }

    pub async fn connect(
        &self,
        credentials: &CredentialStore,
        binding: &IntegrationBinding,
    ) -> IntegrationResult<McpSession> {
        let url = self.url.resolve(&binding.config)?;
        let client_id = client_id_of(binding);
        let resolved = auth::resolve(
            self.id.as_str(),
            &self.credential,
            credentials,
            &binding.account,
            client_id.as_deref(),
        )?;
        let mut endpoint = HttpEndpoint::new(url.clone());
        if let Some(headers) = self.headers {
            for (name, value) in headers(&binding.config) {
                endpoint = endpoint.with_header(name, value);
            }
        }
        let name = self.id.as_str().to_owned();
        match resolved {
            ResolvedAuth::Token(token) => {
                let endpoint = endpoint.with_auth_header(token);
                let client = goat_mcp::http_client().map_err(|e| self.wire_error(&e))?;
                McpSession::connect_http(name, &endpoint, client)
                    .await
                    .map_err(|e| self.wire_error(&e))
            }
            ResolvedAuth::OAuth(store) => {
                let client = goat_mcp::auth::authorized_client(&url, store)
                    .await
                    .map_err(|e| self.wire_error(&e))?;
                McpSession::connect_http(name, &endpoint, client)
                    .await
                    .map_err(|e| self.wire_error(&e))
            }
        }
    }

    pub async fn call(
        &self,
        session: &McpSession,
        tool: &str,
        arguments: Value,
    ) -> IntegrationResult<Value> {
        let result = session
            .call(tool, arguments)
            .await
            .map_err(|e| self.wire_error(&e))?;
        if result.is_error {
            return Err(self.classify(&result.error_message()));
        }
        Ok(self.cap(result.value()))
    }

    pub fn cap(&self, value: Value) -> Value {
        let rendered = value.to_string();
        if rendered.len() <= MAX_RESULT_BYTES {
            return value;
        }
        let mut end = MAX_RESULT_BYTES;
        while end > 0 && !rendered.is_char_boundary(end) {
            end -= 1;
        }
        serde_json::json!({
            "truncated": true,
            "bytes_returned": rendered.len(),
            "bytes_kept": end,
            "note": format!(
                "result truncated at {MAX_RESULT_BYTES} bytes; {}",
                self.truncation_hint
            ),
            "partial": &rendered[..end],
        })
    }

    pub fn wire_error(&self, error: &McpError) -> IntegrationError {
        match error {
            McpError::InvalidParams { message, .. } => IntegrationError::Service(format!(
                "{} rejected the arguments: {message}",
                self.id.as_str()
            )),
            other => self.classify(&other.to_string()),
        }
    }

    pub fn classify(&self, rendered: &str) -> IntegrationError {
        let lowered = rendered.to_ascii_lowercase();
        if looks_like_auth_failure(&lowered) {
            return IntegrationError::Auth(format!("{}: {rendered}", self.id.as_str()));
        }
        if let Some(seconds) = retry_after_seconds(&lowered) {
            return IntegrationError::Service(format!(
                "{} is rate limited; retry after {seconds}s ({rendered})",
                self.id.as_str()
            ));
        }
        if looks_like_rate_limit(&lowered) {
            return IntegrationError::Service(format!(
                "{} is rate limited ({rendered})",
                self.id.as_str()
            ));
        }
        IntegrationError::Service(format!(
            "{} mcp request failed: {rendered}",
            self.id.as_str()
        ))
    }

    #[must_use]
    pub fn build(self) -> McpIntegration {
        McpIntegration {
            service: Arc::new(self),
        }
    }
}

pub const COMMON_BINDING_KEYS: &[&str] = &["account", "client_id"];
pub const COMMON_DENY_KEYS: &[&str] = &["deny_prefixes", "deny_suffixes"];

pub fn validate_binding<T>(name: &str, config: &Value) -> IntegrationResult<()>
where
    T: serde::de::DeserializeOwned,
{
    let object = config
        .as_object()
        .ok_or_else(|| IntegrationError::Config(format!("{name} binding must be an object")))?;
    for key in COMMON_BINDING_KEYS {
        if let Some(value) = object.get(*key)
            && !value.is_string()
        {
            return Err(IntegrationError::Config(format!(
                "{name} binding: `{key}` must be a string"
            )));
        }
    }
    for key in COMMON_DENY_KEYS {
        if let Some(value) = object.get(*key)
            && !value
                .as_array()
                .is_some_and(|items| items.iter().all(Value::is_string))
        {
            return Err(IntegrationError::Config(format!(
                "{name} binding: `{key}` must be an array of strings"
            )));
        }
    }
    serde_json::from_value::<T>(Value::Object(leaf_keys(object)))
        .map(|_| ())
        .map_err(|e| IntegrationError::Config(format!("{name} binding: {e}")))
}

pub fn read_binding<T>(config: &Value) -> T
where
    T: serde::de::DeserializeOwned + Default,
{
    let Some(object) = config.as_object() else {
        return T::default();
    };
    serde_json::from_value(Value::Object(leaf_keys(object))).unwrap_or_default()
}

fn leaf_keys(object: &serde_json::Map<String, Value>) -> serde_json::Map<String, Value> {
    object
        .iter()
        .filter(|(key, _)| {
            !COMMON_BINDING_KEYS.contains(&key.as_str())
                && !COMMON_DENY_KEYS.contains(&key.as_str())
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

pub fn client_id_of(binding: &IntegrationBinding) -> Option<String> {
    binding
        .config
        .get("client_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn looks_like_auth_failure(lowered: &str) -> bool {
    [
        "401",
        "403",
        "unauthorized",
        "invalid_token",
        "invalid_grant",
        "authorization required",
        "authorizationrequired",
    ]
    .iter()
    .any(|marker| lowered.contains(marker))
}

fn looks_like_rate_limit(lowered: &str) -> bool {
    ["429", "rate limit", "rate_limit", "too many requests"]
        .iter()
        .any(|marker| lowered.contains(marker))
}

fn retry_after_seconds(lowered: &str) -> Option<u64> {
    let index = lowered.find("retry-after")?;
    let tail = &lowered[index + "retry-after".len()..];
    let digits: String = tail
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

pub struct McpIntegration {
    service: Arc<McpService>,
}

impl McpIntegration {
    #[must_use]
    pub fn service(&self) -> &Arc<McpService> {
        &self.service
    }
}

#[async_trait]
impl Integration for McpIntegration {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn id(&self) -> IntegrationId {
        self.service.id.clone()
    }

    fn metadata(&self) -> IntegrationMetadata {
        IntegrationMetadata {
            id: self.service.name,
            display: self.service.display,
            auth: self.service.credential.auth,
            secret_label: self.service.credential.label,
            env_var: self.service.credential.env_var,
            setup: self.service.setup,
        }
    }

    async fn register_tools(
        &self,
        registry: &mut ToolRegistry,
        runtime: &IntegrationRuntime,
        bindings: Arc<BindingMap>,
    ) -> Vec<ToolName> {
        toolset::register(&self.service, registry, runtime, bindings).await
    }

    fn default_watch(&self, binding: &IntegrationBinding) -> Vec<WatchSpec> {
        match self.service.defaults {
            Some(defaults) => defaults(binding),
            None => Vec::new(),
        }
    }

    fn watch_vocabulary(&self) -> Option<&'static WatchVocabulary> {
        self.service.vocabulary
    }

    fn compile_watch(
        &self,
        binding: &IntegrationBinding,
        runtime: &IntegrationRuntime,
        spec: &WatchSpec,
    ) -> IntegrationResult<CompiledWatch> {
        match self.service.compile {
            Some(compile) => compile(binding, runtime, spec),
            None => Err(IntegrationError::Config(format!(
                "{} does not support watch queries",
                self.service.id.as_str()
            ))),
        }
    }

    async fn verify(
        &self,
        config: &Value,
        credentials: &CredentialStore,
    ) -> IntegrationResult<String> {
        let binding = IntegrationBinding::from_config(config.clone());
        let session = self.service.connect(credentials, &binding).await?;
        let described = match self.service.identity {
            Some(probe) => {
                let value = self.service.call(&session, probe.tool, Value::Null).await;
                value.and_then(|value| (probe.describe)(&value))
            }
            None => Ok(session
                .peer_name()
                .await
                .unwrap_or_else(|| format!("{} mcp", self.service.id.as_str()))),
        };
        session.close().await;
        described
    }

    async fn oauth_login(
        &self,
        credentials: &CredentialStore,
        account: &str,
        present_url: &(dyn for<'a> Fn(&'a str) + Send + Sync),
    ) -> IntegrationResult<Value> {
        let url = self.service.url.resolve(&Value::Null)?;
        let authorization =
            goat_mcp::auth::run_login(&url, self.service.credential.scopes, present_url)
                .await
                .map_err(|e| IntegrationError::Auth(e.to_string()))?;
        credentials
            .store(
                &goat_auth::CredentialKey::integration(self.service.id.as_str(), account),
                Credential::OAuth(authorization.tokens),
            )
            .map_err(|e| IntegrationError::Auth(e.to_string()))?;
        Ok(serde_json::json!({ "client_id": authorization.client_id }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SERVICE: McpService = McpService::new(
        "acme",
        "Acme",
        ServiceUrl::Fixed("https://mcp.acme.test/mcp"),
        "setup",
    );

    #[test]
    fn auth_markers_are_the_union_of_what_the_leaves_used_to_check() {
        for rendered in [
            "HTTP 401",
            "403 Forbidden",
            "Unauthorized",
            "invalid_token",
            "invalid_grant",
            "authorization required",
            "AuthorizationRequired",
        ] {
            assert!(
                matches!(SERVICE.classify(rendered), IntegrationError::Auth(_)),
                "{rendered} should classify as auth"
            );
        }
    }

    #[test]
    fn a_rate_limit_with_a_retry_after_reports_the_delay() {
        let error = SERVICE.classify("429 too many requests, retry-after: 42");
        let IntegrationError::Service(message) = error else {
            panic!("expected a service error");
        };
        assert!(message.contains("retry after 42s"));
    }

    #[test]
    fn a_rate_limit_without_a_delay_is_still_recognised() {
        let error = SERVICE.classify("rate limit exceeded");
        let IntegrationError::Service(message) = error else {
            panic!("expected a service error");
        };
        assert!(message.contains("rate limited"));
        assert!(!message.contains("retry after"));
    }

    #[test]
    fn anything_else_keeps_the_original_text() {
        let error = SERVICE.classify("boom");
        assert!(matches!(error, IntegrationError::Service(m) if m.contains("boom")));
    }

    #[test]
    fn a_small_result_passes_through_uncapped() {
        let value = json!({ "a": 1 });
        assert_eq!(SERVICE.cap(value.clone()), value);
    }

    #[test]
    fn an_oversized_result_is_capped_with_a_service_specific_hint() {
        let service = SERVICE.truncation_hint("add a LIMIT");
        let big = json!({ "rows": "x".repeat(MAX_RESULT_BYTES) });
        let capped = service.cap(big);
        assert_eq!(capped["truncated"], true);
        assert!(capped["note"].as_str().unwrap().contains("add a LIMIT"));
        assert!(capped["partial"].as_str().unwrap().len() <= MAX_RESULT_BYTES);
    }

    #[test]
    fn the_cap_never_splits_a_character() {
        let big = json!({ "text": "가".repeat(MAX_RESULT_BYTES) });
        let capped = SERVICE.cap(big);
        assert!(capped["partial"].as_str().is_some());
    }

    #[test]
    fn a_fixed_url_resolves_to_itself() {
        let url = ServiceUrl::Fixed("https://mcp.acme.test/mcp");
        assert_eq!(
            url.resolve(&json!({})).unwrap(),
            "https://mcp.acme.test/mcp"
        );
        assert_eq!(
            url.resolve(&Value::Null).unwrap(),
            "https://mcp.acme.test/mcp"
        );
    }

    #[test]
    fn a_hosted_url_falls_back_to_its_default() {
        let url = ServiceUrl::FromHost {
            default: "https://cloud.acme.test",
            path: "/api/mcp",
        };
        assert_eq!(
            url.resolve(&json!({})).unwrap(),
            "https://cloud.acme.test/api/mcp"
        );
        assert_eq!(
            url.resolve(&json!({ "host": "  " })).unwrap(),
            "https://cloud.acme.test/api/mcp"
        );
    }

    #[test]
    fn a_hosted_url_takes_the_binding_host_and_trims_the_trailing_slash() {
        let url = ServiceUrl::FromHost {
            default: "https://cloud.acme.test",
            path: "/api/mcp",
        };
        assert_eq!(
            url.resolve(&json!({ "host": "https://eu.acme.test/" }))
                .unwrap(),
            "https://eu.acme.test/api/mcp"
        );
        assert_eq!(
            url.resolve(&json!({ "host": " http://acme.internal:3000 " }))
                .unwrap(),
            "http://acme.internal:3000/api/mcp"
        );
    }

    #[test]
    fn a_host_without_a_scheme_is_rejected() {
        let url = ServiceUrl::FromHost {
            default: "https://cloud.acme.test",
            path: "/api/mcp",
        };
        let err = url.resolve(&json!({ "host": "eu.acme.test" })).unwrap_err();
        assert!(matches!(err, IntegrationError::Config(m) if m.contains("http")));
    }

    #[test]
    fn a_descriptor_becomes_an_integration_that_reports_itself() {
        let integration = McpService::new(
            "acme",
            "Acme",
            ServiceUrl::Fixed("https://mcp.acme.test/mcp"),
            "how to",
        )
        .env_var("GOAT_ACME_TOKEN")
        .build();
        let meta = integration.metadata();
        assert_eq!(meta.id, "acme");
        assert_eq!(meta.display, "Acme");
        assert_eq!(meta.env_var, Some("GOAT_ACME_TOKEN"));
        assert_eq!(meta.setup, "how to");
        assert_eq!(integration.id().as_str(), "acme");
    }

    #[test]
    fn declaring_watch_hooks_shows_up_on_the_descriptor() {
        fn no_defaults(_: &IntegrationBinding) -> Vec<WatchSpec> {
            Vec::new()
        }
        fn no_compile(
            _: &IntegrationBinding,
            _: &IntegrationRuntime,
            _: &WatchSpec,
        ) -> IntegrationResult<CompiledWatch> {
            Err(IntegrationError::Config("no".into()))
        }
        static VOCABULARY: WatchVocabulary = WatchVocabulary {
            integration: "acme",
            residue: goat_integration::query::Residue::Reject,
            terms: goat_integration::query::TermPolicy::Reject,
            limit: None,
            keys: &[],
        };
        let integration = McpService::new("acme", "Acme", ServiceUrl::Fixed("u"), "s")
            .defaults(no_defaults)
            .watch(&VOCABULARY, no_compile)
            .build();
        assert!(integration.service().compile.is_some());
        assert!(integration.service().defaults.is_some());
        assert!(integration.watch_vocabulary().is_some());
    }

    #[test]
    fn a_secret_service_advertises_its_label_and_keeps_its_scheme() {
        let integration = McpService::new("acme", "Acme", ServiceUrl::Fixed("u"), "s")
            .secret("Acme token", AuthScheme::Basic)
            .build();
        let meta = integration.metadata();
        assert_eq!(meta.auth, IntegrationAuth::Secret);
        assert_eq!(meta.secret_label, "Acme token");
        assert_eq!(integration.service().credential.scheme, AuthScheme::Basic);
    }

    #[test]
    fn an_oauth_service_cannot_carry_a_secret_label() {
        let service = McpService::new("acme", "Acme", ServiceUrl::Fixed("u"), "s").oauth(&["read"]);
        assert_eq!(service.credential.auth, IntegrationAuth::OAuth);
        assert_eq!(service.credential.label, "");
        assert_eq!(service.credential.scopes, &["read"]);
    }

    #[test]
    fn the_base_owns_the_common_keys_so_leaves_need_not_declare_them() {
        #[derive(Debug, Default, serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Leaf {
            #[serde(default)]
            project: Option<String>,
        }

        assert!(validate_binding::<Leaf>("acme", &json!({})).is_ok());
        assert!(
            validate_binding::<Leaf>("acme", &json!({ "account": "work", "client_id": "cid" }))
                .is_ok()
        );
        assert!(validate_binding::<Leaf>("acme", &json!({ "deny_suffixes": ["-delete"] })).is_ok());
        assert!(validate_binding::<Leaf>("acme", &json!({ "deny_prefixes": ["delete"] })).is_ok());
        assert!(validate_binding::<Leaf>("acme", &json!({ "project": "p" })).is_ok());
        let read: Leaf = read_binding(&json!({ "account": "work", "project": "p" }));
        assert_eq!(read.project.as_deref(), Some("p"));
    }

    #[test]
    fn an_unknown_leaf_key_is_rejected() {
        #[derive(Debug, Default, serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Leaf {
            #[serde(default)]
            project: Option<String>,
        }

        let err = validate_binding::<Leaf>("acme", &json!({ "prject": "typo" }));
        assert!(err.is_err());
        assert!(validate_binding::<Leaf>("acme", &json!("nope")).is_err());
        let read: Leaf = read_binding(&json!({ "project": "p" }));
        assert_eq!(read.project.as_deref(), Some("p"));
    }

    #[test]
    fn a_common_key_of_the_wrong_type_is_rejected() {
        #[derive(Debug, Default, serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Leaf {}

        assert!(validate_binding::<Leaf>("acme", &json!({ "account": 3 })).is_err());
        assert!(validate_binding::<Leaf>("acme", &json!({ "client_id": false })).is_err());
        assert!(validate_binding::<Leaf>("acme", &json!({ "deny_suffixes": "-delete" })).is_err());
        assert!(validate_binding::<Leaf>("acme", &json!({ "deny_prefixes": [3] })).is_err());
    }

    #[test]
    fn reading_a_binding_ignores_the_common_keys_and_survives_junk() {
        #[derive(Debug, Default, serde::Deserialize)]
        struct Leaf {
            #[serde(default)]
            project: Option<String>,
        }

        let read: Leaf = read_binding(&json!({ "account": "work", "project": "p" }));
        assert_eq!(read.project.as_deref(), Some("p"));
        let read: Leaf = read_binding(&json!("not an object"));
        assert!(read.project.is_none());
    }

    #[test]
    fn a_blank_client_id_is_treated_as_absent() {
        let binding = IntegrationBinding::from_config(json!({ "client_id": "   " }));
        assert_eq!(client_id_of(&binding), None);
        let binding = IntegrationBinding::from_config(json!({ "client_id": " cid " }));
        assert_eq!(client_id_of(&binding).as_deref(), Some("cid"));
    }
}
