use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use goat_auth::{Credential, CredentialKey, CredentialStore};
use rmcp::service::ServiceError;
use serde::{Deserialize, Serialize};

mod approval;
pub mod auth;
mod config;
mod handshake;
mod import;
mod names;
mod result;
mod session;

pub use approval::Approvals;
pub use config::{
    ConfigFile, McpConfig, Scope, project_config_path, project_identity, validate_server_name,
};
pub use handshake::START_TIMEOUT;
pub use import::{ImportCandidate, ImportFormat, ImportSet, parse_import};
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
    #[error("mcp config failed: {0}")]
    Config(String),
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ValueSource {
    Literal(String),
    Env { env: String },
    Secret { secret: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ServerConfig {
    Stdio(StdioConfig),
    Http(HttpConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StdioConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<ValueSource>,
    #[serde(default)]
    pub env: BTreeMap<String, ValueSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpConfig {
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, ValueSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer_token_env_var: Option<String>,
}

pub async fn load_manager(path: Option<&Path>, cwd: &Path) -> Arc<McpManager> {
    let Some(path) = path else {
        return Arc::new(McpManager::default());
    };
    if !path.exists() {
        return Arc::new(McpManager::default());
    }
    match ConfigFile::open(path.to_path_buf()) {
        Ok(file) => McpManager::start(file.config, cwd).await,
        Err(err) => {
            tracing::warn!(%err, path = %path.display(), "failed to load mcp config");
            Arc::new(McpManager::default())
        }
    }
}

pub async fn load_scoped_manager(
    user_path: Option<&Path>,
    approvals_path: Option<&Path>,
    project_root: &Path,
    credentials: &CredentialStore,
    cwd: &Path,
) -> Arc<McpManager> {
    let mut configured = BTreeMap::new();
    let mut failed = Vec::new();
    collect_user_servers(user_path, &mut configured, &mut failed);

    let project_path = project_config_path(project_root);
    if project_path.exists() {
        match ConfigFile::open(project_path) {
            Ok(file) => {
                let approvals = approvals_path
                    .map(|path| Approvals::load(path.to_path_buf()))
                    .transpose();
                match approvals {
                    Ok(approvals) => {
                        for (name, config) in file.config.servers {
                            configured.remove(&name);
                            if approvals
                                .as_ref()
                                .is_some_and(|store| store.approved(project_root, &name, &config))
                            {
                                configured.insert(
                                    name,
                                    LocatedServer {
                                        config,
                                        scope: Scope::Project,
                                    },
                                );
                            } else {
                                failed.push(format!("{name}: pending approval"));
                            }
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, "failed to load project mcp approvals");
                        failed.push("project approvals unavailable".to_owned());
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, "failed to load project mcp config");
                failed.push("project config unavailable".to_owned());
            }
        }
    }

    McpManager::start_located(configured, credentials, project_root, cwd, failed).await
}

pub async fn load_user_manager(
    user_path: Option<&Path>,
    credentials: &CredentialStore,
    root: &Path,
) -> Arc<McpManager> {
    let mut configured = BTreeMap::new();
    let mut failed = Vec::new();
    collect_user_servers(user_path, &mut configured, &mut failed);
    McpManager::start_located(configured, credentials, root, root, failed).await
}

fn collect_user_servers(
    user_path: Option<&Path>,
    configured: &mut BTreeMap<String, LocatedServer>,
    failed: &mut Vec<String>,
) {
    let Some(path) = user_path else {
        return;
    };
    match ConfigFile::open(path.to_path_buf()) {
        Ok(file) => {
            for (name, config) in file.config.servers {
                configured.insert(
                    name,
                    LocatedServer {
                        config,
                        scope: Scope::User,
                    },
                );
            }
        }
        Err(error) => {
            tracing::warn!(%error, "failed to load user mcp config");
            failed.push("user config unavailable".to_owned());
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
    failed: Vec<String>,
}

impl McpManager {
    pub async fn start(config: McpConfig, cwd: &Path) -> Arc<Self> {
        let configured = config
            .servers
            .into_iter()
            .map(|(name, config)| {
                (
                    name,
                    LocatedServer {
                        config,
                        scope: Scope::User,
                    },
                )
            })
            .collect();
        let credentials = CredentialStore::new(cwd.join(".goat-mcp-unused-credentials.json"));
        Self::start_located(configured, &credentials, Path::new("."), cwd, Vec::new()).await
    }

    async fn start_located(
        configured: BTreeMap<String, LocatedServer>,
        credentials: &CredentialStore,
        project_root: &Path,
        cwd: &Path,
        mut failed: Vec<String>,
    ) -> Arc<Self> {
        let mut servers = Vec::new();
        for (name, located) in configured {
            let account = located.scope.account(project_root);
            match connect_server(&name, &account, located.config, credentials, cwd).await {
                Ok((session, tools)) => servers.push(McpServer {
                    name,
                    session: Arc::new(session),
                    tools,
                }),
                Err(error) => {
                    tracing::warn!(%error, server = %name, "skipping mcp server");
                    failed.push(format!("{name}: unavailable"));
                }
            }
        }
        Arc::new(Self { servers, failed })
    }

    pub fn servers(&self) -> &[McpServer] {
        &self.servers
    }

    pub fn startup_message(&self) -> Option<String> {
        if self.servers.is_empty() && self.failed.is_empty() {
            return None;
        }
        let connected = self
            .servers
            .iter()
            .map(|server| server.name.as_str())
            .collect::<Vec<_>>();
        let mut parts = Vec::new();
        if !connected.is_empty() {
            parts.push(format!("{} connected", connected.join(", ")));
        }
        if !self.failed.is_empty() {
            parts.push(self.failed.join(", "));
        }
        Some(format!("mcp  {}", parts.join(" · ")))
    }

    pub async fn shutdown(&self) {
        for server in &self.servers {
            server.session.close().await;
        }
    }
}

struct LocatedServer {
    config: ServerConfig,
    scope: Scope,
}

async fn connect_server(
    name: &str,
    account: &str,
    config: ServerConfig,
    credentials: &CredentialStore,
    cwd: &Path,
) -> Result<(McpSession, Vec<McpTool>), McpError> {
    match config {
        ServerConfig::Stdio(config) => {
            let env = resolve_values(name, account, "env", config.env, credentials)?;
            let args = resolve_arguments(name, account, config.args, credentials)?;
            McpSession::connect_stdio(
                name.to_owned(),
                config.command,
                args,
                env.into_iter().collect(),
                cwd,
            )
            .await
        }
        ServerConfig::Http(config) => {
            let headers = resolve_values(name, account, "header", config.headers, credentials)?;
            let mut endpoint = HttpEndpoint::new(config.url.clone());
            endpoint.headers = headers.into_iter().collect();
            if let Some(variable) = &config.bearer_token_env_var {
                let token = std::env::var(variable).map_err(|_| {
                    McpError::Config(format!("environment variable `{variable}` is not set"))
                })?;
                endpoint.auth_header = Some(format!("Bearer {token}"));
            }
            let key = CredentialKey::mcp(name, account, "oauth");
            let session = if config.bearer_token_env_var.is_none()
                && matches!(credentials.get(&key), Some(Credential::OAuth(_)))
            {
                let store = auth::StoredOAuth::new(credentials.clone(), key, None);
                let client = auth::authorized_client(&config.url, store).await?;
                McpSession::connect_http(name.to_owned(), &endpoint, client).await?
            } else {
                McpSession::connect_http(name.to_owned(), &endpoint, http_client()?).await?
            };
            let tools = session.list_tools().await?;
            Ok((session, tools))
        }
    }
}

fn resolve_arguments(
    server: &str,
    account: &str,
    arguments: Vec<ValueSource>,
    credentials: &CredentialStore,
) -> Result<Vec<String>, McpError> {
    arguments
        .into_iter()
        .enumerate()
        .map(|(index, source)| match source {
            ValueSource::Literal(value) => Ok(value),
            ValueSource::Env { env } => std::env::var(&env)
                .map_err(|_| McpError::Config(format!("environment variable `{env}` is not set"))),
            ValueSource::Secret { secret: true } => credentials
                .get(&CredentialKey::mcp(server, account, format!("arg:{index}")))
                .map(|credential| credential.bearer().to_owned())
                .ok_or_else(|| {
                    McpError::Config(format!("stored value for `arg.{index}` is missing"))
                }),
            ValueSource::Secret { secret: false } => Err(McpError::Config(format!(
                "`args.{index}.secret` must be true"
            ))),
        })
        .collect()
}

fn resolve_values(
    server: &str,
    account: &str,
    kind: &str,
    values: BTreeMap<String, ValueSource>,
    credentials: &CredentialStore,
) -> Result<BTreeMap<String, String>, McpError> {
    values
        .into_iter()
        .map(|(name, source)| {
            let value = match source {
                ValueSource::Literal(value) => value,
                ValueSource::Env { env } => std::env::var(&env).map_err(|_| {
                    McpError::Config(format!("environment variable `{env}` is not set"))
                })?,
                ValueSource::Secret { secret: true } => credentials
                    .get(&CredentialKey::mcp(
                        server,
                        account,
                        format!("{kind}:{name}"),
                    ))
                    .map(|credential| credential.bearer().to_owned())
                    .ok_or_else(|| {
                        McpError::Config(format!("stored value for `{kind}.{name}` is missing"))
                    })?,
                ValueSource::Secret { secret: false } => {
                    return Err(McpError::Config(format!(
                        "`{kind}.{name}.secret` must be true"
                    )));
                }
            };
            Ok((name, value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mcp_servers_config() {
        let config = config::parse_compatible(
            br#"{
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
        let server = config.servers.get("filesystem").unwrap();
        let ServerConfig::Stdio(server) = server else {
            panic!("stdio")
        };
        assert_eq!(server.command, "npx");
        assert_eq!(
            server.args,
            [
                ValueSource::Literal("-y".to_owned()),
                ValueSource::Literal("pkg".to_owned())
            ]
        );
        assert_eq!(
            server.env.get("A").unwrap(),
            &ValueSource::Literal("B".to_owned())
        );
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
