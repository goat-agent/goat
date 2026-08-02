use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use rmcp::service::ServiceError;
use serde::Deserialize;

pub mod auth;
mod handshake;
mod names;
mod result;
mod session;

pub use handshake::START_TIMEOUT;
pub use names::sanitize_component;
pub use result::{McpContent, McpImage, McpToolResult};
pub use rmcp::model::Tool as McpTool;
pub use session::{CALL_TIMEOUT, HttpEndpoint, McpSession, http_client};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum McpError {
    #[error("mcp config io failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("mcp config json failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("mcp server {server} failed to initialize: {message}")]
    Initialize { server: String, message: String },
    #[error("mcp server {server} request failed: {source}")]
    Request {
        server: String,
        #[source]
        source: ServiceError,
    },
    #[error("mcp server {server} rejected the arguments: {message}")]
    InvalidParams { server: String, message: String },
    #[error("mcp tool {tool} returned an error: {message}")]
    ToolError { tool: String, message: String },
    #[error("mcp tool {tool} arguments must be a json object, found {found}")]
    InputNotObject { tool: String, found: String },
    #[error(
        "mcp server {server} asked for more input while running {tool}; goat cannot answer mid-call yet"
    )]
    InputRequired { server: String, tool: String },
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpConfig {
    #[serde(default)]
    pub mcp_servers: HashMap<String, ServerConfig>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

impl McpConfig {
    pub fn load(path: &Path) -> Result<Self, McpError> {
        let raw = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }
}

pub async fn load_manager(path: Option<&Path>, cwd: &Path) -> Arc<McpManager> {
    let Some(path) = path else {
        return Arc::new(McpManager::default());
    };
    if !path.exists() {
        return Arc::new(McpManager::default());
    }
    match McpConfig::load(path) {
        Ok(config) => McpManager::start(config, cwd).await,
        Err(err) => {
            tracing::warn!(%err, path = %path.display(), "failed to load mcp config");
            Arc::new(McpManager::default())
        }
    }
}

pub struct McpServer {
    pub name: String,
    pub session: Arc<McpSession>,
    pub tools: Vec<McpTool>,
}

#[derive(Default)]
pub struct McpManager {
    servers: Vec<McpServer>,
}

impl McpManager {
    pub async fn start(config: McpConfig, cwd: &Path) -> Arc<Self> {
        let mut servers = Vec::new();
        let mut configured: Vec<_> = config.mcp_servers.into_iter().collect();
        configured.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, server_config) in configured {
            match McpSession::connect_stdio(name.clone(), server_config, cwd).await {
                Ok((session, tools)) => servers.push(McpServer {
                    name,
                    session: Arc::new(session),
                    tools,
                }),
                Err(err) => tracing::warn!(%err, server = %name, "skipping mcp server"),
            }
        }
        Arc::new(Self { servers })
    }

    pub fn servers(&self) -> &[McpServer] {
        &self.servers
    }

    pub async fn shutdown(&self) {
        for server in &self.servers {
            server.session.close().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mcp_servers_config() {
        let config: McpConfig = serde_json::from_str(
            r#"{
                "mcpServers": {
                    "filesystem": {
                        "command": "npx",
                        "args": ["-y", "pkg"],
                        "env": {"A": "B"}
                    }
                }
            }"#,
        )
        .unwrap();
        let server = config.mcp_servers.get("filesystem").unwrap();
        assert_eq!(server.command, "npx");
        assert_eq!(server.args, ["-y", "pkg"]);
        assert_eq!(server.env.get("A").unwrap(), "B");
    }

    #[tokio::test]
    async fn a_manager_without_config_has_no_servers() {
        let manager = load_manager(None, Path::new(".")).await;
        assert!(manager.servers().is_empty());
    }

    #[tokio::test]
    async fn a_missing_config_path_is_not_fatal() {
        let manager = load_manager(Some(Path::new("/nonexistent/mcp.json")), Path::new(".")).await;
        assert!(manager.servers().is_empty());
    }
}
