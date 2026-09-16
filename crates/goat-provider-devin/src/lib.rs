mod client;
mod oauth;
mod proto;
mod wire;

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use goat_auth::{CredentialKey, CredentialService, CredentialStore, TokenSet};
use goat_provider::{
    AuthMethod, Capabilities, ChunkStream, Model, Provider, ProviderId, ProviderMetadata, Request,
    StreamChunk, StreamError, ValidateError, Validated,
};
use tokio::{sync::mpsc, task::JoinHandle};

pub use client::DEFAULT_BASE_URL;
pub use oauth::login;

pub const PROVIDER_ID: &str = "devin";
const ENV_VAR: &str = "DEVIN_API_KEY";

const UNARY_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

const CONTEXT_WINDOWS: &[(&str, u32)] = &[
    ("swe-1-7-lightning", 202_752),
    ("swe-1-7", 262_144),
    ("swe-1", 200_000),
    ("claude-opus-4-7", 1_000_000),
    ("claude-opus-4-8", 1_000_000),
    ("claude-fable", 1_000_000),
    ("claude", 200_000),
    ("gpt-5-3-codex", 400_000),
    ("gpt-5-2", 384_000),
    ("gpt-5", 272_000),
    ("gemini", 1_048_576),
    ("deepseek", 1_048_576),
    ("glm", 200_000),
];

const TEXT_ONLY_PREFIXES: &[&str] = &["deepseek", "glm"];

pub fn build(store: &CredentialStore, account: &str) -> DevinProvider {
    DevinProvider::new(store.clone(), CredentialKey::model(PROVIDER_ID, account))
}

pub struct DevinProvider {
    store: CredentialStore,
    key: CredentialKey,
    client: reqwest::Client,
    stream_client: reqwest::Client,
    session_id: String,
}

impl DevinProvider {
    pub fn new(store: CredentialStore, key: CredentialKey) -> Self {
        Self {
            store,
            key,
            client: reqwest::Client::builder()
                .timeout(UNARY_TIMEOUT)
                .connect_timeout(CONNECT_TIMEOUT)
                .build()
                .expect("reqwest client"),
            stream_client: reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .build()
                .expect("reqwest client"),
            session_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    fn session_token(&self) -> Option<String> {
        self.store
            .resolve(&self.key, Some(ENV_VAR))
            .map(|credential| credential.bearer().to_owned())
    }
}

fn candidate_tokens(store: &CredentialStore, key: &CredentialKey) -> Vec<String> {
    let mut tokens = Vec::new();
    if let Some(credential) = store.resolve(key, Some(ENV_VAR)) {
        tokens.push(credential.bearer().to_owned());
    }
    for (stored_key, _) in store.entries() {
        if stored_key.service != CredentialService::Model
            || stored_key.provider != PROVIDER_ID
            || stored_key == *key
        {
            continue;
        }
        if let Some(credential) = store.get(&stored_key) {
            tokens.push(credential.bearer().to_owned());
        }
    }
    tokens
}

async fn run_stream(
    stream_client: reqwest::Client,
    chat_base: String,
    frame: Vec<u8>,
) -> Result<ChunkStream, StreamError> {
    let response = stream_client
        .post(format!("{chat_base}{}", client::GET_CHAT_MESSAGE))
        .header("content-type", "application/connect+proto")
        .header("connect-protocol-version", "1")
        .header("connect-content-encoding", "gzip")
        .header("accept-encoding", "identity")
        .header("connect-accept-encoding", "gzip")
        .header("user-agent", "connect-go/1.18.1 (go1.26.3)")
        .body(frame)
        .send()
        .await
        .map_err(|err| StreamError::transport(err.to_string()))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(client::ConnectError::rpc(status, &body).into_stream_error());
    }
    let mut bytes = response.bytes_stream();
    Ok(Box::pin(async_stream::try_stream! {
        let mut buffer = Vec::new();
        let mut tool_order: Vec<String> = Vec::new();
        let mut tool_calls: HashMap<String, (String, String)> = HashMap::new();
        let mut active_tool_id: Option<String> = None;
        loop {
            let item = match tokio::time::timeout(IDLE_TIMEOUT, bytes.next()).await {
                Ok(item) => item,
                Err(_) => Err(StreamError::transport("stream idle timeout"))?,
            };
            let Some(chunk) = item else { break };
            let chunk = chunk.map_err(|err| StreamError::transport(err.to_string()))?;
            buffer.extend_from_slice(&chunk);
            while let Some(frame) = client::take_frame(&mut buffer)
                .map_err(client::ConnectError::into_stream_error)?
            {
                match frame {
                    client::Frame::End(payload) => {
                        if let Some(err) = client::trailer_error(&payload) {
                            Err(err.into_stream_error())?;
                        }
                    }
                    client::Frame::Message(payload) => {
                        let delta = wire::chat_response(&payload);
                        if delta.redacted {
                            let data = if delta.signature.is_empty() {
                                delta.thinking.clone()
                            } else {
                                delta.signature.clone()
                            };
                            yield StreamChunk::RedactedThinking { data };
                        } else {
                            if !delta.thinking.is_empty() {
                                yield StreamChunk::ThinkingDelta { text: delta.thinking };
                            }
                            if !delta.signature.is_empty() {
                                yield StreamChunk::ThinkingSignature { signature: delta.signature };
                            }
                        }
                        if !delta.text.is_empty() {
                            yield StreamChunk::TextDelta { text: delta.text };
                        }
                        for call in delta.tool_calls {
                            let id = if call.id.is_empty() {
                                match active_tool_id.clone() {
                                    Some(id) => id,
                                    None => continue,
                                }
                            } else {
                                call.id.clone()
                            };
                            let entry = tool_calls
                                .entry(id.clone())
                                .or_insert_with(|| (String::new(), String::new()));
                            if !call.name.is_empty() {
                                entry.0.clone_from(&call.name);
                            }
                            if !call.arguments.is_empty() {
                                let previous = entry.1.clone();
                                entry.1 = if call.arguments.starts_with(&previous) {
                                    call.arguments
                                } else {
                                    format!("{previous}{}", call.arguments)
                                };
                            }
                            if !tool_order.contains(&id) {
                                tool_order.push(id.clone());
                            }
                            active_tool_id = Some(id);
                        }
                        if let Some(usage) = delta.usage {
                            yield StreamChunk::Usage { usage };
                        }
                    }
                }
            }
        }
        for id in tool_order {
            if let Some((name, input)) = tool_calls.remove(&id) {
                yield StreamChunk::ToolCall { id, name, input };
            }
        }
    }))
}

#[async_trait]
impl Provider for DevinProvider {
    fn id(&self) -> ProviderId {
        ProviderId::from(PROVIDER_ID)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            tools: true,
            auth: AuthMethod::ApiKeyOrOAuth,
            images: true,
        }
    }

