use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::mcp::{Context, Error};
use crate::oauth::{self, Listener, Pkce};
use crate::provider::Endpoints;
use crate::store::Logins;

const CLIENT_ID: &str = "37164328822-ok9stl3msms3abtulohl6tm18rf0g0db.apps.googleusercontent.com";
const CLIENT_SECRET: &str = "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";
const REFRESH_MARGIN_SECS: i64 = 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Grant {
    access_token: String,
    refresh_token: String,
    expires_at: i64,
    client_id: String,
}

#[derive(Debug)]
pub(crate) struct Pending {
    listener: Listener,
    pkce: Pkce,
    state: String,
    redirect_uri: String,
    url: String,
}

#[derive(Debug)]
pub(crate) struct Access {
    http: reqwest::Client,
    logins: Arc<Logins>,
    key: String,
    token_url: Box<str>,
}

impl Pending {
    pub(crate) async fn start(
        scopes: &[&str],
        endpoints: &Endpoints,
    ) -> Result<Self, oauth::Error> {
        let listener = Listener::bind(&[0]).await?;
        let redirect_uri = listener.redirect_uri()?;
        let pkce = Pkce::generate();
        let state = oauth::random_token::<32>();
        let scope = scopes.join(" ");
        let url = oauth::authorize_url(
            &endpoints.google_sign_in,
            &[
                ("response_type", "code"),
                ("client_id", CLIENT_ID),
                ("redirect_uri", &redirect_uri),
                ("scope", &scope),
                ("state", &state),
                ("code_challenge", &pkce.challenge),
                ("code_challenge_method", "S256"),
                ("access_type", "offline"),
                ("prompt", "consent select_account"),
            ],
        );
        Ok(Self {
            listener,
            pkce,
            state,
            redirect_uri,
            url,
        })
    }

    pub(crate) fn url(&self) -> &str {
        &self.url
    }

    pub(crate) async fn finish(self, context: &Context, key: &str) -> Result<(), Error> {
        let (http, logins, endpoints) = (&context.http, &context.logins, &context.endpoints);
        let callback = self.listener.callback().await?;
        let (code, _) = callback.into_code(&self.state)?;
        let issued = oauth::request_token(
            http,
            &endpoints.google_token,
            &[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", &self.redirect_uri),
                ("client_id", CLIENT_ID),
                ("client_secret", CLIENT_SECRET),
                ("code_verifier", &self.pkce.verifier),
            ],
        )
        .await?;
        let refresh_token = issued.refresh_token.ok_or(oauth::Error::InvalidToken(
            "Google returned no refresh token",
        ))?;
        let token = Grant {
            access_token: issued.access_token,
            refresh_token,
            expires_at: oauth::now() + issued.expires_in.unwrap_or(3600),
            client_id: CLIENT_ID.to_owned(),
        };
        save(logins, key, &token)
    }
}

impl Access {
    pub(crate) fn new(context: &Context, key: String) -> Self {
        Self {
            http: context.http.clone(),
            logins: Arc::clone(&context.logins),
            key,
            token_url: context.endpoints.google_token.clone(),
        }
    }

    pub(crate) async fn bearer(&self) -> Result<String, String> {
        let Some(text) = self.logins.get(&self.key).map_err(|err| err.to_string())? else {
            return Err(SIGN_IN.to_owned());
        };
        let mut token: Grant = serde_json::from_str(&text).map_err(|_| SIGN_IN.to_owned())?;
        if token.client_id != CLIENT_ID {
            return Err(SIGN_IN.to_owned());
        }
        if token.expires_at - REFRESH_MARGIN_SECS > oauth::now() {
            return Ok(token.access_token);
        }
        let issued = oauth::request_token(
            &self.http,
            &self.token_url,
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", &token.refresh_token),
                ("client_id", CLIENT_ID),
                ("client_secret", CLIENT_SECRET),
            ],
        )
        .await
        .map_err(|err| match oauth::refresh_failure(err) {
            oauth::Error::Reauthenticate(_) => SIGN_IN.to_owned(),
            other => format!("could not refresh the Google sign-in: {other}"),
        })?;
        token.access_token = issued.access_token;
        token.expires_at = oauth::now() + issued.expires_in.unwrap_or(3600);
        if let Some(refresh_token) = issued.refresh_token {
            token.refresh_token = refresh_token;
        }
        save(&self.logins, &self.key, &token).map_err(|err| err.to_string())?;
        Ok(token.access_token)
    }
}

pub(crate) const SIGN_IN: &str =
    "the Google sign-in is missing or expired; ask the user to sign in again";

fn save(logins: &Logins, key: &str, token: &Grant) -> Result<(), Error> {
    let text = serde_json::to_string(token).map_err(oauth::Error::from)?;
    logins.save(key, &text)?;
    Ok(())
}
