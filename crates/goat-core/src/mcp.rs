mod auth;
pub mod builtin;
mod edit;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use rmcp::ErrorData;
use rmcp::handler::client::ClientHandler;
use rmcp::model::{
    CallToolRequestParams, ClientCapabilities, ClientConfig, ElicitRequestParams, ElicitResult,
    ElicitationAction, ElicitationCapability, FormElicitationCapability, Implementation,
    ProtocolVersion,
};
use rmcp::service::RequestContext;
use rmcp::service::{
    ClientInitializeError, ClientLifecycleMode, ClientServiceExt, Peer, RoleClient, RunningService,
    ServiceError,
};
use rmcp::transport::auth::{AuthClient, AuthorizationManager, OAuthClientConfig};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::oauth;
use crate::provider::Endpoints;
use crate::store::{self, Logins};
use crate::tool::{self, Question, Step};

pub use auth::SignIn;
pub use builtin::Builtin;
pub use edit::{add, remove};

const FILE: &str = "mcp.toml";
const DEPTH: usize = 3;
const CATALOG: &str = "\
MCP servers are connected to the sandbox: {servers}. \
In a run, `await mcp.tools()` lists their tools as [{name, description, input}]; \
an entry named by a server alone gives that server's own instructions, which you follow when you use it, \
or why it is unavailable. `await mcp.describe(name)` shows one tool's input, and `await mcp.<server>.<tool>(args)` calls it \
with one object argument.";
const ACCOUNTS: &str = "\
A server signed in with another account is named server#account and reached as \
`mcp[\"server#account\"].<tool>(args)`.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub name: String,
    pub source: Source,
    pub transport: Transport,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    File(PathBuf),
    Plugin(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    pub name: String,
    pub account: Option<String>,
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
    Builtin(Builtin),
}

#[derive(Debug, Clone)]
pub struct Context {
    pub http: reqwest::Client,
    pub logins: Arc<Logins>,
    pub endpoints: Endpoints,
    pub home: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub name: String,
    pub state: State,
    pub plugin: Option<String>,
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
    #[error(
        "{0} is not a name; names use letters, digits, - and _, and name#account picks an account"
    )]
    BadName(String),
    #[error("{server} comes from the {plugin} plugin; use `goat plugin logout {plugin}`")]
    FromPlugin { server: String, plugin: String },
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

