mod codeassist;
mod error;
mod oauth;
mod wire;

use std::sync::Arc;

use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use goat_auth::{CredentialKey, CredentialStore, TokenSet};
use goat_provider::{
    AuthMethod, Capabilities, ChunkStream, Effort, Model, Provider, ProviderId, ProviderMetadata,
    Request, SearchResult, StreamChunk, StreamError, ValidateError, Validated, WebSearchOutput,
};
use serde_json::json;
use tokio::{sync::Mutex, sync::mpsc, task::JoinHandle};

pub const PROVIDER_ID: &str = "gemini";
pub const ENV_VAR: &str = "GEMINI_API_KEY";
const GL_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";
const SEARCH_MODEL: &str = "gemini-3.6-flash";

fn gemini_supports_images(model: &str) -> bool {
    let id = model.to_ascii_lowercase();
    id.starts_with("gemini-") && !id.contains("embedding")
}

const CATALOG: &[&str] = &[
    "gemini-3.6-flash",
    "gemini-3.5-flash",
    "gemini-3.5-flash-lite",
    "gemini-3.1-pro-preview",
    "gemini-3.1-flash-lite",
    "gemini-2.5-pro",
    "gemini-2.5-flash",
    "gemini-2.5-flash-lite",
];

const CONTEXT_WINDOWS: &[(&str, u32)] = &[("gemini-3", 1_048_576), ("gemini-2.5", 1_048_576)];

fn gemini_context_window(model: &str) -> Option<u32> {
    CONTEXT_WINDOWS
        .iter()
        .find_map(|(prefix, window)| model.starts_with(prefix).then_some(*window))
}

pub struct GeminiProvider {
    store: CredentialStore,
    key: CredentialKey,
    client: reqwest::Client,
    project: Arc<Mutex<Option<String>>>,
}

impl GeminiProvider {
    fn new(store: CredentialStore, key: CredentialKey) -> Self {
        Self {
            store,
            key,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_mins(5))
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("reqwest client"),
            project: Arc::new(Mutex::new(None)),
        }
    }
}

pub fn build(store: &CredentialStore, account: &str) -> GeminiProvider {
    let key = CredentialKey::model(PROVIDER_ID, account);
    GeminiProvider::new(store.clone(), key)
}

#[derive(serde::Deserialize)]
struct GlModelsResponse {
    #[serde(default)]
    models: Vec<GlModel>,
}

#[derive(serde::Deserialize)]
struct GlModel {
    name: String,
    #[serde(default, rename = "supportedGenerationMethods")]
    supported_generation_methods: Vec<String>,
}

async fn fetch_gl_models(client: &reqwest::Client, api_key: &str) -> Vec<Model> {
    let url = format!("{GL_BASE}/models");
    let Ok(resp) = client
        .get(&url)
        .header("x-goog-api-key", api_key)
        .send()
        .await
    else {
        return Vec::new();
    };
    if !resp.status().is_success() {
        return Vec::new();
    }
    let Ok(body) = resp.json::<GlModelsResponse>().await else {
        return Vec::new();
    };
    body.models
        .into_iter()
        .filter(|m| {
            m.supported_generation_methods
                .iter()
                .any(|method| method == "generateContent")
        })
        .map(|m| {
            let id = m.name.strip_prefix("models/").unwrap_or(&m.name).to_owned();
            Model {
                supports_images: gemini_supports_images(&id),
                id,
            }
        })
        .collect()
}

fn stream_response(resp: reqwest::Response, oauth: bool) -> ChunkStream {
    Box::pin(async_stream::try_stream! {
        let mut stream = resp.bytes_stream().eventsource();
        let mut last_usage: Option<goat_provider::Usage> = None;
        while let Some(event) = stream.next().await {
            match event {
                Ok(event) => {
                    if event.data == "[DONE]" {
                        break;
                    }
                    let Ok(value) = serde_json::from_str::<serde_json::Value>(&event.data) else {
                        continue;
                    };
                    if let Some(error) = error::stream_error(&event.data) {
                        Err(error)?;
                    }
                    for ev in wire::parse_chunk(&value, oauth) {
                        yield ev;
                    }
                    if let Some(usage) = wire::parse_usage(&value, oauth) {
                        last_usage = Some(usage);
                    }
                    match wire::extract_finish_reason(&value, oauth) {
                        None | Some("") => {}
                        Some("STOP" | "MAX_TOKENS") => break,
                        Some(reason) => {
                            if let Some(usage) = last_usage.take() {
                                yield StreamChunk::Usage { usage };
                            }
                            Err(goat_provider::StreamError::other(format!(
                                "generation stopped: {reason}"
                            )))?;
                        }
                    }
                }
                Err(err) => {
                    Err(goat_provider::StreamError::transport(err.to_string()))?;
                }
            }
        }
        if let Some(usage) = last_usage.take() {
            yield StreamChunk::Usage { usage };
        }
    })
}

