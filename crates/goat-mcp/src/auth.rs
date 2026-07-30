use std::sync::Arc;

use goat_auth::{
    Credential, CredentialKey, CredentialStore as GoatCredentialStore, SecretString, TokenSet,
};
use rmcp::transport::auth::{
    AuthClient, AuthError, AuthorizationManager, AuthorizationRequest, CredentialStore, OAuthState,
    OAuthTokenResponse, StoredCredentials,
};
use serde_json::{Value, json};
use tracing::warn;

use crate::McpError;

pub use rmcp::transport::auth::AuthClient as McpAuthClient;

pub struct StoredOAuth {
    credentials: GoatCredentialStore,
    key: CredentialKey,
    fallback_client_id: Option<String>,
}

impl StoredOAuth {
    pub fn new(
        credentials: GoatCredentialStore,
        key: CredentialKey,
        fallback_client_id: Option<String>,
    ) -> Self {
        Self {
            credentials,
            key,
            fallback_client_id,
        }
    }
}

#[async_trait::async_trait]
impl CredentialStore for StoredOAuth {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let Some(Credential::OAuth(tokens)) = self.credentials.get(&self.key) else {
            return Ok(None);
        };
        let Some(client_id) = tokens
            .client_id
            .clone()
            .or_else(|| self.fallback_client_id.clone())
        else {
            return Ok(None);
        };
        let response =
            response_from_tokens(&tokens).map_err(|e| AuthError::InternalError(e.to_string()))?;
        Ok(Some(
            StoredCredentials::new(client_id, Some(response), tokens.scopes.clone(), None)
                .with_issuer(tokens.issuer.clone()),
        ))
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let Some(response) = credentials.token_response else {
            return Ok(());
        };
        let fresh = tokens_from_response(&response)
            .map_err(|e| AuthError::InternalError(e.to_string()))?
            .with_client(credentials.client_id)
            .with_scopes(credentials.granted_scopes)
            .with_issuer(credentials.issuer);
        let unchanged = matches!(
            self.credentials.get(&self.key),
            Some(Credential::OAuth(old)) if old == fresh
        );
        if unchanged {
            return Ok(());
        }
        if let Err(e) = self.credentials.store(&self.key, Credential::OAuth(fresh)) {
            warn!(error = %e, key = ?self.key, "failed to persist refreshed mcp tokens");
            return Err(AuthError::InternalError(e.to_string()));
        }
        Ok(())
    }

    async fn clear(&self) -> Result<(), AuthError> {
        if let Err(e) = self.credentials.remove(&self.key) {
            warn!(error = %e, key = ?self.key, "failed to clear mcp tokens");
        }
        Ok(())
    }
}

pub fn tokens_from_response(response: &OAuthTokenResponse) -> Result<TokenSet, McpError> {
    let raw = serde_json::to_value(response).map_err(McpError::Json)?;
    let access = raw
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::Initialize {
            server: "oauth".to_owned(),
            message: "token response missing access_token".to_owned(),
        })?;
    Ok(TokenSet {
        access_token: SecretString::from(access),
        refresh_token: raw
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(SecretString::from),
        expires_at: raw
            .get("expires_in")
            .and_then(Value::as_i64)
            .map(|secs| chrono::Utc::now().timestamp() + secs),
        ..TokenSet::default()
    })
}

pub fn response_from_tokens(tokens: &TokenSet) -> Result<OAuthTokenResponse, McpError> {
    let mut raw = json!({
        "access_token": tokens.access_token.expose(),
        "token_type": "bearer",
    });
    if let Some(refresh) = &tokens.refresh_token {
        raw["refresh_token"] = json!(refresh.expose());
    }
    if let Some(expires_at) = tokens.expires_at {
        raw["expires_in"] = json!((expires_at - chrono::Utc::now().timestamp()).max(0));
    }
    serde_json::from_value(raw).map_err(McpError::Json)
}

pub async fn authorized_client(
    url: &str,
    store: StoredOAuth,
) -> Result<AuthClient<rmcp_reqwest::Client>, McpError> {
    let mut manager = AuthorizationManager::new(url).await.map_err(auth_failed)?;
    manager.set_credential_store(store);
    if !manager.initialize_from_store().await.map_err(auth_failed)? {
        return Err(McpError::Initialize {
            server: url.to_owned(),
            message: "stored oauth credentials did not authorize".to_owned(),
        });
    }
    Ok(AuthClient::new(crate::http_client()?, manager))
}