#[derive(Debug)]
struct Live {
    client: RunningService<RoleClient, Asker>,
    server: Option<builtin::Running>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Asker {
    steps: Option<tokio::sync::mpsc::UnboundedSender<Step>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    command: Option<Vec<String>>,
    url: Option<String>,
    builtin: Option<String>,
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

impl Transport {
    fn identity(&self) -> Option<String> {
        match self {
            Self::Url { url, .. } => Some(url.clone()),
            Self::Builtin(builtin) => Some(format!("builtin:{}", builtin.as_str())),
            Self::Command { .. } => None,
        }
    }
}

impl Live {
    fn is_open(&self) -> bool {
        !self.client.is_closed()
    }

    async fn close(self) {
        self.client.cancel().await.ok();
        if let Some(server) = self.server {
            server.cancel().await;
        }
    }
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File(path) => write!(f, "{}", path.display()),
            Self::Plugin(name) => write!(f, "the {name} plugin"),
        }
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

impl Address {
    pub fn parse(raw: &str) -> Result<Self, Error> {
        let (name, account) = match raw.split_once('#') {
            Some((name, account)) => (name, Some(account)),
            None => (raw, None),
        };
        if !is_name(name) || account.is_some_and(|account| !is_name(account)) {
            return Err(Error::BadName(raw.to_owned()));
        }
        Ok(Self {
            name: name.to_owned(),
            account: account.map(str::to_owned),
        })
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.account {
            Some(account) => write!(f, "{}#{account}", self.name),
            None => f.write_str(&self.name),
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
        let source = Source::File(file);
        for server in parse(&text, &source, variable, &mut config.problems) {
            config.servers.retain(|known| known.name != server.name);
            config.servers.push(server);
        }
    }
    config
}

pub fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub(crate) fn parse(
    text: &str,
    source: &Source,
    variable: fn(&str) -> Option<String>,
    problems: &mut Vec<String>,
) -> Vec<Server> {
    let entries: BTreeMap<String, Entry> = match toml::from_str(text) {
        Ok(entries) => entries,
        Err(err) => {
            problems.push(format!("{source}: {}", err.message()));
            return Vec::new();
        }
    };
    let mut servers = Vec::new();
    for (name, entry) in entries {
        if !is_name(&name) {
            problems.push(format!(
                "{name}: a server name uses letters, digits, - and _"
            ));
            continue;
        }
        let mut expand = |value: &str| expand(value, variable, &name, problems);
        let transport = match (entry.command, entry.url, entry.builtin) {
            (Some(_), None, None) if entry.oauth.is_some() => {
                problems.push(format!("{name}: oauth needs a url"));
                continue;
            }
            (None, None, Some(builtin)) => match Builtin::parse(&builtin) {
                Some(builtin) if entry.oauth.is_none() => Transport::Builtin(builtin),
                Some(_) => {
                    problems.push(format!("{name}: a builtin server signs in by itself"));
                    continue;
                }
                None => {
                    problems.push(format!(
                        "{name}: goat has no builtin server named {builtin}"
                    ));
                    continue;
                }
            },
            (Some(command), None, None) if !command.is_empty() => Transport::Command {
                program: expand(&command[0]),
                args: command[1..].iter().map(|arg| expand(arg)).collect(),
                env: entry
                    .env
                    .iter()
                    .map(|(key, value)| (key.clone(), expand(value)))
                    .collect(),
            },
            (None, Some(url), None) => Transport::Url {
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
                problems.push(format!("{name} needs one of command, url or builtin"));
                continue;
            }
        };
        servers.push(Server {
            name,
            source: source.clone(),
            transport,
        });
    }
    servers
}

pub fn sign_ins(logins: &Logins, server: &Server) -> Result<Vec<Option<String>>, store::Error> {
    let Some(identity) = server.transport.identity() else {
        return Ok(Vec::new());
    };
    let key = login_key(&identity, None);
    let prefix = format!("{key}#");
    let default = logins.get(&key)?.map(|_| None);
    let accounts = logins
        .keys(&prefix)?
        .into_iter()
        .map(|key| Some(key[prefix.len()..].to_owned()));
    Ok(default.into_iter().chain(accounts).collect())
}

pub fn sign_out(logins: &Logins, server: &Server, account: Option<&str>) -> Result<bool, Error> {
    let Some(identity) = server.transport.identity() else {
        return Err(Error::NotRemote(server.name.clone()));
    };
    Ok(logins.remove(&login_key(&identity, account))?)
}

pub async fn check(server: &Server, context: &Context) -> State {
    match connect(server, None, context, Asker::default()).await {
        Ok(live) => {
            live.close().await;
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

fn login_key(identity: &str, account: Option<&str>) -> String {
    match account {
        Some(account) => format!("mcp:{identity}#{account}"),
        None => format!("mcp:{identity}"),
    }
}

pub(crate) fn addresses(servers: &[Server], logins: &Logins) -> Vec<Address> {
    servers
        .iter()
        .flat_map(|server| {
            let accounts = sign_ins(logins, server).unwrap_or_default();
            std::iter::once(None)
                .chain(accounts.into_iter().flatten().map(Some))
                .map(|account| Address {
                    name: server.name.clone(),
                    account,
                })
        })
        .collect()
}

pub(crate) fn catalog(addresses: &[Address]) -> Option<String> {
    if addresses.is_empty() {
        return None;
    }
    let names: Vec<String> = addresses.iter().map(Address::to_string).collect();
    let mut text = CATALOG.replace("{servers}", &names.join(", "));
    if addresses.iter().any(|address| address.account.is_some()) {
        text.push(' ');
        text.push_str(ACCOUNTS);
    }
    Some(text)
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
    context: Context,
    asker: Asker,
    live: RefCell<HashMap<String, Live>>,
    troubled: RefCell<HashMap<String, Trouble>>,
    destructive: RefCell<HashMap<String, HashSet<String>>>,
}

impl Clients {
    pub(crate) fn new(
        servers: Vec<Server>,
        context: Context,
        steps: tokio::sync::mpsc::UnboundedSender<Step>,
    ) -> Self {
        Self {
            servers,
            context,
            asker: Asker { steps: Some(steps) },
            live: RefCell::new(HashMap::new()),
            troubled: RefCell::new(HashMap::new()),
            destructive: RefCell::new(HashMap::new()),
        }
    }

    pub(crate) async fn is_destructive(&self, name: &str, tool: &str) -> bool {
        if let Some(found) = self.destructive.borrow().get(name) {
            return found.contains(tool);
        }
        let Ok(peer) = self.peer(name).await else {
            return false;
        };
        let Ok(tools) = peer.list_all_tools().await else {
            return false;
        };
        let found: HashSet<String> = tools
            .iter()
            .filter(|tool| {
                tool.annotations
                    .as_ref()
                    .and_then(|annotations| annotations.destructive_hint)
                    == Some(true)
            })
            .map(|tool| tool.name.to_string())
            .collect();
        let is_destructive = found.contains(tool);
        self.destructive.borrow_mut().insert(name.to_owned(), found);
        is_destructive
    }

    pub(crate) fn states(&self) -> Vec<Status> {
        addresses(&self.servers, &self.context.logins)
            .iter()
            .map(|address| {
                let shown = address.to_string();
                let state = match self.troubled.borrow().get(&shown) {
                    Some(Trouble::NeedsSignIn) => State::NeedsSignIn,
                    Some(Trouble::Failed(problem)) => State::Failed(problem.clone()),
                    None if self.live.borrow().get(&shown).is_some_and(Live::is_open) => {
                        State::Connected
                    }
                    None => State::Idle,
                };
                let plugin = self
                    .servers
                    .iter()
                    .find(|server| server.name == address.name)
                    .and_then(|server| match &server.source {
                        Source::Plugin(plugin) => Some(plugin.clone()),
                        Source::File(_) => None,
                    });
                Status {
                    name: shown,
                    state,
                    plugin,
                }
            })
            .collect()
    }

    pub(crate) fn reset(&self, name: &str) {
        self.live.borrow_mut().remove(name);
        self.troubled.borrow_mut().remove(name);
    }

    pub(crate) async fn tools(&self, only: Option<&str>) -> Result<Vec<Listed>, String> {
        if let Some(only) = only {
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
        let reached = if server.contains('#') {
            format!("mcp[\"{server}\"]")
        } else {
            format!("mcp.{server}")
        };
        write!(
            text,
            "{reached}.{name}(args: {}): Promise<unknown>",
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
        let instructions = peer
            .peer_info()
            .and_then(|info| {
                info.instructions
                    .as_deref()
                    .map(str::trim)
                    .map(str::to_owned)
            })
            .filter(|text| !text.is_empty())
            .map(|description| Listed {
                name: name.to_owned(),
                description,
                input: String::new(),
            });
        Ok(instructions
            .into_iter()
            .chain(tools.into_iter().map(|tool| Listed {
                name: format!("{name}.{}", tool.name),
                description: tool.description.as_deref().unwrap_or_default().to_owned(),
                input: brief(&Value::Object((*tool.input_schema).clone())),
            }))
            .collect())
    }

    async fn peer(&self, name: &str) -> Result<Peer<RoleClient>, String> {
        if let Some(live) = self.live.borrow().get(name).filter(|live| live.is_open()) {
            return Ok(live.client.peer().clone());
        }
        let address = Address::parse(name).map_err(|err| err.to_string())?;
        let server = self
            .servers
            .iter()
            .find(|server| server.name == address.name)
            .ok_or_else(|| format!("there is no MCP server named {}", address.name))?;
        match connect(
            server,
            address.account.as_deref(),
            &self.context,
            self.asker.clone(),
        )
        .await
        {
            Ok(live) => {
                let peer = live.client.peer().clone();
                self.troubled.borrow_mut().remove(name);
                self.live.borrow_mut().insert(name.to_owned(), live);
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
    account: Option<&str>,
    context: &Context,
    asker: Asker,
) -> Result<Live, Trouble> {
    let name = &server.name;
    let (http, logins) = (&context.http, &context.logins);
    let client = match &server.transport {
        Transport::Builtin(builtin) => {
            return start_builtin(*builtin, name, account, context, asker).await;
        }
        Transport::Command { program, args, env } => {
            let mut command = tokio::process::Command::new(program);
            command
                .args(args)
                .envs(env.iter().map(|(key, value)| (key, value)));
            let (transport, _) = TokioChildProcess::builder(command)
                .stderr(Stdio::null())
                .spawn()
                .map_err(|err| Trouble::Failed(format!("{name} could not start: {err}")))?;
            asker
                .clone()
                .serve_with_lifecycle(transport, lifecycle())
                .await
                .map_err(|err| {
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
                .get(&login_key(url, account))
                .map_err(|err| Trouble::Failed(format!("{name}: {err}")))?
                .is_some();
            let started = if is_signed_in {
                let manager = authorized(url, account, client.as_ref(), logins)
                    .await
                    .map_err(|err| {
                        if auth::is_sign_in_error(&err) {
                            Trouble::NeedsSignIn
                        } else {
                            Trouble::Failed(format!("{name} did not connect: {err}"))
                        }
                    })?;
                let client = AuthClient::new(http.clone(), manager);
                asker
                    .clone()
                    .serve_with_lifecycle(
                        StreamableHttpClientTransport::with_client(client, config),
                        lifecycle(),
                    )
                    .await
            } else {
                asker
                    .clone()
                    .serve_with_lifecycle(
                        StreamableHttpClientTransport::with_client(http.clone(), config),
                        lifecycle(),
                    )
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
    }?;
    Ok(Live {
        client,
        server: None,
    })
}

async fn start_builtin(
    builtin: Builtin,
    name: &str,
    account: Option<&str>,
    context: &Context,
    asker: Asker,
) -> Result<Live, Trouble> {
    let key = login_key(&format!("builtin:{}", builtin.as_str()), account);
    if builtin.needs_sign_in() {
        let is_signed_in = context
            .logins
            .get(&key)
            .map_err(|err| Trouble::Failed(format!("{name}: {err}")))?
            .is_some();
        if !is_signed_in {
            return Err(Trouble::NeedsSignIn);
        }
    }
    let (near, far) = builtin::pipe();
    let (server, client) = tokio::join!(
        builtin.serve(far, context, key),
        asker.serve_with_lifecycle(near, ClientLifecycleMode::Initialize)
    );
    let server = server.map_err(Trouble::Failed)?;
    let client = client
        .map_err(|err| Trouble::Failed(format!("{name} did not start: {}", start_cause(&err))))?;
    Ok(Live {
        client,
        server: Some(server),
    })
}

impl ClientHandler for Asker {
    fn get_info(&self) -> ClientConfig {
        let mut capabilities = ClientCapabilities::default();
        capabilities.elicitation =
            Some(ElicitationCapability::new().with_form(FormElicitationCapability::new()));
        ClientConfig::new(
            capabilities,
            Implementation::new("goat", env!("CARGO_PKG_VERSION")),
        )
    }

    async fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, ErrorData> {
        let (
            Some(steps),
            ElicitRequestParams::FormElicitationParams {
                message,
                requested_schema,
                ..
            },
        ) = (&self.steps, request)
        else {
            return Ok(ElicitResult::new(ElicitationAction::Decline));
        };
        let scope = if serde_json::to_value(&requested_schema)
            .is_ok_and(|schema| schema["properties"]["always"]["type"] == "boolean")
        {
            tool::Scope::Always
        } else {
            tool::Scope::Once
        };
        let (question, answer) = Question::new(message, scope);
        if steps.send(Step::Confirm(question)).is_err() {
            return Ok(ElicitResult::new(ElicitationAction::Cancel));
        }
        Ok(match answer.await {
            Ok(tool::Decision::Once | tool::Decision::Run) => {
                ElicitResult::new(ElicitationAction::Accept)
                    .with_content(serde_json::json!({ "always": false }))
            }
            Ok(tool::Decision::Always) => ElicitResult::new(ElicitationAction::Accept)
                .with_content(serde_json::json!({ "always": true })),
            Ok(tool::Decision::Refuse) => ElicitResult::new(ElicitationAction::Decline),
            Err(_) => ElicitResult::new(ElicitationAction::Cancel),
        })
    }
}

fn lifecycle() -> ClientLifecycleMode {
    ClientLifecycleMode::Auto {
        preferred_versions: vec![ProtocolVersion::LATEST],
        legacy_version: Some(ProtocolVersion::LATEST_WITH_INITIALIZE),
    }
}

async fn authorized(
    url: &str,
    account: Option<&str>,
    client: Option<&OauthClient>,
    logins: &Arc<Logins>,
) -> Result<AuthorizationManager, rmcp::transport::auth::AuthError> {
    let mut manager = AuthorizationManager::new(url).await?;
    manager.set_credential_store(auth::Saved::new(Arc::clone(logins), url, account));
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
        assert_eq!(
            config.servers[1].source,
            Source::File(workspace.join(".goat/mcp.toml"))
        );
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
    fn an_address_names_a_server_and_maybe_an_account() {
        let plain = Address::parse("linear").unwrap();
        let work = Address::parse("linear#work").unwrap();

        assert_eq!(plain.account, None);
        assert_eq!(work.name, "linear");
        assert_eq!(work.account.as_deref(), Some("work"));
        assert_eq!(work.to_string(), "linear#work");
    }

    #[test]
    fn an_address_with_a_dot_or_an_empty_account_is_rejected() {
        assert!(Address::parse("linear.issues").is_err());
        assert!(Address::parse("linear#").is_err());
        assert!(Address::parse("a#b#c").is_err());
    }

    #[test]
    fn a_server_name_outside_the_name_rule_is_a_problem() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".goat")).unwrap();
        std::fs::write(
            dir.path().join(".goat/mcp.toml"),
            "[\"my.notes\"]\nurl = \"https://x\"\n",
        )
        .unwrap();

        let config = config(dir.path(), dir.path(), variable, &[]);

        assert_eq!(config.servers, Vec::new());
        assert_eq!(
            config.problems,
            ["my.notes: a server name uses letters, digits, - and _"]
        );
    }

    #[test]
    fn sign_ins_list_the_default_and_each_account() {
        let store = store::Store::open_in_memory().unwrap();
        let logins = store.logins().unwrap();
        let server = Server {
            name: "notes".to_owned(),
            source: Source::File(PathBuf::new()),
            transport: Transport::Url {
                url: "https://notes/mcp".to_owned(),
                headers: Vec::new(),
                client: None,
            },
        };
        logins
            .save(&login_key("https://notes/mcp", None), "{}")
            .unwrap();
        logins
            .save(&login_key("https://notes/mcp", Some("work")), "{}")
            .unwrap();

        let found = sign_ins(&logins, &server).unwrap();

        assert_eq!(found, [None, Some("work".to_owned())]);
    }

    #[test]
    fn the_catalog_names_accounts_and_how_to_reach_them() {
        let addresses = [
            Address::parse("notes").unwrap(),
            Address::parse("notes#work").unwrap(),
        ];

        let text = catalog(&addresses).unwrap();

        assert!(text.starts_with("MCP servers are connected to the sandbox: notes, notes#work."));
        assert!(text.contains("`mcp[\"server#account\"].<tool>(args)`"));
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
