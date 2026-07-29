use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use goat_agent_tool::{ToolName, ToolRegistry};
use goat_auth::{Credential, CredentialStore};
use goat_integration::{
    BindingMap, Integration, IntegrationAuth, IntegrationBinding, IntegrationError,
    IntegrationMetadata, IntegrationResult, IntegrationRuntime,
};
use goat_mcp::{HttpEndpoint, McpError, McpSession};
use goat_types::{IntegrationId, ProfileId};
use serde_json::Value;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

mod auth;
mod toolset;

pub use auth::{ResolvedAuth, header_value};
pub use toolset::{CachedTool, ToolDisposition};

pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_mins(2);
pub const MAX_RESULT_BYTES: usize = 96 * 1024;

pub type HeaderFn = fn(&Value) -> HashMap<String, String>;
pub type ToolFilterFn = fn(&CachedTool) -> ToolDisposition;
pub type WatchFn = fn(
    ProfileId,
    &IntegrationBinding,
    &IntegrationRuntime,
    CancellationToken,
) -> Option<JoinHandle<()>>;

pub struct McpService {
    pub name: &'static str,
    pub id: IntegrationId,
    pub display: &'static str,
    pub url: &'static str,
    pub setup: &'static str,
    pub secret_label: &'static str,
    pub env_var: Option<&'static str>,
    pub auth: IntegrationAuth,
    pub auth_scheme: Option<&'static str>,
    pub scopes: &'static [&'static str],
    pub headers: Option<HeaderFn>,
    pub call_timeout: Duration,
    pub truncation_hint: &'static str,
    pub tool_prefix: &'static str,
    pub tool_filter: ToolFilterFn,
    pub watch: Option<WatchFn>,
}

impl McpService {
    #[must_use]
    pub const fn new(
        name: &'static str,
        display: &'static str,
        url: &'static str,
        setup: &'static str,
    ) -> Self {
        Self {
            name,
            id: IntegrationId::from_static(name),
            display,
            url,
            setup,
            secret_label: "",
            env_var: None,
            auth: IntegrationAuth::OAuth,
            auth_scheme: None,
            scopes: &[],
            headers: None,
            call_timeout: DEFAULT_CALL_TIMEOUT,
            truncation_hint: "narrow the request and call again",
            tool_prefix: "",
            tool_filter: allow_every_tool,
            watch: None,
        }
    }

    #[must_use]
    pub const fn with_secret(mut self, label: &'static str) -> Self {
        self.auth = IntegrationAuth::Secret;
        self.secret_label = label;
        self
    }

    #[must_use]
    pub const fn with_env_var(mut self, env_var: &'static str) -> Self {
        self.env_var = Some(env_var);
        self
    }

    #[must_use]
    pub const fn with_auth_scheme(mut self, scheme: &'static str) -> Self {
        self.auth_scheme = Some(scheme);
        self
    }

