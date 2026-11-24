use serde::{Deserialize, Serialize};

use crate::oauth::{self, Error, Listener, Pkce};

const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const SCOPE: &str = "openid profile email offline_access";
const PORTS: &[u16] = &[1455, 1457];
pub(crate) const ORIGINATOR: &str = "goat";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Token {
    pub account_id: Box<str>,
    pub plan: Option<Box<str>>,
    pub email: Option<Box<str>>,
    pub id_token: Box<str>,
    pub access_token: Box<str>,
    pub refresh_token: Box<str>,
    pub expires_at: i64,
}

impl Token {
    pub async fn refresh(&self, http: &reqwest::Client) -> Result<Self, Error> {
        let form = [
            ("grant_type", "refresh_token"),
            ("client_id", CLIENT_ID),
            ("refresh_token", &*self.refresh_token),
        ];
        let issued = oauth::request_token(http, TOKEN_URL, &form)
            .await
            .map_err(oauth::refresh_failure)?;
        let id_token = issued.id_token.unwrap_or_else(|| self.id_token.to_string());
        let refresh_token = issued
            .refresh_token
            .unwrap_or_else(|| self.refresh_token.to_string());
        Self::from_tokens(id_token, issued.access_token, refresh_token)
    }

    fn from_tokens(
        id_token: String,
        access_token: String,
        refresh_token: String,
    ) -> Result<Self, Error> {
        let identity: IdClaims = oauth::jwt_claims(&id_token)?;
        let access: AccessClaims = oauth::jwt_claims(&access_token)?;
        Ok(Self {
            account_id: identity
                .auth
                .chatgpt_account_id
                .ok_or(Error::InvalidToken("no ChatGPT account id"))?
                .into(),
            plan: identity.auth.chatgpt_plan_type.map(String::into_boxed_str),
            email: identity.email.map(String::into_boxed_str),
            id_token: id_token.into(),
            access_token: access_token.into(),
            refresh_token: refresh_token.into(),
            expires_at: access.exp,
        })
    }
}

#[derive(Debug)]
pub struct Authorization {
    listener: Listener,
    url: String,
    redirect_uri: String,
    pkce: Pkce,
    state: String,
}

impl Authorization {
    pub async fn start() -> Result<Self, Error> {
        let listener = Listener::bind(PORTS).await?;
        let redirect_uri = listener.redirect_uri()?;
        let pkce = Pkce::generate();
        let state = oauth::random_token::<32>();
        let url = oauth::authorize_url(
            AUTHORIZE_URL,
            &[
                ("response_type", "code"),
                ("client_id", CLIENT_ID),
                ("redirect_uri", &redirect_uri),
                ("scope", SCOPE),
                ("code_challenge", &pkce.challenge),
                ("code_challenge_method", "S256"),
                ("state", &state),
                ("id_token_add_organizations", "true"),
                ("codex_cli_simplified_flow", "true"),
                ("originator", ORIGINATOR),
            ],
        );
        Ok(Self {
            listener,
            url,
            redirect_uri,
            pkce,
            state,
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub async fn finish(self, http: &reqwest::Client) -> Result<Token, Error> {
        let (code, _) = self.listener.callback().await?.into_code(&self.state)?;
        let form = [
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("code", &code),
            ("redirect_uri", &self.redirect_uri),
            ("code_verifier", &self.pkce.verifier),
        ];
        let issued = oauth::request_token(http, TOKEN_URL, &form).await?;
        Token::from_tokens(
            issued.id_token.ok_or(Error::InvalidToken("no id token"))?,
            issued.access_token,
            issued
                .refresh_token
                .ok_or(Error::InvalidToken("no refresh token"))?,
        )
    }
}

#[derive(Deserialize)]
struct IdClaims {
    email: Option<String>,
    #[serde(rename = "https://api.openai.com/auth", default)]
    auth: AuthClaims,
}

#[derive(Deserialize, Default)]
struct AuthClaims {
    chatgpt_account_id: Option<String>,
    chatgpt_plan_type: Option<String>,
}

#[derive(Deserialize)]
struct AccessClaims {
    exp: i64,
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    use super::*;

    fn jwt(claims: &str) -> String {
        format!("e30.{}.sig", URL_SAFE_NO_PAD.encode(claims))
    }

    #[test]
    fn token_reads_account_plan_and_expiry() {
        let id = jwt(
            r#"{"email":"a@b.c","https://api.openai.com/auth":{"chatgpt_account_id":"acc-1","chatgpt_plan_type":"pro"}}"#,
        );
        let access = jwt(r#"{"exp":2000000000}"#);
        let token = Token::from_tokens(id, access, "refresh".to_owned()).unwrap();
        assert_eq!(&*token.account_id, "acc-1");
        assert_eq!(token.plan.as_deref(), Some("pro"));
        assert_eq!(token.expires_at, 2_000_000_000);
    }

    #[test]
    fn token_without_account_id_is_rejected() {
        let id = jwt(r#"{"email":"a@b.c"}"#);
        let access = jwt(r#"{"exp":2000000000}"#);
        assert!(matches!(
            Token::from_tokens(id, access, "refresh".to_owned()),
            Err(Error::InvalidToken("no ChatGPT account id"))
        ));
    }
}
