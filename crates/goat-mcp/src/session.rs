use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use rmcp::model::{CallToolRequestParams, ClientRequest, ErrorCode, ServerResult, Tool as McpTool};
use rmcp::service::{PeerRequestOptions, ServiceError};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{ConfigureCommandExt, StreamableHttpClientTransport, TokioChildProcess};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::handshake::{self, Era, Failed};
use crate::result::McpToolResult;
use crate::{McpError, ServerConfig};

pub const CALL_TIMEOUT: Duration = Duration::from_mins(2);
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_TIMEOUT: Duration = Duration::from_mins(5);

pub struct HttpEndpoint {
    pub url: String,
    pub auth_header: Option<String>,
    pub headers: HashMap<String, String>,
}

impl HttpEndpoint {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            auth_header: None,
            headers: HashMap::new(),
        }
    }

    #[must_use]
    pub fn with_auth_header(mut self, value: impl Into<String>) -> Self {
        self.auth_header = Some(value.into());
        self
    }

    #[must_use]
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }
}

pub fn http_client() -> Result<rmcp_reqwest::Client, McpError> {
    ensure_crypto_provider();
    rmcp_reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .map_err(|err| McpError::Initialize {
            server: "http".to_owned(),
            message: err.to_string(),
        })
}

fn ensure_crypto_provider() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

pub struct McpSession {
    server_name: String,
    pid: Option<u32>,
    client: Mutex<handshake::Client>,
}

impl McpSession {
    pub async fn connect_stdio(
        server_name: String,
        config: ServerConfig,
        cwd: &Path,
    ) -> Result<(Self, Vec<McpTool>), McpError> {
        let (transport, mut pid) = spawn_child(&server_name, &config, cwd)?;
        let client = match handshake::open(handshake::PREFERRED, transport).await {
            Ok(client) => client,
            Err(failure) => {
                let Some(era) = retry_era(&server_name, &failure) else {
                    return Err(handshake::into_error(&server_name, &failure));
                };
                if let Some(stale) = pid.take() {
                    kill_process_group(&server_name, stale);
                }
                let (transport, respawned) = spawn_child(&server_name, &config, cwd)?;
                pid = respawned;
                handshake::open(era, transport)
                    .await
                    .map_err(|retried| exhausted(&server_name, &failure, &retried))?
            }
        };
        let session = Self {
            server_name,
            pid,
            client: Mutex::new(client),
        };
        let tools = session.list_tools().await?;
        Ok((session, tools))
    }

    pub async fn connect_http<C>(
        server_name: String,
        endpoint: &HttpEndpoint,
        client: C,
    ) -> Result<Self, McpError>
    where
        C: rmcp::transport::streamable_http_client::StreamableHttpClient + Clone + 'static,
    {
        let mut config = StreamableHttpClientTransportConfig::with_uri(endpoint.url.clone());
        if let Some(auth) = &endpoint.auth_header {
            config = config.auth_header(auth.clone());
        }
        if !endpoint.headers.is_empty() {
            config = config.custom_headers(headers(&server_name, &endpoint.headers));
        }
        let transport =
            || StreamableHttpClientTransport::with_client(client.clone(), config.clone());
        let running = match handshake::open(handshake::PREFERRED, transport()).await {
            Ok(running) => running,
            Err(failure) => {
                let Some(era) = retry_era(&server_name, &failure) else {
                    return Err(handshake::into_error(&server_name, &failure));
                };
                handshake::open(era, transport())
                    .await
                    .map_err(|retried| exhausted(&server_name, &failure, &retried))?
            }
        };
        Ok(Self {
            server_name,
            pid: None,
            client: Mutex::new(running),
        })
    }

    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    pub async fn peer_name(&self) -> Option<String> {
        self.client
            .lock()
            .await
            .peer_info()
            .and_then(|info| info.server_info.clone())
            .map(|server| server.name)
    }

    pub async fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
        let client = self.client.lock().await;
        tokio::time::timeout(CALL_TIMEOUT, client.list_all_tools())
            .await
            .map_err(|_| McpError::Initialize {
                server: self.server_name.clone(),
                message: format!("tools/list timed out after {}s", CALL_TIMEOUT.as_secs()),
            })?
            .map_err(|source| McpError::Request {
                server: self.server_name.clone(),
                source,
            })
    }

    pub async fn call(&self, tool: &str, arguments: Value) -> Result<McpToolResult, McpError> {
        let params = match arguments {
            Value::Object(map) => CallToolRequestParams::new(tool.to_owned()).with_arguments(map),
            Value::Null => CallToolRequestParams::new(tool.to_owned()),
            other => {
                return Err(McpError::InputNotObject {
                    tool: tool.to_owned(),
                    found: kind_of(&other).to_owned(),
                });
            }
        };
        let request = ClientRequest::CallToolRequest(rmcp::model::CallToolRequest::new(params));
        let client = self.client.lock().await;
        let handle = client
            .send_cancellable_request(request, request_options(CALL_TIMEOUT))
            .await
            .map_err(|source| self.request_error(source))?;
        let result = handle
            .await_response()
            .await
            .map_err(|source| self.request_error(source))?;
        match result {
            ServerResult::CallToolResult(result) => Ok(McpToolResult::from(result)),
            ServerResult::InputRequiredResult(_) => Err(McpError::InputRequired {
                server: self.server_name.clone(),
                tool: tool.to_owned(),
            }),
            _ => Err(self.request_error(ServiceError::UnexpectedResponse)),
        }
    }

    pub async fn close(&self) {
        let mut client = self.client.lock().await;
        if let Err(err) = client.close_with_timeout(SHUTDOWN_TIMEOUT).await {
            tracing::warn!(%err, server = %self.server_name, "failed to close mcp session");
        }
    }

    fn request_error(&self, source: ServiceError) -> McpError {
        if let ServiceError::McpError(error) = &source
            && error.code == ErrorCode::INVALID_PARAMS
        {
            return McpError::InvalidParams {
                server: self.server_name.clone(),
                message: error.message.to_string(),
            };
        }
        McpError::Request {
            server: self.server_name.clone(),
            source,
        }
    }
}