fn parse_grounding_results(value: &serde_json::Value) -> Vec<SearchResult> {
    let root = value.get("response").unwrap_or(value);
    let mut out = Vec::new();
    let Some(candidate) = root
        .get("candidates")
        .and_then(|candidates| candidates.as_array())
        .and_then(|candidates| candidates.first())
    else {
        return out;
    };
    let Some(chunks) = candidate
        .get("groundingMetadata")
        .and_then(|meta| meta.get("groundingChunks"))
        .and_then(|chunks| chunks.as_array())
    else {
        return out;
    };
    for chunk in chunks {
        let Some(web) = chunk.get("web") else {
            continue;
        };
        let url = web
            .get("uri")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        if url.is_empty() {
            continue;
        }
        out.push(SearchResult {
            title: web
                .get("title")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_owned(),
            url: url.to_owned(),
            snippet: String::new(),
        });
    }
    out
}

#[async_trait]
impl Provider for GeminiProvider {
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
            endpoint: None,
            oauth: Some("browser"),
            login_endpoint: None,
            setup: &[],
        }
    }

    fn supports_images(&self, model: &str) -> bool {
        gemini_supports_images(model)
    }

    fn context_window(&self, model: &str) -> Option<u32> {
        gemini_context_window(model)
    }

    fn authenticated(&self) -> bool {
        self.store.resolve(&self.key, Some(ENV_VAR)).is_some()
    }

    fn catalog(&self) -> &'static [&'static str] {
        CATALOG
    }

    fn efforts(&self, model: &str) -> Vec<Effort> {
        wire::gemini_efforts(model)
    }

    fn validate(&self) -> JoinHandle<Result<Validated, ValidateError>> {
        let store = self.store.clone();
        let key = self.key.clone();
        let client = self.client.clone();
        tokio::spawn(async move {
            let auth = oauth::current_auth(&store, &key)
                .await
                .ok_or(ValidateError::NoCredentials)?;
            let api_key = match auth {
                oauth::Auth::OAuth(_) => return Ok(Validated::Assumed),
                oauth::Auth::ApiKey(k) => k,
            };
            let url = format!("{GL_BASE}/models");
            let resp = client
                .get(&url)
                .header("x-goog-api-key", &api_key)
                .send()
                .await
                .map_err(|_| ValidateError::unreachable("could not reach Gemini API"))?;
            let status = resp.status();
            if status.is_success() {
                Ok(Validated::Live)
            } else if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN
            {
                Err(ValidateError::InvalidCredentials)
            } else {
                Err(ValidateError::unreachable(format!(
                    "could not reach Gemini API: {status}"
                )))
            }
        })
    }

    fn discover(&self, out: mpsc::Sender<Model>) -> JoinHandle<()> {
        let client = self.client.clone();
        let store = self.store.clone();
        let key = self.key.clone();
        tokio::spawn(async move {
            let Some(auth) = oauth::current_auth(&store, &key).await else {
                return;
            };
            match auth {
                oauth::Auth::OAuth(_) => {
                    for &id in CATALOG {
                        if out
                            .send(Model {
                                id: id.to_owned(),
                                supports_images: gemini_supports_images(id),
                            })
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                }
                oauth::Auth::ApiKey(api_key) => {
                    for model in fetch_gl_models(&client, &api_key).await {
                        if out.send(model).await.is_err() {
                            return;
                        }
                    }
                }
            }
        })
    }

    fn supports_web_search(&self) -> bool {
        true
    }

    fn web_search(&self, query: String) -> JoinHandle<Result<WebSearchOutput, StreamError>> {
        let client = self.client.clone();
        let store = self.store.clone();
        let key = self.key.clone();
        let project_cache = Arc::clone(&self.project);
        tokio::spawn(async move {
            let Some(auth) = oauth::current_auth(&store, &key).await else {
                return Err(StreamError::auth("not logged in to gemini"));
            };
            let inner = json!({
                "contents": [{ "role": "user", "parts": [{ "text": query }] }],
                "tools": [{ "google_search": {} }],
            });
            let builder = match &auth {
                oauth::Auth::ApiKey(api_key) => {
                    let url = format!("{GL_BASE}/models/{SEARCH_MODEL}:generateContent");
                    client
                        .post(&url)
                        .header("x-goog-api-key", api_key)
                        .json(&inner)
                }
                oauth::Auth::OAuth(access) => {
                    let project =
                        codeassist::resolve_project(&client, access, &project_cache).await?;
                    let body = codeassist::wrap_request(SEARCH_MODEL, project.as_deref(), inner);
                    let url = format!("{}:generateContent", codeassist::CA_BASE);
                    client.post(&url).bearer_auth(access).json(&body)
                }
            };
            let resp = builder
                .send()
                .await
                .map_err(|err| StreamError::transport(err.to_string()))?;
            if !resp.status().is_success() {
                let status = resp.status();
                let detail = resp.text().await.unwrap_or_default();
                return Err(error::classify_http(status, &detail));
            }
            let value: serde_json::Value = resp
                .json()
                .await
                .map_err(|err| StreamError::other(format!("invalid search response: {err}")))?;
            Ok(WebSearchOutput::from_results(parse_grounding_results(
                &value,
            )))
        })
    }

    async fn stream(&self, req: Request) -> Result<ChunkStream, StreamError> {
        let client = self.client.clone();
        let store = self.store.clone();
        let key = self.key.clone();
        let project_cache = Arc::clone(&self.project);
        let Some(auth) = oauth::current_auth(&store, &key).await else {
            return Err(goat_provider::StreamError::auth("not logged in to gemini"));
        };

        let inner = wire::build_request(&req);
        let inner_value = wire::inner_request_to_value(inner);

        tracing::debug!(model = %req.model, body = %inner_value, "gemini request");

        let (builder, oauth) = match &auth {
            oauth::Auth::ApiKey(api_key) => {
                let url = format!(
                    "{GL_BASE}/models/{}:streamGenerateContent?alt=sse",
                    req.model
                );
                tracing::debug!(%url, "gemini api-key stream");
                let b = client
                    .post(&url)
                    .header("x-goog-api-key", api_key)
                    .json(&inner_value);
                (b, false)
            }
            oauth::Auth::OAuth(access) => {
                let project = codeassist::resolve_project(&client, access, &project_cache).await?;
                let body = codeassist::wrap_request(&req.model, project.as_deref(), inner_value);
                let url = format!("{}:streamGenerateContent?alt=sse", codeassist::CA_BASE);
                let b = client.post(&url).bearer_auth(access).json(&body);
                (b, true)
            }
        };

        let resp = builder
            .send()
            .await
            .map_err(|err| goat_provider::StreamError::transport(err.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let detail = resp.text().await.unwrap_or_default();
            return Err(error::classify_http(status, &detail));
        }

        Ok(stream_response(resp, oauth))
    }

    fn login(&self, status: mpsc::Sender<String>) -> JoinHandle<Result<TokenSet, String>> {
        tokio::spawn(async move { oauth::do_login(&status).await.map_err(|e| e.to_string()) })
    }
}

#[cfg(test)]
mod search_tests {
    use super::parse_grounding_results;

    #[test]
    fn extracts_grounding_chunks() {
        let value = serde_json::json!({
            "candidates": [{
                "groundingMetadata": {
                    "groundingChunks": [
                        { "web": { "uri": "https://a.example", "title": "A" } },
                        { "web": { "uri": "https://b.example", "title": "B" } }
                    ]
                }
            }]
        });
        let results = parse_grounding_results(&value);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, "https://a.example");
        assert_eq!(results[1].title, "B");
    }

    #[test]
    fn unwraps_codeassist_envelope() {
        let value = serde_json::json!({
            "response": {
                "candidates": [{
                    "groundingMetadata": {
                        "groundingChunks": [{ "web": { "uri": "https://x.example", "title": "X" } }]
                    }
                }]
            }
        });
        let results = parse_grounding_results(&value);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://x.example");
    }
}
