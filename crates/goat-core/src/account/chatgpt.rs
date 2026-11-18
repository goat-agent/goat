use serde::{Deserialize, Serialize};

use super::oauth::{self, Error, Listener, Pkce};

const ISSUER: &str = "https://auth.openai.com";
const AUTHORIZE_URL: &str = "https://auth.openai.com/api/accounts/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/api/accounts/oauth/token";
const RESOURCE: &str = "https://api.openai.com/v1";
const SCOPE: &str = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";
const REGISTRATION_CLIENT: &str = "dynamic_agent_client";
const PORTS: &[u16] = &[1455, 0];
const DEFAULT_LIFETIME_SECS: i64 = 3600;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Token {
    pub client_id: Box<str>,
    pub host_id: Box<str>,
    pub subject: Box<str>,
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
            ("client_id", &*self.client_id),
            ("refresh_token", &*self.refresh_token),
            ("resource", RESOURCE),
        ];
        let issued = oauth::request_token(http, TOKEN_URL, &form)
            .await
            .map_err(oauth::refresh_failure)?;
        Ok(Self {
            id_token: issued
                .id_token
                .unwrap_or_else(|| self.id_token.to_string())
                .into(),
            access_token: issued.access_token.into(),
            refresh_token: issued
                .refresh_token
                .unwrap_or_else(|| self.refresh_token.to_string())
                .into(),
            expires_at: oauth::now() + issued.expires_in.unwrap_or(DEFAULT_LIFETIME_SECS),
            ..self.clone()
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
    nonce: String,
    host_id: Box<str>,
    known_client_id: Option<Box<str>>,
}

impl Authorization {
    pub async fn start(previous: Option<&Token>) -> Result<Self, Error> {
        let listener = Listener::bind(PORTS).await?;
        let redirect_uri = listener.redirect_uri()?;
        let pkce = Pkce::generate();
        let state = oauth::random_token::<16>();
        let nonce = oauth::random_token::<16>();
        let host_id = previous.map_or_else(new_host_id, |token| token.host_id.clone());
        let known_client_id = previous.map(|token| token.client_id.clone());

        let mut params = vec![
            (
                "client_id",
                known_client_id.as_deref().unwrap_or(REGISTRATION_CLIENT),
            ),
            ("ext_agent_host_id", &*host_id),
            ("response_type", "code"),
            ("redirect_uri", &redirect_uri),
            ("scope", SCOPE),
            ("resource", RESOURCE),
            ("state", &state),
            ("nonce", &nonce),
            ("code_challenge_method", "S256"),
            ("code_challenge", &pkce.challenge),
        ];
        match previous {
            Some(token) => params.push(("id_token_hint", &token.id_token)),
            None => params.push(("agent_name_hint", "goat")),
        }
        let url = oauth::authorize_url(AUTHORIZE_URL, &params);

        Ok(Self {
            listener,
            url,
            redirect_uri,
            pkce,
            state,
            nonce,
            host_id,
            known_client_id,
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub async fn finish(self, http: &reqwest::Client) -> Result<Token, Error> {
        let (code, issued_client_id) = self.listener.callback().await?.into_code(&self.state)?;
        let client_id = match (issued_client_id, self.known_client_id) {
            (Some(issued), Some(known)) if *issued != *known => return Err(Error::ClientIdChanged),
            (Some(issued), _) => issued.into_boxed_str(),
            (None, Some(known)) => known,
            (None, None) => return Err(Error::MissingClientId),
        };

        let form = [
            ("grant_type", "authorization_code"),
            ("client_id", &*client_id),
            ("code", &code),
            ("code_verifier", &self.pkce.verifier),
            ("redirect_uri", &self.redirect_uri),
            ("resource", RESOURCE),
        ];
        let issued = oauth::request_token(http, TOKEN_URL, &form).await?;
        if !issued
            .scope
            .as_deref()
            .is_some_and(|scope| scope.split(' ').any(|s| s == PLAN_SCOPE))
        {
            return Err(Error::PlanNotGranted);
        }
        let id_token = issued.id_token.ok_or(Error::InvalidToken("no id token"))?;
        let claims = verify_id_token(&id_token, &client_id, &self.nonce)?;
        let refresh_token = issued
            .refresh_token
            .ok_or(Error::InvalidToken("no refresh token"))?;

        Ok(Token {
            client_id,
            host_id: self.host_id,
            subject: claims.sub.into(),
            email: claims.email.map(String::into_boxed_str),
            id_token: id_token.into(),
            access_token: issued.access_token.into(),
            refresh_token: refresh_token.into(),
            expires_at: oauth::now() + issued.expires_in.unwrap_or(DEFAULT_LIFETIME_SECS),
        })
    }
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    aud: Audience,
    exp: i64,
    nonce: Option<String>,
    sub: String,
    email: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

impl Audience {
    fn contains(&self, client_id: &str) -> bool {
        match self {
            Self::One(aud) => aud == client_id,
            Self::Many(auds) => auds.iter().any(|aud| aud == client_id),
        }
    }
}

// OIDC Core 3.1.3.7 allows TLS from the token endpoint to replace signature checks: https://openid.net/specs/openid-connect-core-1_0.html#IDTokenValidation
fn verify_id_token(id_token: &str, client_id: &str, nonce: &str) -> Result<Claims, Error> {
    let claims: Claims = oauth::jwt_claims(id_token)?;
    if claims.iss != ISSUER {
        return Err(Error::InvalidToken("wrong issuer"));
    }
    if !claims.aud.contains(client_id) {
        return Err(Error::InvalidToken("wrong audience"));
    }
    if claims.exp <= oauth::now() {
        return Err(Error::InvalidToken("expired"));
    }
    if claims.nonce.as_deref() != Some(nonce) {
        return Err(Error::InvalidToken("wrong nonce"));
    }
    Ok(claims)
}

fn new_host_id() -> Box<str> {
    let uuid = uuid::Builder::from_random_bytes(oauth::random_bytes::<16>()).into_uuid();
    format!("urn:uuid:{uuid}").into()
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    use super::*;

    fn id_token(claims: &str) -> String {
        format!("e30.{}.sig", URL_SAFE_NO_PAD.encode(claims))
    }

    #[test]
    fn valid_id_token_is_accepted() {
        let exp = oauth::now() + 600;
        let token = id_token(&format!(
            r#"{{"iss":"https://auth.openai.com","aud":"oaiapp_1","exp":{exp},"nonce":"n","sub":"user-1","email":"a@b.c"}}"#
        ));
        let claims = verify_id_token(&token, "oaiapp_1", "n").unwrap();
        assert_eq!(claims.sub, "user-1");
    }

    #[test]
    fn id_token_with_wrong_nonce_is_rejected() {
        let exp = oauth::now() + 600;
        let token = id_token(&format!(
            r#"{{"iss":"https://auth.openai.com","aud":["oaiapp_1"],"exp":{exp},"nonce":"other","sub":"u"}}"#
        ));
        assert!(matches!(
            verify_id_token(&token, "oaiapp_1", "n"),
            Err(Error::InvalidToken("wrong nonce"))
        ));
    }

    #[test]
    fn host_id_is_a_uuid_urn() {
        assert!(new_host_id().starts_with("urn:uuid:"));
    }
}
