use goat_auth::{
    AuthError, Credential, CredentialKey, CredentialStore, Pkce, TokenSet, capture_on,
    ensure_valid, random_state,
};
use serde::Deserialize;
use tokio::sync::mpsc;

const CLIENT_ID: &str = "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com";
const CLIENT_SECRET: &str = "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";
const AUTHORIZE: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN: &str = "https://oauth2.googleapis.com/token";
const SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile";

#[derive(Debug, thiserror::Error)]
pub enum GeminiAuthError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("url error: {0}")]
    Url(String),
    #[error("token error: {0}")]
    Token(String),
    #[error("auth error: {0}")]
    Auth(#[from] AuthError),
}

pub enum Auth {
    ApiKey(String),
    OAuth(String),
}

fn authorize_url(
    challenge: &str,
    state: &str,
    redirect_uri: &str,
) -> Result<String, GeminiAuthError> {
    reqwest::Url::parse_with_params(
        AUTHORIZE,
        &[
            ("client_id", CLIENT_ID),
            ("response_type", "code"),
            ("redirect_uri", redirect_uri),
            ("scope", SCOPE),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            ("state", state),
            ("access_type", "offline"),
            ("prompt", "consent"),
        ],
    )
    .map(|url| url.to_string())
    .map_err(|err| GeminiAuthError::Url(err.to_string()))
}

fn auth_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("reqwest client")
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
}

async fn parse_token_response(
    response: reqwest::Response,
) -> Result<TokenResponse, GeminiAuthError> {
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(GeminiAuthError::Token(format!("{status}: {body}")));
    }
    response.json().await.map_err(GeminiAuthError::Http)
}

async fn exchange_code(
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<TokenSet, GeminiAuthError> {
    let response = auth_client()
        .post(TOKEN)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
            ("code_verifier", verifier),
        ])
        .send()
        .await?;
    parse_token_response(response)
        .await
        .map(|t| TokenSet::from_parts(t.access_token, t.refresh_token, t.expires_in, None))
}

pub async fn do_refresh(refresh_token: String) -> Result<TokenSet, String> {
    let response = auth_client()
        .post(TOKEN)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
        ])
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!("{status}: {body}"));
    }
    response
        .json::<TokenResponse>()
        .await
        .map_err(|e| e.to_string())
        .map(|t| {
            TokenSet::from_parts(
                t.access_token,
                t.refresh_token,
                t.expires_in,
                Some(&refresh_token),
            )
        })
}

pub async fn do_login(status: &mpsc::Sender<String>) -> Result<TokenSet, GeminiAuthError> {
    let pkce = Pkce::generate();
    let state = random_state();
    let (listener, port) = goat_auth::bind_loopback().await?;
    let redirect = format!("http://127.0.0.1:{port}/oauth2callback");
    let url = authorize_url(&pkce.challenge, &state, &redirect)?;
    let _ = status
        .send(format!(
            "opening browser to sign in to Google\u{2026} if it does not open, visit:\n{url}"
        ))
        .await;
    let _ = open::that(&url);
    let code = capture_on(listener, &state).await?;
    exchange_code(&code, &pkce.verifier, &redirect).await
}

pub async fn current_auth(store: &CredentialStore, key: &CredentialKey) -> Option<Auth> {
    match store.resolve(key, Some(super::ENV_VAR))? {
        Credential::ApiKey(secret) | Credential::ApiKeyWithEndpoint { secret, .. } => {
            Some(Auth::ApiKey(secret.expose().to_owned()))
        }
        Credential::OAuth(tokens) => {
            let tokens = ensure_valid(tokens, store, key, do_refresh).await?;
            Some(Auth::OAuth(tokens.access_token.expose().to_owned()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::authorize_url;

    #[test]
    fn authorize_url_contains_required_params() {
        let url = authorize_url("CHAL", "STATE", "http://127.0.0.1:9999/oauth2callback").unwrap();
        assert!(
            url.contains(
                "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com"
            )
        );
        assert!(url.contains("code_challenge=CHAL"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=STATE"));
        assert!(url.contains("access_type=offline"));
        assert!(url.contains("prompt=consent"));
        assert!(url.contains("127.0.0.1"));
        assert!(url.contains("oauth2callback"));
    }
}