    #[must_use]
    pub const fn with_scopes(mut self, scopes: &'static [&'static str]) -> Self {
        self.scopes = scopes;
        self
    }

    #[must_use]
    pub const fn with_headers(mut self, headers: HeaderFn) -> Self {
        self.headers = Some(headers);
        self
    }

    #[must_use]
    pub const fn with_call_timeout(mut self, timeout: Duration) -> Self {
        self.call_timeout = timeout;
        self
    }

    #[must_use]
    pub const fn with_truncation_hint(mut self, hint: &'static str) -> Self {
        self.truncation_hint = hint;
        self
    }

    #[must_use]
    pub const fn with_tool_prefix(mut self, prefix: &'static str) -> Self {
        self.tool_prefix = prefix;
        self
    }

    #[must_use]
    pub const fn with_tool_filter(mut self, filter: ToolFilterFn) -> Self {
        self.tool_filter = filter;
        self
    }

    #[must_use]
    pub const fn with_watch(mut self, watch: WatchFn) -> Self {
        self.watch = Some(watch);
        self
    }

    pub async fn connect(
        &self,
        credentials: &CredentialStore,
        binding: &IntegrationBinding,
    ) -> IntegrationResult<McpSession> {
        let client_id = client_id_of(binding);
        let resolved = auth::resolve(self, credentials, &binding.account, client_id.as_deref())?;
        let mut endpoint = HttpEndpoint::new(self.url);
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
                let client = goat_mcp::auth::authorized_client(self.url, store)
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

fn allow_every_tool(_: &CachedTool) -> ToolDisposition {
    ToolDisposition::Enabled
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
    fn id(&self) -> IntegrationId {
        self.service.id.clone()
    }

    fn metadata(&self) -> IntegrationMetadata {
        IntegrationMetadata {
            id: self.service.name,
            display: self.service.display,
            auth: self.service.auth,
            secret_label: self.service.secret_label,
            env_var: self.service.env_var,
            setup: self.service.setup,
            has_watcher: self.service.watch.is_some(),
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

    fn spawn_watcher(
        &self,
        persona: ProfileId,
        binding: IntegrationBinding,
        runtime: IntegrationRuntime,
        cancel: CancellationToken,
    ) -> Option<JoinHandle<()>> {
        let watch = self.service.watch?;
        watch(persona, &binding, &runtime, cancel)
    }

    async fn verify(
        &self,
        config: &Value,
        credentials: &CredentialStore,
    ) -> IntegrationResult<String> {
        let binding = IntegrationBinding::from_config(config.clone());
        let session = self.service.connect(credentials, &binding).await?;
        let name = session
            .peer_name()
            .await
            .unwrap_or_else(|| format!("{} mcp", self.service.id.as_str()));
        session.close().await;
        Ok(name)
    }

    async fn oauth_login(
        &self,
        credentials: &CredentialStore,
        account: &str,
        present_url: &(dyn for<'a> Fn(&'a str) + Send + Sync),
    ) -> IntegrationResult<Value> {
        let authorization =
            goat_mcp::auth::run_login(self.service.url, self.service.scopes, present_url)
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

    const SERVICE: McpService =
        McpService::new("acme", "Acme", "https://mcp.acme.test/mcp", "setup");

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
        let service = SERVICE.with_truncation_hint("add a LIMIT");
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
    fn a_descriptor_becomes_an_integration_that_reports_itself() {
        let integration = McpService::new("acme", "Acme", "https://mcp.acme.test/mcp", "how to")
            .with_env_var("GOAT_ACME_TOKEN")
            .build();
        let meta = integration.metadata();
        assert_eq!(meta.id, "acme");
        assert_eq!(meta.display, "Acme");
        assert_eq!(meta.env_var, Some("GOAT_ACME_TOKEN"));
        assert_eq!(meta.setup, "how to");
        assert!(!meta.has_watcher);
        assert_eq!(integration.id().as_str(), "acme");
    }

    #[test]
    fn declaring_a_watcher_shows_up_in_the_metadata() {
        fn never(
            _: ProfileId,
            _: &IntegrationBinding,
            _: &IntegrationRuntime,
            _: CancellationToken,
        ) -> Option<JoinHandle<()>> {
            None
        }
        let integration = McpService::new("acme", "Acme", "u", "s")
            .with_watch(never)
            .build();
        assert!(integration.metadata().has_watcher);
    }

    #[test]
    fn a_secret_service_advertises_its_label() {
        let integration = McpService::new("acme", "Acme", "u", "s")
            .with_secret("Acme token")
            .build();
        let meta = integration.metadata();
        assert_eq!(meta.auth, IntegrationAuth::Secret);
        assert_eq!(meta.secret_label, "Acme token");
    }

    #[test]
    fn a_blank_client_id_is_treated_as_absent() {
        let binding = IntegrationBinding::from_config(json!({ "client_id": "   " }));
        assert_eq!(client_id_of(&binding), None);
        let binding = IntegrationBinding::from_config(json!({ "client_id": " cid " }));
        assert_eq!(client_id_of(&binding).as_deref(), Some("cid"));
    }
}
