use std::sync::Arc;

use rmcp::transport::DynamicTransportError;
use rmcp::transport::auth::{
    AuthError, AuthorizationManager, AuthorizationRequest, CredentialStore, OAuthState,
    StoredCredentials,
};
use rmcp::transport::streamable_http_client::{AuthRequiredError, InsufficientScopeError};

use super::{Address, Error, Server, Transport, login_key};
use crate::oauth::{self, Listener};
use crate::store::Logins;

const CLIENT_NAME: &str = "goat";

pub struct SignIn {
    server: String,
    state: OAuthState,
    listener: Listener,
    url: String,
}

pub(crate) struct Saved {
    logins: Arc<Logins>,
    key: String,
}

impl SignIn {
    pub async fn start(
        server: &Server,
        account: Option<&str>,
        logins: Arc<Logins>,
    ) -> Result<Self, Error> {
        let Transport::Url { url, client, .. } = &server.transport else {
            return Err(Error::NotRemote(server.name.clone()));
        };
        let shown = Address {
            name: server.name.clone(),
            account: account.map(str::to_owned),
        }
        .to_string();
        let authorize = |source| Error::Authorize {
            server: shown.clone(),
            source: Box::new(source),
        };
        let port = client.as_ref().and_then(|client| client.port).unwrap_or(0);
        let listener = Listener::bind(&[port]).await?;
        let mut request =
            AuthorizationRequest::new(listener.redirect_uri()?).with_client_name(CLIENT_NAME);
        if let Some(client) = client {
            request = request.with_preregistered_client(&client.id);
            if let Some(secret) = &client.secret {
                request = request.with_client_secret(secret);
            }
        }
        let mut manager = AuthorizationManager::new(url.as_str())
            .await
            .map_err(authorize)?;
        manager.set_credential_store(Saved::new(logins, url, account));
        let mut state = OAuthState::Unauthorized(manager);
        state
            .start_authorization(request)
            .await
            .map_err(authorize)?;
        let url = state.get_authorization_url().await.map_err(authorize)?;
        Ok(Self {
            server: shown,
            state,
            listener,
            url,
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub async fn finish(mut self) -> Result<(), Error> {
        let callback = self.listener.callback().await?;
        if let Some(error) = callback.error {
            return Err(oauth::Error::Denied(error).into());
        }
        let code = callback.code.ok_or(oauth::Error::MissingCode)?;
        let csrf = callback.state.unwrap_or_default();
        self.state
            .handle_callback_with_issuer(&code, &csrf, callback.issuer.as_deref())
            .await
            .map_err(|source| Error::Authorize {
                server: self.server,
                source: Box::new(source),
            })
    }
}

impl std::fmt::Debug for SignIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignIn")
            .field("server", &self.server)
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl Saved {
    pub(crate) fn new(logins: Arc<Logins>, url: &str, account: Option<&str>) -> Self {
        Self {
            logins,
            key: login_key(url, account),
        }
    }
}

#[async_trait::async_trait]
impl CredentialStore for Saved {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let Some(text) = self.logins.get(&self.key).map_err(unstored)? else {
            return Ok(None);
        };
        serde_json::from_str(&text).map(Some).map_err(unstored)
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let text = serde_json::to_string(&credentials).map_err(unstored)?;
        self.logins.save(&self.key, &text).map_err(unstored)
    }

    async fn clear(&self) -> Result<(), AuthError> {
        self.logins.remove(&self.key).map(drop).map_err(unstored)
    }
}

pub(crate) fn needs_sign_in(transport: &DynamicTransportError) -> bool {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(transport.error.as_ref());
    while let Some(error) = current {
        let is_auth = error.is::<AuthRequiredError>()
            || error.is::<InsufficientScopeError>()
            || error
                .downcast_ref::<AuthError>()
                .is_some_and(is_sign_in_error);
        if is_auth {
            return true;
        }
        current = error.source();
    }
    false
}

pub(crate) fn is_sign_in_error(error: &AuthError) -> bool {
    matches!(
        error,
        AuthError::AuthorizationRequired | AuthError::TokenRefreshRejected(_)
    )
}

fn unstored(error: impl std::fmt::Display) -> AuthError {
    AuthError::CredentialStoreError(error.to_string())
}