pub struct Authorization {
    pub client_id: String,
    pub tokens: TokenSet,
}

pub async fn run_login(
    url: &str,
    scopes: &[&str],
    present_url: &(dyn for<'a> Fn(&'a str) + Send + Sync),
) -> Result<Authorization, McpError> {
    let issuer = AuthorizationManager::new(url)
        .await
        .map_err(auth_failed)?
        .resolve_metadata()
        .await
        .map_err(auth_failed)?
        .metadata
        .issuer;
    let (listener, port) = goat_auth::bind_loopback()
        .await
        .map_err(|e| auth_failed(e.to_string()))?;
    let redirect = format!("http://127.0.0.1:{port}/callback");

    let mut oauth = OAuthState::new(url, None).await.map_err(auth_failed)?;
    let mut request = AuthorizationRequest::new(&redirect)
        .with_client_name("goat")
        .with_application_type("native");
    if !scopes.is_empty() {
        request = request.with_scopes(scopes.iter().copied());
    }
    oauth
        .start_authorization(request)
        .await
        .map_err(auth_failed)?;

    let auth_url = oauth.get_authorization_url().await.map_err(auth_failed)?;
    let state = state_of(&auth_url).ok_or_else(|| McpError::Initialize {
        server: url.to_owned(),
        message: "authorization url missing state".to_owned(),
    })?;

    present_url(&auth_url);
    let code = goat_auth::capture_on(listener, &state)
        .await
        .map_err(|e| auth_failed(e.to_string()))?;
    oauth
        .handle_callback(&code, &state)
        .await
        .map_err(auth_failed)?;

    let (client_id, tokens) = oauth.get_credentials().await.map_err(auth_failed)?;
    let tokens = tokens.ok_or_else(|| McpError::Initialize {
        server: url.to_owned(),
        message: "no tokens after authorization".to_owned(),
    })?;
    let granted = granted_scopes(&tokens);
    Ok(Authorization {
        client_id: client_id.clone(),
        tokens: tokens_from_response(&tokens)?
            .with_client(client_id)
            .with_scopes(granted)
            .with_issuer(issuer),
    })
}

fn granted_scopes(response: &OAuthTokenResponse) -> Vec<String> {
    serde_json::to_value(response)
        .ok()
        .and_then(|raw| {
            raw.get("scope")
                .and_then(Value::as_str)
                .map(|scope| scope.split_whitespace().map(str::to_owned).collect())
        })
        .unwrap_or_default()
}

fn state_of(auth_url: &str) -> Option<String> {
    url::Url::parse(auth_url).ok().and_then(|parsed| {
        parsed
            .query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.to_string())
    })
}

fn auth_failed(error: impl std::fmt::Display) -> McpError {
    McpError::Initialize {
        server: "oauth".to_owned(),
        message: error.to_string(),
    }
}