impl Drop for McpSession {
    fn drop(&mut self) {
        if let Some(pid) = self.pid {
            kill_process_group(&self.server_name, pid);
        }
    }
}

fn retry_era(server_name: &str, failure: &Failed) -> Option<Era> {
    let era = handshake::other_era(handshake::PREFERRED, failure)?;
    tracing::debug!(
        server = %server_name,
        detail = %handshake::message(failure),
        "retrying the mcp handshake in the other protocol era"
    );
    Some(era)
}

fn exhausted(server_name: &str, first: &Failed, retried: &Failed) -> McpError {
    tracing::debug!(
        server = %server_name,
        detail = %handshake::message(retried),
        "the retried mcp handshake failed too"
    );
    handshake::into_error(server_name, first)
}

fn spawn_child(
    server_name: &str,
    config: &ServerConfig,
    cwd: &Path,
) -> Result<(TokioChildProcess, Option<u32>), McpError> {
    let mut command = Command::new(&config.command);
    command
        .args(&config.args)
        .envs(&config.env)
        .current_dir(cwd);
    let (transport, stderr) = TokioChildProcess::builder(command.configure(|cmd| {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            cmd.process_group(0);
        }
    }))
    .spawn()?;
    let pid = transport.id();
    if let Some(stderr) = stderr {
        spawn_stderr_logger(server_name.to_owned(), stderr);
    }
    Ok((transport, pid))
}

fn kill_process_group(server_name: &str, child: u32) {
    let Ok(pgid) = i32::try_from(child) else {
        return;
    };
    if let Err(err) = goat_process::kill_group(pgid) {
        tracing::warn!(%err, pgid, server = %server_name, "failed to kill mcp process group");
    }
}

fn headers(
    server_name: &str,
    raw: &HashMap<String, String>,
) -> HashMap<rmcp_reqwest::header::HeaderName, rmcp_reqwest::header::HeaderValue> {
    let mut out = HashMap::new();
    for (name, value) in raw {
        let Ok(name) = rmcp_reqwest::header::HeaderName::try_from(name.as_str()) else {
            tracing::warn!(server = %server_name, header = %name, "skipping unusable header name");
            continue;
        };
        let Ok(value) = rmcp_reqwest::header::HeaderValue::try_from(value.as_str()) else {
            tracing::warn!(server = %server_name, header = %name, "skipping unusable header value");
            continue;
        };
        out.insert(name, value);
    }
    out
}

fn request_options(timeout: Duration) -> PeerRequestOptions {
    let mut options = PeerRequestOptions::no_options();
    options.timeout = Some(timeout);
    options
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn spawn_stderr_logger(server_name: String, stderr: tokio::process::ChildStderr) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    tracing::warn!(server = %server_name, stream = "stderr", "{line}");
                }
                Ok(None) => break,
                Err(err) => {
                    tracing::warn!(%err, server = %server_name, "failed to read mcp stderr");
                    break;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unusable_headers_are_dropped_not_fatal() {
        let mut raw = HashMap::new();
        raw.insert("x-good".to_owned(), "1".to_owned());
        raw.insert("bad header".to_owned(), "1".to_owned());
        raw.insert("x-bad-value".to_owned(), "\n".to_owned());
        let out = headers("test", &raw);
        assert_eq!(out.len(), 1);
        assert!(out.contains_key(&rmcp_reqwest::header::HeaderName::try_from("x-good").unwrap()));
    }

    #[test]
    fn http_client_builds_without_a_preinstalled_crypto_provider() {
        assert!(http_client().is_ok());
        assert!(http_client().is_ok());
    }

    #[test]
    fn endpoint_builder_collects_auth_and_headers() {
        let endpoint = HttpEndpoint::new("https://example.test/mcp")
            .with_auth_header("Bearer t")
            .with_header("x-project", "42");
        assert_eq!(endpoint.auth_header.as_deref(), Some("Bearer t"));
        assert_eq!(
            endpoint.headers.get("x-project").map(String::as_str),
            Some("42")
        );
    }
}
