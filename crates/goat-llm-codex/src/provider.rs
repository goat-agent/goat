use async_trait::async_trait;
use eventsource_stream::Eventsource;
use goat_llm::{
    LlmError, LlmProvider, LlmRequest, LlmStream, ProviderId, RefreshError, Refreshable,
};
use reqwest::{Response, StatusCode};

use crate::body::Body;
use crate::credential::Credential;
use crate::error::{map_error, parse_retry_after};
use crate::stream::translate;

const URL: &str = "https://chatgpt.com/backend-api/codex/responses";

pub struct CodexProvider {
    creds: Refreshable<Credential>,
    http: reqwest::Client,
}

impl CodexProvider {
    pub(crate) fn new(creds: Refreshable<Credential>) -> Self {
        let http = reqwest::Client::builder()
            .user_agent("codex_cli_rs/0.1.0")
            .build()
            .expect("reqwest client");
        Self { creds, http }
    }

    async fn send(&self, cred: &Credential, body: &Body<'_>) -> Result<Response, LlmError> {
        let session_id = uuid::Uuid::now_v7().to_string();
        let mut req = self
            .http
            .post(URL)
            .bearer_auth(&cred.access_token)
            .header("session-id", session_id)
            .header("content-type", "application/json");
        if let Some(acct) = &cred.account_id {
            req = req.header("ChatGPT-Account-ID", acct);
        }
        req.json(body)
            .send()
            .await
            .map_err(|e| LlmError::Transport(describe_reqwest_error(&e)))
    }
}

#[async_trait]
impl LlmProvider for CodexProvider {
    fn id(&self) -> ProviderId {
        crate::ID
    }

    async fn stream(&self, req: LlmRequest) -> Result<LlmStream, LlmError> {
        let cred = self.creds.current().await.map_err(refresh_to_llm)?;
        let body = Body::from(&req);
        let resp = self.send(&cred, &body).await?;

        if resp.status() == StatusCode::UNAUTHORIZED {
            drop(resp);
            let cred = self.creds.force_refresh().await.map_err(refresh_to_llm)?;
            let resp = self.send(&cred, &body).await?;
            if !resp.status().is_success() {
                let status = resp.status();
                let retry_after = parse_retry_after(resp.headers());
                let text = resp.text().await.unwrap_or_default();
                return Err(map_error(status, retry_after, &text));
            }
            return Ok(translate(resp.bytes_stream().eventsource()));
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let retry_after = parse_retry_after(resp.headers());
            let text = resp.text().await.unwrap_or_default();
            return Err(map_error(status, retry_after, &text));
        }

        Ok(translate(resp.bytes_stream().eventsource()))
    }
}

fn describe_reqwest_error(err: &reqwest::Error) -> String {
    use std::error::Error;
    let mut parts: Vec<String> = vec![err.to_string()];
    let mut src: Option<&dyn Error> = err.source();
    while let Some(s) = src {
        parts.push(s.to_string());
        src = s.source();
    }
    let mut flags = Vec::new();
    if err.is_timeout() {
        flags.push("timeout");
    }
    if err.is_connect() {
        flags.push("connect");
    }
    if err.is_request() {
        flags.push("request");
    }
    if err.is_body() {
        flags.push("body");
    }
    if err.is_decode() {
        flags.push("decode");
    }
    if !flags.is_empty() {
        parts.push(format!("flags=[{}]", flags.join(",")));
    }
    parts.join(" | ")
}

fn refresh_to_llm(e: RefreshError) -> LlmError {
    match e {
        RefreshError::NotFound => {
            LlmError::Auth("no codex credential configured; run `goat provider add codex`".into())
        }
        RefreshError::Auth(msg) => LlmError::Auth(format!(
            "codex token refresh rejected ({msg}); re-run `goat provider add codex`"
        )),
        RefreshError::Transport(msg) => LlmError::Transport(msg),
        RefreshError::Io(e) => LlmError::Transport(e.to_string()),
        RefreshError::Json(e) => LlmError::Provider(e.to_string()),
        RefreshError::Other(msg) => LlmError::Provider(msg),
    }
}
