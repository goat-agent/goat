mod auth;
mod edit;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use rmcp::model::CallToolRequestParams;
use rmcp::service::{
    ClientInitializeError, Peer, RoleClient, RunningService, ServiceError, ServiceExt,
};
use rmcp::transport::auth::{AuthClient, AuthorizationManager, OAuthClientConfig};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::oauth;
use crate::store::{self, Logins};

pub use auth::SignIn;
pub use edit::{add, remove};

const FILE: &str = "mcp.toml";
const DEPTH: usize = 3;
const CATALOG: &str = "\
MCP servers are connected to the sandbox: {servers}. \
In a run, `await mcp.tools()` lists their tools as [{name, description, input}], \
`await mcp.describe(name)` shows one tool's input, and `await mcp.<server>.<tool>(args)` calls it \
with one object argument.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub name: String,
    pub source: PathBuf,
    pub transport: Transport,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transport {
    Command {
        program: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
    },
    Url {
        url: String,
        headers: Vec<(String, String)>,
        client: Option<OauthClient>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OauthClient {
    pub id: String,
    pub secret: Option<String>,
    pub port: Option<u16>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub servers: Vec<Server>,
    pub unreviewed: Vec<Unreviewed>,
    pub problems: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Digest([u8; 32]);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allowed,
    Denied,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    pub path: PathBuf,
    pub digest: Digest,
    pub decision: Decision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unreviewed {
    pub path: PathBuf,
    pub text: Box<str>,
    digest: Digest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Idle,
    Connected,
    NeedsSignIn,
    Failed(String),
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("there is no MCP server named {0}")]
    Unknown(String),
    #[error("{0} is started by a command and has no sign-in")]
    NotRemote(String),
    #[error("{name} is already in {}", path.display())]
    Exists { name: String, path: PathBuf },
    #[error("read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("write {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parse {}", path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: toml_edit::TomlError,
    },
    #[error("sign in to {server}")]
    Authorize {
        server: String,
        #[source]
        source: Box<rmcp::transport::auth::AuthError>,
    },
    #[error("sign-in callback")]
    Callback(#[from] oauth::Error),
    #[error("database")]
    Store(#[from] store::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Trouble {
    NeedsSignIn,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Listed {
    name: String,
    description: String,
    input: String,
}

#[derive(Debug, Default)]
pub(crate) struct Called {
    pub(crate) value: Value,
    pub(crate) images: Vec<Vec<u8>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    command: Option<Vec<String>>,
    url: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    oauth: Option<OauthEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OauthEntry {
    client_id: String,
    client_secret: Option<String>,
    callback_port: Option<u16>,
}

impl Digest {
    fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    pub(crate) fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Decision {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::Denied => "denied",
        }
    }

    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw {
            "allowed" => Some(Self::Allowed),
            "denied" => Some(Self::Denied),
            _ => None,
        }
    }
}

impl Unreviewed {
    pub fn decide(self, decision: Decision) -> Review {
        Review {
            path: self.path,
            digest: self.digest,
            decision,
        }
    }
}

pub fn personal_file(home: &Path) -> PathBuf {
    home.join(".goat").join(FILE)
}

pub fn project_file(workspace: &Path) -> PathBuf {
    root(workspace).join(".goat").join(FILE)
}

pub fn config(
    workspace: &Path,
    home: &Path,
    variable: fn(&str) -> Option<String>,
    reviews: &[Review],
) -> Config {
    let root = root(workspace);
    let mut project: Vec<PathBuf> = workspace
        .ancestors()
        .take_while(|dir| dir.starts_with(root))
        .map(|dir| dir.join(".goat").join(FILE))
        .collect();
    project.reverse();
    let personal = personal_file(home);
    let mut files = vec![personal.clone()];
    for path in project {
        if !files.contains(&path) {
            files.push(path);
        }
    }
    let mut config = Config::default();
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        if file != personal {
            let digest = Digest::of(text.as_bytes());
            let decision = reviews
                .iter()
                .find(|review| review.path == file && review.digest == digest)
                .map(|review| review.decision);
            match decision {
                Some(Decision::Allowed) => {}
                Some(Decision::Denied) => {
                    config
                        .problems
                        .push(format!("{} is not allowed", file.display()));
                    continue;
                }
                None => {
                    config.unreviewed.push(Unreviewed {
                        path: file,
                        text: text.into(),
                        digest,
                    });
                    continue;
                }
            }
        }
        let entries: BTreeMap<String, Entry> = match toml::from_str(&text) {
            Ok(entries) => entries,
            Err(err) => {
                config
                    .problems
                    .push(format!("{}: {}", file.display(), err.message()));
                continue;
            }
        };
        for (name, entry) in entries {
            let mut expand = |value: &str| expand(value, variable, &name, &mut config.problems);
            let transport = match (entry.command, entry.url) {
                (Some(_), None) if entry.oauth.is_some() => {
                    config.problems.push(format!("{name}: oauth needs a url"));
                    continue;
                }
                (Some(command), None) if !command.is_empty() => Transport::Command {
                    program: expand(&command[0]),
                    args: command[1..].iter().map(|arg| expand(arg)).collect(),
                    env: entry
                        .env
                        .iter()
                        .map(|(key, value)| (key.clone(), expand(value)))
                        .collect(),
                },
                (None, Some(url)) => Transport::Url {
                    url: expand(&url),
                    headers: entry
                        .headers
                        .iter()
                        .map(|(key, value)| (key.clone(), expand(value)))
                        .collect(),
                    client: entry.oauth.map(|oauth| OauthClient {
                        id: expand(&oauth.client_id),
                        secret: oauth.client_secret.as_deref().map(&mut expand),
                        port: oauth.callback_port,
                    }),
                },
                _ => {
                    config
                        .problems
                        .push(format!("{name} needs either command or url"));
                    continue;
                }
            };
            config.servers.retain(|server| server.name != name);
            config.servers.push(Server {
                name,
                source: file.clone(),
                transport,
            });
        }
    }
    config
}

pub fn signed_in(logins: &Logins, server: &Server) -> Result<bool, Error> {
    let Transport::Url { url, .. } = &server.transport else {
        return Ok(false);
    };
    Ok(logins.get(&login_key(url))?.is_some())
}

pub fn sign_out(logins: &Logins, server: &Server) -> Result<bool, Error> {
    let Transport::Url { url, .. } = &server.transport else {
        return Err(Error::NotRemote(server.name.clone()));
    };
    Ok(logins.remove(&login_key(url))?)
}

pub async fn check(server: &Server, http: &reqwest::Client, logins: Arc<Logins>) -> State {
    match connect(server, http, &logins).await {
        Ok(service) => {
            service.cancel().await.ok();
            State::Connected
        }
        Err(Trouble::NeedsSignIn) => State::NeedsSignIn,
        Err(Trouble::Failed(problem)) => State::Failed(problem),
    }
}

fn root(workspace: &Path) -> &Path {
    workspace
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .unwrap_or(workspace)
}

fn login_key(url: &str) -> String {
    format!("mcp:{url}")
}

pub(crate) fn catalog(servers: &[Server]) -> Option<String> {
    if servers.is_empty() {
        return None;
    }
    let names: Vec<&str> = servers.iter().map(|server| server.name.as_str()).collect();
    Some(CATALOG.replace("{servers}", &names.join(", ")))
}

fn expand(
    value: &str,
    variable: fn(&str) -> Option<String>,
    server: &str,
    problems: &mut Vec<String>,
) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        if let Some(tail) = after.strip_prefix('$') {
            out.push('$');
            rest = tail;
            continue;
        }
        let (name, tail) = if let Some(inner) = after.strip_prefix('{') {
            inner
                .find('}')
                .map_or(("", after), |end| (&inner[..end], &inner[end + 1..]))
        } else {
            let end = after
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(after.len());
            (&after[..end], &after[end..])
        };
        if name.is_empty() {
            out.push('$');
            rest = after;
            continue;
        }
        match variable(name) {
            Some(found) => out.push_str(&found),
            None => problems.push(format!("{server}: ${name} is not set")),
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

#[derive(Debug)]
pub(crate) struct Clients {
    servers: Vec<Server>,
    http: reqwest::Client,
    logins: Arc<Logins>,
    live: RefCell<HashMap<String, RunningService<RoleClient, ()>>>,
    troubled: RefCell<HashMap<String, Trouble>>,
}

impl Clients {
    pub(crate) fn new(servers: Vec<Server>, http: reqwest::Client, logins: Arc<Logins>) -> Self {
        Self {
            servers,
            http,
            logins,
            live: RefCell::new(HashMap::new()),
            troubled: RefCell::new(HashMap::new()),
        }
    }

    pub(crate) fn states(&self) -> Vec<(String, State)> {
        self.servers
            .iter()
            .map(|server| {
                let state = match self.troubled.borrow().get(&server.name) {
                    Some(Trouble::NeedsSignIn) => State::NeedsSignIn,
                    Some(Trouble::Failed(problem)) => State::Failed(problem.clone()),
                    None if self
                        .live
                        .borrow()
                        .get(&server.name)
                        .is_some_and(|service| !service.is_closed()) =>
                    {
                        State::Connected
                    }
                    None => State::Idle,
                };
                (server.name.clone(), state)
            })
            .collect()
    }

    pub(crate) fn reset(&self, name: &str) {
        self.live.borrow_mut().remove(name);
        self.troubled.borrow_mut().remove(name);
    }

    pub(crate) async fn tools(&self, only: Option<&str>) -> Result<Vec<Listed>, String> {
        if let Some(only) = only {
            if !self.servers.iter().any(|server| server.name == only) {
                return Err(format!("there is no MCP server named {only}"));
            }
            return self.server_tools(only).await;
        }
        let mut listed = Vec::new();
        for server in &self.servers {
            match self.server_tools(&server.name).await {
                Ok(tools) => listed.extend(tools),
                Err(problem) => listed.push(Listed {
                    name: server.name.clone(),
                    description: format!("unavailable: {problem}"),
                    input: String::new(),
                }),
            }
        }
        Ok(listed)
    }

    pub(crate) async fn describe(&self, full: &str) -> Result<String, String> {
        let (server, name) = full
            .split_once('.')
            .ok_or_else(|| format!("{full} is not in the form server.tool"))?;
        let peer = self.peer(server).await?;
        let tools = peer.list_all_tools().await.map_err(|err| {
            self.lost(server, &err).unwrap_or_else(|| {
                format!("{server} could not list its tools: {}", call_cause(&err))
            })
        })?;
        let tool = tools
            .iter()
            .find(|tool| tool.name == name)
            .ok_or_else(|| format!("{server} has no tool named {name}"))?;
        let schema = Value::Object((*tool.input_schema).clone());
        let mut text = String::new();
        if let Some(description) = tool.description.as_deref().filter(|text| !text.is_empty()) {
            writeln!(text, "/** {} */", description.trim()).ok();
        }
        write!(
            text,
            "mcp.{server}.{name}(args: {}): Promise<unknown>",
            typed(&schema, 0)
        )
        .ok();
        Ok(text)
    }

    pub(crate) async fn call(
        &self,
        server: &str,
        tool: &str,
        args: Value,
    ) -> Result<Called, String> {
        let peer = self.peer(server).await?;
        let arguments = match args {
            Value::Object(map) => Some(map),
            Value::Null => None,
            _ => return Err(format!("{server}.{tool} takes one object argument")),
        };
        let mut params = CallToolRequestParams::new(tool.to_owned());
        params.arguments = arguments;
        let result = peer.call_tool(params).await.map_err(|err| {
            self.lost(server, &err)
                .unwrap_or_else(|| format!("{server}.{tool} failed: {}", call_cause(&err)))
        })?;
        let result = serde_json::to_value(result).map_err(|err| err.to_string())?;
        called(&result).map_err(|problem| format!("{server}.{tool} failed: {problem}"))
    }

    async fn server_tools(&self, name: &str) -> Result<Vec<Listed>, String> {
        let peer = self.peer(name).await?;
        let tools = peer.list_all_tools().await.map_err(|err| {
            self.lost(name, &err)
                .unwrap_or_else(|| format!("{name} could not list its tools: {}", call_cause(&err)))
        })?;
        Ok(tools
            .into_iter()
            .map(|tool| Listed {
                name: format!("{name}.{}", tool.name),
                description: tool.description.as_deref().unwrap_or_default().to_owned(),
                input: brief(&Value::Object((*tool.input_schema).clone())),
            })
            .collect())
    }

    async fn peer(&self, name: &str) -> Result<Peer<RoleClient>, String> {
        if let Some(service) = self
            .live
            .borrow()
            .get(name)
            .filter(|service| !service.is_closed())
        {
            return Ok(service.peer().clone());
        }
        let server = self
            .servers
            .iter()
            .find(|server| server.name == name)
            .ok_or_else(|| format!("there is no MCP server named {name}"))?;
        match connect(server, &self.http, &self.logins).await {
            Ok(service) => {
                let peer = service.peer().clone();
                self.troubled.borrow_mut().remove(name);
                self.live.borrow_mut().insert(name.to_owned(), service);
                Ok(peer)
            }
            Err(trouble) => {
                let problem = match &trouble {
                    Trouble::NeedsSignIn => sign_in_needed(name),
                    Trouble::Failed(problem) => problem.clone(),
                };
                self.troubled.borrow_mut().insert(name.to_owned(), trouble);
                Err(problem)
            }
        }
    }

    fn lost(&self, name: &str, err: &ServiceError) -> Option<String> {
        let ServiceError::TransportSend(transport) = err else {
            return None;
        };
        if !auth::needs_sign_in(transport) {
            return None;
        }
        self.live.borrow_mut().remove(name);
        self.troubled
            .borrow_mut()
            .insert(name.to_owned(), Trouble::NeedsSignIn);
        Some(sign_in_needed(name))
    }
}

async fn connect(
    server: &Server,
    http: &reqwest::Client,
    logins: &Arc<Logins>,
) -> Result<RunningService<RoleClient, ()>, Trouble> {
    let name = &server.name;
    match &server.transport {
        Transport::Command { program, args, env } => {
            let mut command = tokio::process::Command::new(program);
            command
                .args(args)
                .envs(env.iter().map(|(key, value)| (key, value)));
            let (transport, _) = TokioChildProcess::builder(command)
                .stderr(Stdio::null())
                .spawn()
                .map_err(|err| Trouble::Failed(format!("{name} could not start: {err}")))?;
            ().serve(transport).await.map_err(|err| {
                Trouble::Failed(format!("{name} did not start: {}", start_cause(&err)))
            })
        }
        Transport::Url {
            url,
            headers,
            client,
        } => {
            let mut map = HashMap::new();
            for (key, value) in headers {
                let key = reqwest::header::HeaderName::try_from(key.as_str())
                    .map_err(|err| Trouble::Failed(format!("{name}: header {key}: {err}")))?;
                let value = reqwest::header::HeaderValue::try_from(value.as_str())
                    .map_err(|err| Trouble::Failed(format!("{name}: header {key}: {err}")))?;
                map.insert(key, value);
            }
            let config =
                StreamableHttpClientTransportConfig::with_uri(url.as_str()).custom_headers(map);
            let is_signed_in = logins
                .get(&login_key(url))
                .map_err(|err| Trouble::Failed(format!("{name}: {err}")))?
                .is_some();
            let started = if is_signed_in {
                let manager = authorized(url, client.as_ref(), logins)
                    .await
                    .map_err(|err| {
                        if auth::is_sign_in_error(&err) {
                            Trouble::NeedsSignIn
                        } else {
                            Trouble::Failed(format!("{name} did not connect: {err}"))
                        }
                    })?;
                let client = AuthClient::new(http.clone(), manager);
                ().serve(StreamableHttpClientTransport::with_client(client, config))
                    .await
            } else {
                ().serve(StreamableHttpClientTransport::with_client(
                    http.clone(),
                    config,
                ))
                .await
            };
            started.map_err(|err| match &err {
                ClientInitializeError::TransportError { error, .. }
                    if auth::needs_sign_in(error) =>
                {
                    Trouble::NeedsSignIn
                }
                _ => Trouble::Failed(format!("{name} did not connect: {}", start_cause(&err))),
            })
        }
    }
}

async fn authorized(
    url: &str,
    client: Option<&OauthClient>,
    logins: &Arc<Logins>,
) -> Result<AuthorizationManager, rmcp::transport::auth::AuthError> {
    let mut manager = AuthorizationManager::new(url).await?;
    manager.set_credential_store(auth::Saved::new(Arc::clone(logins), url));
    manager.initialize_from_store().await?;
    if let Some(OauthClient {
        id,
        secret: Some(secret),
        ..
    }) = client
    {
        manager.configure_client(OAuthClientConfig::new(id, url).with_client_secret(secret))?;
    }
    Ok(manager)
}

fn start_cause(err: &ClientInitializeError) -> String {
    match err {
        ClientInitializeError::TransportError { error, .. } => error.error.to_string(),
        _ => err.to_string(),
    }
}

fn call_cause(err: &ServiceError) -> String {
    match err {
        ServiceError::TransportSend(error) => error.error.to_string(),
        _ => err.to_string(),
    }
}

fn sign_in_needed(name: &str) -> String {
    format!("{name} needs sign-in; ask the user to run /mcp {name}")
}

fn called(result: &Value) -> Result<Called, String> {
    let mut texts = Vec::new();
    let mut images = Vec::new();
    for block in result["content"].as_array().into_iter().flatten() {
        match block["type"].as_str() {
            Some("text") => texts.push(block["text"].as_str().unwrap_or_default().to_owned()),
            Some("image") => {
                if let Some(bytes) = block["data"]
                    .as_str()
                    .and_then(|data| BASE64.decode(data).ok())
                {
                    images.push(bytes);
                }
            }
            Some("resource") => {
                let resource = &block["resource"];
                match resource["text"].as_str() {
                    Some(text) => texts.push(text.to_owned()),
                    None => texts.push(format!(
                        "[resource {}]",
                        resource["uri"].as_str().unwrap_or("?")
                    )),
                }
            }
            Some("resource_link") => {
                texts.push(format!("[link {}]", block["uri"].as_str().unwrap_or("?")));
            }
            _ => {}
        }
    }
    let text = texts.join("\n");
    if result["isError"].as_bool() == Some(true) {
        return Err(if text.is_empty() {
            "the tool reported an error".to_owned()
        } else {
            text
        });
    }
    let value = match result.get("structuredContent") {
        Some(structured) if !structured.is_null() => structured.clone(),
        _ => Value::String(text),
    };
    Ok(Called { value, images })
}

fn brief(schema: &Value) -> String {
    let required: Vec<&str> = schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let fields: Vec<String> = schema["properties"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(key, property)| {
            let optional = if required.contains(&key.as_str()) {
                ""
            } else {
                "?"
            };
            format!("{key}{optional}: {}", typed(property, DEPTH))
        })
        .collect();
    fields.join(", ")
}

fn typed(schema: &Value, depth: usize) -> String {
    if let Some(choices) = schema["enum"].as_array() {
        let shown: Vec<String> = choices.iter().map(Value::to_string).collect();
        return shown.join(" | ");
    }
    match schema["type"].as_str() {
        Some("string") => "string".to_owned(),
        Some("integer" | "number") => "number".to_owned(),
        Some("boolean") => "boolean".to_owned(),
        Some("null") => "null".to_owned(),
        Some("array") => {
            let item = typed(&schema["items"], depth + 1);
            if item.contains(' ') {
                format!("({item})[]")
            } else {
                format!("{item}[]")
            }
        }
        Some("object") | None if schema.get("properties").is_some() => {
            if depth >= DEPTH {
                return "object".to_owned();
            }
            let required: Vec<&str> = schema["required"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let indent = "  ".repeat(depth + 1);
            let mut out = "{\n".to_owned();
            for (key, property) in schema["properties"].as_object().into_iter().flatten() {
                if let Some(description) = property["description"].as_str() {
                    writeln!(out, "{indent}/** {} */", description.trim()).ok();
                }
                let optional = if required.contains(&key.as_str()) {
                    ""
                } else {
                    "?"
                };
                writeln!(
                    out,
                    "{indent}{key}{optional}: {};",
                    typed(property, depth + 1)
                )
                .ok();
            }
            out.push_str(&"  ".repeat(depth));
            out.push('}');
            out
        }
        Some("object") => "object".to_owned(),
        _ => "unknown".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn variable(name: &str) -> Option<String> {
        (name == "TOKEN").then(|| "secret".to_owned())
    }

    fn project(dir: &Path) -> (PathBuf, PathBuf) {
        let home = dir.join("home");
        let workspace = dir.join("work");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(workspace.join(".goat")).unwrap();
        std::fs::write(
            workspace.join(".goat/mcp.toml"),
            "[notes]\ncommand = [\"notes-mcp\"]\n",
        )
        .unwrap();
        (workspace, home)
    }

    fn review(workspace: &Path, decision: Decision) -> Review {
        let path = workspace.join(".goat/mcp.toml");
        let text = std::fs::read(&path).unwrap();
        Review {
            path,
            digest: Digest::of(&text),
            decision,
        }
    }

    fn allowed(workspace: &Path) -> Review {
        review(workspace, Decision::Allowed)
    }

    #[test]
    fn a_project_file_without_a_review_is_held_back_for_review() {
        let dir = tempfile::tempdir().unwrap();
        let (workspace, home) = project(dir.path());

        let config = config(&workspace, &home, variable, &[]);

        assert_eq!(config.servers, Vec::new());
        assert_eq!(config.unreviewed.len(), 1);
        assert_eq!(config.unreviewed[0].path, workspace.join(".goat/mcp.toml"));
        assert_eq!(
            &*config.unreviewed[0].text,
            "[notes]\ncommand = [\"notes-mcp\"]\n"
        );
    }

    #[test]
    fn an_allowed_project_file_is_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let (workspace, home) = project(dir.path());
        let review = config(&workspace, &home, variable, &[]).unreviewed[0]
            .clone()
            .decide(Decision::Allowed);

        let config = config(&workspace, &home, variable, &[review]);

        assert_eq!(config.servers.len(), 1);
        assert_eq!(config.servers[0].name, "notes");
        assert_eq!(config.unreviewed, Vec::new());
    }

    #[test]
    fn a_denied_project_file_is_skipped_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        let (workspace, home) = project(dir.path());
        let denied = review(&workspace, Decision::Denied);

        let config = config(&workspace, &home, variable, &[denied]);

        assert_eq!(config.servers, Vec::new());
        assert_eq!(config.unreviewed, Vec::new());
        assert_eq!(
            config.problems,
            [format!(
                "{} is not allowed",
                workspace.join(".goat/mcp.toml").display()
            )]
        );
    }

    #[test]
    fn a_project_file_changed_after_it_was_allowed_is_reviewed_again() {
        let dir = tempfile::tempdir().unwrap();
        let (workspace, home) = project(dir.path());
        let allowed = allowed(&workspace);
        std::fs::write(
            workspace.join(".goat/mcp.toml"),
            "[notes]\ncommand = [\"curl\", \"https://evil.example\"]\n",
        )
        .unwrap();

        let config = config(&workspace, &home, variable, &[allowed]);

        assert_eq!(config.servers, Vec::new());
        assert_eq!(config.unreviewed.len(), 1);
    }

    #[test]
    fn the_personal_file_needs_no_review() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join(".goat")).unwrap();
        std::fs::write(
            home.join(".goat/mcp.toml"),
            "[notes]\nurl = \"https://x\"\n",
        )
        .unwrap();

        let config = config(&dir.path().join("work"), &home, variable, &[]);

        assert_eq!(config.servers.len(), 1);
        assert_eq!(config.unreviewed, Vec::new());
    }

    #[test]
    fn servers_come_from_toml_with_variables_expanded_and_projects_winning() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let workspace = dir.path().join("work");
        std::fs::create_dir_all(home.join(".goat")).unwrap();
        std::fs::create_dir_all(workspace.join(".goat")).unwrap();
        std::fs::write(
            home.join(".goat/mcp.toml"),
            "[github]\ncommand = [\"gh-mcp\", \"--token\", \"$TOKEN\"]\n\n[notes]\nurl = \"https://example.com/mcp\"\nheaders = { Authorization = \"Bearer ${TOKEN}\" }\n",
        )
        .unwrap();
        std::fs::write(
            workspace.join(".goat/mcp.toml"),
            "[notes]\ncommand = [\"notes-mcp\"]\nenv = { KEY = \"$MISSING\" }\n",
        )
        .unwrap();

        let config = config(&workspace, &home, variable, &[allowed(&workspace)]);

        assert_eq!(config.servers.len(), 2);
        assert_eq!(
            config.servers[0].transport,
            Transport::Command {
                program: "gh-mcp".to_owned(),
                args: vec!["--token".to_owned(), "secret".to_owned()],
                env: Vec::new(),
            }
        );
        assert_eq!(config.servers[1].source, workspace.join(".goat/mcp.toml"));
        assert_eq!(config.problems, ["notes: $MISSING is not set"]);
    }

    #[test]
    fn an_oauth_client_is_read_with_variables_expanded() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".goat")).unwrap();
        std::fs::write(
            dir.path().join(".goat/mcp.toml"),
            "[slack]\nurl = \"https://mcp.slack.com/mcp\"\noauth = { client_id = \"123\", client_secret = \"$TOKEN\", callback_port = 3118 }\n",
        )
        .unwrap();

        let config = config(dir.path(), dir.path(), variable, &[]);

        assert_eq!(
            config.servers[0].transport,
            Transport::Url {
                url: "https://mcp.slack.com/mcp".to_owned(),
                headers: Vec::new(),
                client: Some(OauthClient {
                    id: "123".to_owned(),
                    secret: Some("secret".to_owned()),
                    port: Some(3118),
                }),
            }
        );
    }

    #[test]
    fn oauth_on_a_command_server_is_a_problem() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".goat")).unwrap();
        std::fs::write(
            dir.path().join(".goat/mcp.toml"),
            "[local]\ncommand = [\"x\"]\noauth = { client_id = \"123\" }\n",
        )
        .unwrap();

        let config = config(dir.path(), dir.path(), variable, &[]);

        assert_eq!(config.servers, Vec::new());
        assert_eq!(config.problems, ["local: oauth needs a url"]);
    }

    #[test]
    fn a_server_needs_exactly_one_way_to_reach_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".goat")).unwrap();
        std::fs::write(
            dir.path().join(".goat/mcp.toml"),
            "[both]\ncommand = [\"x\"]\nurl = \"https://x\"\n\n[typo]\ncomand = [\"x\"]\n",
        )
        .unwrap();

        let config = config(dir.path(), dir.path(), variable, &[]);

        assert_eq!(config.servers, Vec::new());
        assert_eq!(config.problems.len(), 1);
        assert!(config.problems[0].contains("unknown field"));
    }

    #[test]
    fn schemas_read_as_typescript() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "title": { "type": "string", "description": "Issue title" },
                "labels": { "type": "array", "items": { "type": "string" } },
                "state": { "enum": ["open", "closed"] }
            },
            "required": ["title"]
        });

        assert_eq!(
            typed(&schema, 0),
            "{\n  labels?: string[];\n  state?: \"open\" | \"closed\";\n  /** Issue title */\n  title: string;\n}"
        );
    }

    #[test]
    fn a_brief_input_names_each_argument_on_one_line() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "message": { "type": "string" },
                "options": { "type": "object", "properties": { "loud": { "type": "boolean" } } }
            },
            "required": ["message"]
        });

        assert_eq!(brief(&schema), "message: string, options?: object");
    }

    #[test]
    fn results_prefer_structured_content_and_errors_throw() {
        let structured = serde_json::json!({
            "content": [{ "type": "text", "text": "{\"total\":3}" }],
            "structuredContent": { "total": 3 }
        });
        let failed = serde_json::json!({
            "content": [{ "type": "text", "text": "no such repo" }],
            "isError": true
        });

        assert_eq!(
            called(&structured).unwrap().value,
            serde_json::json!({ "total": 3 })
        );
        assert_eq!(called(&failed).unwrap_err(), "no such repo");
    }
}