pub type SharedOAuth = Arc<StoredOAuth>;

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(access: &str, refresh: Option<&str>, expires_at: Option<i64>) -> TokenSet {
        TokenSet {
            access_token: SecretString::from(access),
            refresh_token: refresh.map(SecretString::from),
            expires_at,
            ..TokenSet::default()
        }
    }

    #[test]
    fn tokens_round_trip_through_the_oauth_response_shape() {
        let original = tokens(
            "at",
            Some("rt"),
            Some(chrono::Utc::now().timestamp() + 3600),
        );
        let response = response_from_tokens(&original).unwrap();
        let back = tokens_from_response(&response).unwrap();
        assert_eq!(back.access_token.expose(), "at");
        assert_eq!(
            back.refresh_token.map(|r| r.expose().to_owned()),
            Some("rt".to_owned())
        );
        assert!(back.expires_at.unwrap() > chrono::Utc::now().timestamp());
    }

    #[test]
    fn a_token_without_refresh_or_expiry_still_round_trips() {
        let response = response_from_tokens(&tokens("at", None, None)).unwrap();
        let back = tokens_from_response(&response).unwrap();
        assert_eq!(back.access_token.expose(), "at");
        assert!(back.refresh_token.is_none());
    }

    #[test]
    fn an_expired_token_reports_no_remaining_lifetime() {
        let past = chrono::Utc::now().timestamp() - 60;
        let response = response_from_tokens(&tokens("at", None, Some(past))).unwrap();
        let raw = serde_json::to_value(&response).unwrap();
        assert_eq!(raw["expires_in"], 0);
    }

    #[tokio::test]
    async fn the_store_adapter_round_trips_through_goat_auth() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = GoatCredentialStore::new(dir.path().join("credentials.json"));
        let key = CredentialKey::integration("sentry", "default");
        credentials
            .store(&key, Credential::OAuth(tokens("first", Some("rt"), None)))
            .unwrap();

        let store = StoredOAuth::new(credentials.clone(), key.clone(), Some("cid".to_owned()));
        let loaded = store.load().await.unwrap().expect("stored credentials");
        assert_eq!(loaded.client_id, "cid");

        let refreshed = StoredCredentials::new(
            "cid".to_owned(),
            Some(response_from_tokens(&tokens("second", Some("rt2"), None)).unwrap()),
            Vec::new(),
            None,
        );
        store.save(refreshed).await.unwrap();

        let Some(Credential::OAuth(saved)) = credentials.get(&key) else {
            panic!("expected an oauth credential");
        };
        assert_eq!(saved.access_token.expose(), "second");
        assert_eq!(
            saved.refresh_token.map(|r| r.expose().to_owned()),
            Some("rt2".to_owned())
        );
    }

    #[tokio::test]
    async fn the_oauth_identity_survives_a_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = GoatCredentialStore::new(dir.path().join("credentials.json"));
        let key = CredentialKey::integration("sentry", "default");
        let store = StoredOAuth::new(credentials.clone(), key.clone(), None);

        store
            .save(
                StoredCredentials::new(
                    "cid".to_owned(),
                    Some(response_from_tokens(&tokens("at", Some("rt"), None)).unwrap()),
                    vec!["read".to_owned(), "write".to_owned()],
                    None,
                )
                .with_issuer(Some("https://as.test".to_owned())),
            )
            .await
            .unwrap();

        let Some(Credential::OAuth(saved)) = credentials.get(&key) else {
            panic!("expected an oauth credential");
        };
        assert_eq!(saved.client_id.as_deref(), Some("cid"));
        assert_eq!(saved.scopes, ["read", "write"]);
        assert_eq!(saved.issuer.as_deref(), Some("https://as.test"));

        let reloaded = store.load().await.unwrap().expect("credentials");
        assert_eq!(reloaded.client_id, "cid");
        assert_eq!(reloaded.granted_scopes, ["read", "write"]);
        assert_eq!(reloaded.issuer.as_deref(), Some("https://as.test"));
    }

    #[tokio::test]
    async fn a_credential_stored_before_this_change_uses_the_config_client_id() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = GoatCredentialStore::new(dir.path().join("credentials.json"));
        let key = CredentialKey::integration("sentry", "default");
        credentials
            .store(&key, Credential::OAuth(tokens("at", None, None)))
            .unwrap();

        let without = StoredOAuth::new(credentials.clone(), key.clone(), None);
        assert!(without.load().await.unwrap().is_none());

        let with = StoredOAuth::new(credentials, key, Some("from-config".to_owned()));
        assert_eq!(
            with.load().await.unwrap().expect("credentials").client_id,
            "from-config"
        );
    }

    #[test]
    fn granted_scopes_are_read_from_the_space_separated_scope_field() {
        let mut raw =
            serde_json::to_value(response_from_tokens(&tokens("at", None, None)).unwrap()).unwrap();
        raw["scope"] = serde_json::json!("read write admin");
        let response: OAuthTokenResponse = serde_json::from_value(raw).unwrap();
        assert_eq!(granted_scopes(&response), ["read", "write", "admin"]);
    }

    #[test]
    fn a_response_without_scopes_grants_nothing() {
        let response = response_from_tokens(&tokens("at", None, None)).unwrap();
        assert!(granted_scopes(&response).is_empty());
    }

    #[tokio::test]
    async fn an_absent_credential_loads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = GoatCredentialStore::new(dir.path().join("credentials.json"));
        let store = StoredOAuth::new(
            credentials,
            CredentialKey::integration("sentry", "default"),
            Some("cid".to_owned()),
        );
        assert!(store.load().await.unwrap().is_none());
    }

    #[test]
    fn the_state_parameter_is_read_back_from_the_authorization_url() {
        assert_eq!(
            state_of("https://as.test/authorize?client_id=x&state=abc123&scope=y").as_deref(),
            Some("abc123")
        );
        assert_eq!(state_of("https://as.test/authorize?client_id=x"), None);
        assert_eq!(state_of("not a url"), None);
    }
}