    fn metadata(&self) -> ProviderMetadata {
        ProviderMetadata {
            env_var: Some(ENV_VAR),
            validation: "network",
            endpoint: Some(client::DEFAULT_BASE_URL),
            oauth: Some("browser (Devin account)"),
            login_endpoint: None,
            setup: &[
                "run `goat provider login devin` for browser sign-in",
                "or set DEVIN_API_KEY to a session token or cog_ API key",
            ],
        }
    }

    fn authenticated(&self) -> bool {
        self.session_token().is_some()
    }

    fn validate(&self) -> JoinHandle<Result<Validated, ValidateError>> {
        let store = self.store.clone();
        let key = self.key.clone();
        let client = self.client.clone();
        let session_id = self.session_id.clone();
        tokio::spawn(async move {
            let token = match store.resolve(&key, Some(ENV_VAR)) {
                Some(credential) => credential.bearer().to_owned(),
                None => return Err(ValidateError::NoCredentials),
            };
            match client::verify_status(&client, client::DEFAULT_BASE_URL, &token, &session_id)
                .await
            {
                Ok(()) => Ok(Validated::Live),
                Err(client::ConnectError::Rpc { ref code, .. })
                    if code == "unauthenticated"
                        || code == "permission_denied"
                        || code == "http_401"
                        || code == "http_403" =>
                {
                    Err(ValidateError::InvalidCredentials)
                }
                Err(err) => Err(ValidateError::unreachable(err.to_string())),
            }
        })
    }

    async fn stream(&self, req: Request) -> Result<ChunkStream, StreamError> {
        let token = self
            .session_token()
            .ok_or_else(|| StreamError::auth("not logged in to devin"))?;
        let auth = client::get_user_jwt(
            &self.client,
            client::DEFAULT_BASE_URL,
            &token,
            &self.session_id,
        )
        .await
        .map_err(client::ConnectError::into_stream_error)?;
        let chat_base = auth
            .base_url
            .unwrap_or_else(|| client::DEFAULT_BASE_URL.to_owned());
        let api_key = client::normalize_session_token(&token);
        let body = wire::chat_request(&req, &api_key, &auth.user_jwt, &self.session_id);
        let frame =
            client::encode_frame(&body, true).map_err(client::ConnectError::into_stream_error)?;
        run_stream(self.stream_client.clone(), chat_base, frame).await
    }

    fn discover(&self, out: mpsc::Sender<Model>) -> JoinHandle<()> {
        let client = self.client.clone();
        let store = self.store.clone();
        let key = self.key.clone();
        let session_id = self.session_id.clone();
        tokio::spawn(async move {
            for token in candidate_tokens(&store, &key) {
                let Ok(models) =
                    client::model_configs(&client, client::DEFAULT_BASE_URL, &token, &session_id)
                        .await
                else {
                    continue;
                };
                for model in models {
                    if out.send(model).await.is_err() {
                        return;
                    }
                }
                return;
            }
        })
    }

    fn context_window(&self, model: &str) -> Option<u32> {
        CONTEXT_WINDOWS
            .iter()
            .find(|(prefix, _)| model.starts_with(prefix))
            .map(|(_, window)| *window)
    }

    fn supports_images(&self, model: &str) -> bool {
        !TEXT_ONLY_PREFIXES
            .iter()
            .any(|prefix| model.starts_with(prefix))
    }

    fn login(&self, status: mpsc::Sender<String>) -> JoinHandle<Result<TokenSet, String>> {
        tokio::spawn(async move { oauth::login(&status).await.map_err(|err| err.to_string()) })
    }
}
