mod client;
mod uri;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

use gen_lsp_types::{
    Contents, Definition, DefinitionParams, DefinitionRequest, DefinitionResponse, Diagnostic,
    DocumentChange, DocumentSymbol, DocumentSymbolParams, DocumentSymbolRequest,
    DocumentSymbolResponse, Edit, HoverParams, HoverRequest, ImplementationParams,
    ImplementationRequest, ImplementationResponse, Location, LocationLink, Message,
    PartialResultParams, Position, ReferenceContext, ReferenceParams, ReferencesRequest,
    RenameParams, RenameRequest, TextDocumentIdentifier, TextDocumentPositionParams, TextEdit, Uri,
    WorkDoneProgressParams, WorkspaceEdit, WorkspaceSymbolLocation, WorkspaceSymbolParams,
    WorkspaceSymbolRequest, WorkspaceSymbolResponse,
};
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use self::client::{Client, Readiness, Settled, Sync, is_error};
use super::hashline;

const BUILTIN: &str = include_str!("lsp/servers.toml");
const PERSONAL: &str = "lsp.toml";
const MODULES: &str = "node_modules/.bin";
const DATA: &str = "{data}";
const SETTLE: Duration = Duration::from_secs(10);
const START: Duration = Duration::from_secs(60);
const ANSWER: Duration = Duration::from_secs(30);
const SHOWN_ERRORS: usize = 20;
const LISTED: usize = 200;
const PULLED: usize = 20;
const KEY_CHARS: usize = 16;
const SCANNED: usize = 20_000;

#[derive(Debug)]
pub(crate) struct Servers {
    root: PathBuf,
    workspace: PathBuf,
    data: PathBuf,
    table: Vec<Entry>,
    problems: Vec<String>,
    slots: RefCell<HashMap<Box<str>, Slot>>,
    documents: RefCell<HashMap<PathBuf, Document>>,
    changed: RefCell<Vec<PathBuf>>,
    hinted: RefCell<HashSet<String>>,
    seen: RefCell<Option<HashMap<PathBuf, Stamp>>>,
    is_shelled: Cell<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Mode {
    Push,
    Pull,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Table {
    servers: Vec<Entry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    name: String,
    command: Vec<String>,
    diagnostics: Mode,
    languages: HashMap<String, String>,
}

#[derive(Debug)]
enum Slot {
    Running(Rc<Client>),
    Missing,
    Failed(String),
}

#[derive(Debug)]
struct Document {
    server: Box<str>,
    version: i32,
    stamp: Option<Stamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    modified: SystemTime,
    len: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Definition,
    References,
    Implementations,
    Hover,
}

#[derive(Debug, Serialize)]
pub(crate) struct Place {
    path: String,
    line: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct Symbol {
    name: String,
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    line: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct Finding {
    path: String,
    line: String,
    severity: &'static str,
    message: String,
}

pub(crate) struct Target<'a> {
    pub(crate) path: &'a Path,
    pub(crate) line: &'a str,
    pub(crate) name: &'a str,
}

struct Server {
    client: Rc<Client>,
    language: String,
}

enum Served {
    Ready(Server),
    Starting(Box<str>),
    Absent(Option<String>),
}

impl Servers {
    pub(crate) fn new(workspace: &Path, home: &Path) -> Self {
        let root = workspace
            .ancestors()
            .find(|dir| dir.join(".git").exists())
            .unwrap_or(workspace)
            .to_owned();
        let mut problems = Vec::new();
        let mut table = Vec::new();
        let personal = home.join(".goat").join(PERSONAL);
        if let Ok(text) = std::fs::read_to_string(&personal) {
            match toml::from_str::<Table>(&text) {
                Ok(read) => table.extend(read.servers),
                Err(err) => problems.push(format!("{}: {}", personal.display(), err.message())),
            }
        }
        match toml::from_str::<Table>(BUILTIN) {
            Ok(builtin) => table.extend(builtin.servers),
            Err(err) => unreachable!("the built-in server table is valid: {err}"),
        }
        let digest = Sha256::digest(root.as_os_str().as_encoded_bytes());
        let key: String = digest.iter().fold(String::new(), |mut key, byte| {
            write!(key, "{byte:02x}").ok();
            key
        });
        Self {
            data: home.join(".goat").join("lsp").join(&key[..KEY_CHARS]),
            root,
            workspace: workspace.to_owned(),
            table,
            problems,
            slots: RefCell::new(HashMap::new()),
            documents: RefCell::new(HashMap::new()),
            changed: RefCell::new(Vec::new()),
            hinted: RefCell::new(HashSet::new()),
            seen: RefCell::new(None),
            is_shelled: Cell::new(false),
        }
    }

    pub(crate) fn states(&self) -> Vec<String> {
        let slots = self.slots.borrow();
        let mut states: Vec<String> = slots
            .iter()
            .filter_map(|(name, slot)| match slot {
                Slot::Running(client) => Some(match client.readiness() {
                    Readiness::Ready => format!("{name} (running)"),
                    Readiness::Starting => format!("{name} (starting)"),
                    Readiness::Failed(problem) => format!("{name} (stopped: {problem})"),
                }),
                Slot::Failed(problem) => Some(format!("{name} (could not start: {problem})")),
                Slot::Missing => None,
            })
            .collect();
        states.sort();
        states.extend(self.problems.iter().cloned());
        states
    }

    pub(crate) async fn saw(&self, path: &Path) {
        let Some(server) = self.pick(path) else {
            return;
        };
        if server.client.readiness() == Readiness::Ready && !self.is_open(path) {
            self.sync(&server, path, false).await;
        }
    }

    pub(crate) fn changed(&self, path: &Path) {
        let mut changed = self.changed.borrow_mut();
        if !changed.iter().any(|known| known == path) {
            changed.push(path.to_owned());
        }
    }

    pub(crate) fn shelling(&self) {
        self.is_shelled.set(true);
        if self.seen.borrow().is_none() {
            self.seen.replace(Some(self.scan()));
        }
    }

    pub(crate) async fn report(&self) -> Option<String> {
        if self.is_shelled.take() {
            let now = self.scan();
            if let Some(before) = self.seen.borrow().as_ref() {
                for (path, stamp) in &now {
                    if before.get(path) != Some(stamp) {
                        self.changed(path);
                    }
                }
            }
            self.seen.replace(Some(now));
        }
        let mut changed = std::mem::take(&mut *self.changed.borrow_mut());
        for (path, document) in self.documents.borrow().iter() {
            if document.stamp != stamp(path) && !changed.contains(path) {
                changed.push(path.clone());
            }
        }
        if changed.is_empty() {
            return None;
        }
        let deadline = Instant::now() + SETTLE;
        let mut groups: Vec<(Server, Vec<PathBuf>)> = Vec::new();
        let mut notes = Vec::new();
        for path in changed {
            let server = match self.serve(&path, deadline).await {
                Served::Ready(server) => server,
                Served::Starting(name) => {
                    let note = format!(
                        "{name} is still starting, so errors are checked after a later run"
                    );
                    if !notes.contains(&note) {
                        notes.push(note);
                    }
                    self.changed(&path);
                    continue;
                }
                Served::Absent(Some(problem)) => {
                    notes.push(problem);
                    continue;
                }
                Served::Absent(None) => {
                    notes.extend(self.hint(&path));
                    continue;
                }
            };
            match groups
                .iter_mut()
                .find(|(known, _)| Rc::ptr_eq(&known.client, &server.client))
            {
                Some((_, paths)) => paths.push(path),
                None => groups.push((server, vec![path])),
            }
        }
        let mut found = Vec::new();
        let mut before = 0;
        for (server, paths) in &groups {
            let client = &server.client;
            let baseline = client.errors();
            let since = Instant::now();
            let mut synced = Vec::new();
            for path in paths {
                if let Some(version) = self.sync(server, path, true).await {
                    synced.push((uri::from_path(path), version));
                }
            }
            let uris: Vec<String> = synced.iter().map(|(uri, _)| uri.clone()).collect();
            client.watched(&uris).await;
            let others = self.others(client.name(), paths);
            let syncs: Vec<Sync<'_>> = synced
                .iter()
                .map(|(uri, version)| Sync {
                    uri,
                    version: *version,
                })
                .collect();
            if client.settle(&syncs, &others, since, deadline).await == Settled::Checking {
                notes.push(format!(
                    "{} is still checking, so this list may be incomplete; run the build to be sure",
                    client.name()
                ));
            }
            let (new, old) = fresh(client.errors(), &baseline);
            before += old;
            found.extend(new);
        }
        render(&self.workspace, &found, before, &notes)
    }

    pub(crate) async fn locate(
        &self,
        target: &Target<'_>,
        action: Action,
    ) -> Result<serde_json::Value, String> {
        let (server, text) = self.open(target.path).await?;
        let position = position(&server, &text, target)?;
        let params = || TextDocumentPositionParams {
            text_document: identifier(target.path),
            position,
        };
        let client = &server.client;
        let places = match action {
            Action::Hover => {
                let hover = client
                    .request::<HoverRequest>(
                        HoverParams {
                            work_done_progress_params: WorkDoneProgressParams::default(),
                            text_document_position_params: params(),
                        },
                        ANSWER,
                    )
                    .await?;
                return Ok(serde_json::Value::String(
                    hover.map_or_default(|hover| hover_text(hover.contents)),
                ));
            }
            Action::Definition => client
                .request::<DefinitionRequest>(
                    DefinitionParams {
                        work_done_progress_params: WorkDoneProgressParams::default(),
                        partial_result_params: PartialResultParams::default(),
                        text_document_position_params: params(),
                    },
                    ANSWER,
                )
                .await?
                .map_or_default(|response| match response {
                    DefinitionResponse::Definition(found) => locations(found),
                    DefinitionResponse::DefinitionLinkList(links) => linked(links),
                }),
            Action::Implementations => client
                .request::<ImplementationRequest>(
                    ImplementationParams {
                        work_done_progress_params: WorkDoneProgressParams::default(),
                        partial_result_params: PartialResultParams::default(),
                        text_document_position_params: params(),
                    },
                    ANSWER,
                )
                .await?
                .map_or_default(|response| match response {
                    ImplementationResponse::Definition(found) => locations(found),
                    ImplementationResponse::DefinitionLinkList(links) => linked(links),
                }),
            Action::References => client
                .request::<ReferencesRequest>(
                    ReferenceParams {
                        context: ReferenceContext {
                            include_declaration: true,
                        },
                        work_done_progress_params: WorkDoneProgressParams::default(),
                        partial_result_params: PartialResultParams::default(),
                        text_document_position_params: params(),
                    },
                    ANSWER,
                )
                .await?
                .unwrap_or_default()
                .into_iter()
                .map(|location| (location.uri.0, location.range.start.line))
                .collect(),
        };
        let mut files = Files::default();
        let shown: Vec<Place> = places
            .into_iter()
            .take(LISTED)
            .filter_map(|(uri, line)| {
                let path = uri::to_path(&uri)?;
                Some(Place {
                    path: self.shown(&path),
                    line: files.tagged(&path, line),
                })
            })
            .collect();
        serde_json::to_value(shown).map_err(|err| err.to_string())
    }

    pub(crate) async fn outline(&self, path: &Path) -> Result<Vec<Symbol>, String> {
        let (server, _) = self.open(path).await?;
        let response = server
            .client
            .request::<DocumentSymbolRequest>(
                DocumentSymbolParams {
                    text_document: identifier(path),
                    work_done_progress_params: WorkDoneProgressParams::default(),
                    partial_result_params: PartialResultParams::default(),
                },
                ANSWER,
            )
            .await?;
        let mut files = Files::default();
        let mut symbols = Vec::new();
        match response {
            Some(DocumentSymbolResponse::DocumentSymbolList(found)) => {
                flatten(&found, "", path, &mut files, &mut symbols);
            }
            Some(DocumentSymbolResponse::SymbolInformationList(found)) => {
                for information in found {
                    let base = information.base_symbol_information;
                    symbols.push(Symbol {
                        name: base.name,
                        kind: kind(base.kind),
                        path: None,
                        line: files.tagged(path, information.location.range.start.line),
                    });
                }
            }
            None => {}
        }
        symbols.truncate(LISTED);
        Ok(symbols)
    }

    pub(crate) async fn search(&self, query: &str) -> Result<Vec<Symbol>, String> {
        let client = self
            .running()
            .ok_or("no language server is running yet; read a file of the project first")?;
        let response = client
            .request::<WorkspaceSymbolRequest>(
                WorkspaceSymbolParams {
                    query: query.to_owned(),
                    work_done_progress_params: WorkDoneProgressParams::default(),
                    partial_result_params: PartialResultParams::default(),
                },
                ANSWER,
            )
            .await?;
        let found: Vec<(String, gen_lsp_types::SymbolKind, String, Option<u32>)> = match response {
            Some(WorkspaceSymbolResponse::SymbolInformationList(found)) => found
                .into_iter()
                .map(|information| {
                    let base = information.base_symbol_information;
                    (
                        base.name,
                        base.kind,
                        information.location.uri.0,
                        Some(information.location.range.start.line),
                    )
                })
                .collect(),
            Some(WorkspaceSymbolResponse::WorkspaceSymbolList(found)) => found
                .into_iter()
                .map(|symbol| {
                    let base = symbol.base_symbol_information;
                    let (uri, line) = match symbol.location {
                        WorkspaceSymbolLocation::Location(location) => {
                            (location.uri.0, Some(location.range.start.line))
                        }
                        WorkspaceSymbolLocation::LocationUriOnly(only) => (only.uri.0, None),
                    };
                    (base.name, base.kind, uri, line)
                })
                .collect(),
            None => Vec::new(),
        };
        let mut files = Files::default();
        Ok(found
            .into_iter()
            .take(LISTED)
            .filter_map(|(name, symbol_kind, uri, line)| {
                let path = uri::to_path(&uri)?;
                Some(Symbol {
                    name,
                    kind: kind(symbol_kind),
                    line: line.map_or_default(|line| files.tagged(&path, line)),
                    path: Some(self.shown(&path)),
                })
            })
            .collect())
    }

    pub(crate) async fn rename(
        &self,
        target: &Target<'_>,
        new_name: &str,
    ) -> Result<Vec<(PathBuf, String)>, String> {
        let (server, text) = self.open(target.path).await?;
        let position = position(&server, &text, target)?;
        let edit = server
            .client
            .request::<RenameRequest>(
                RenameParams {
                    new_name: new_name.to_owned(),
                    work_done_progress_params: WorkDoneProgressParams::default(),
                    text_document_position_params: TextDocumentPositionParams {
                        text_document: identifier(target.path),
                        position,
                    },
                },
                ANSWER,
            )
            .await?
            .ok_or_else(|| format!("{} found nothing to rename", server.client.name()))?;
        let utf8 = server.client.is_utf8();
        edits(edit)?
            .into_iter()
            .map(|(uri, edits)| {
                let path = uri::to_path(&uri).ok_or_else(|| format!("{uri} is not a file"))?;
                let text = std::fs::read_to_string(&path)
                    .map_err(|err| format!("could not read {}: {err}", self.shown(&path)))?;
                Ok((path, apply(&text, edits, utf8)))
            })
            .collect()
    }

    pub(crate) async fn diagnostics(&self, path: Option<&Path>) -> Result<Vec<Finding>, String> {
        let Some(path) = path else {
            let mut files = Files::default();
            let clients: Vec<Rc<Client>> = self.clients();
            let mut findings: Vec<Finding> = clients
                .iter()
                .flat_map(|client| client.errors())
                .filter_map(|(uri, item)| {
                    let path = uri::to_path(&uri)?;
                    Some(self.finding(&path, &item, &mut files))
                })
                .collect();
            findings.truncate(LISTED);
            return Ok(findings);
        };
        let (server, _) = self.open(path).await?;
        let since = Instant::now();
        let version = self.sync(&server, path, true).await.unwrap_or(1);
        let uri = uri::from_path(path);
        server
            .client
            .settle(
                &[Sync { uri: &uri, version }],
                &[],
                since,
                Instant::now() + SETTLE,
            )
            .await;
        let mut files = Files::default();
        Ok(server
            .client
            .diagnostics(&uri)
            .iter()
            .take(LISTED)
            .map(|item| self.finding(path, item, &mut files))
            .collect())
    }

    fn finding(&self, path: &Path, item: &Diagnostic, files: &mut Files) -> Finding {
        Finding {
            path: self.shown(path),
            line: files.tagged(path, item.range.start.line),
            severity: severity(item),
            message: message(&item.message),
        }
    }

    async fn open(&self, path: &Path) -> Result<(Server, String), String> {
        if !path.starts_with(&self.root) {
            return Err(format!(
                "{} is outside the project, which language servers do not cover",
                self.shown(path)
            ));
        }
        let server = match self.serve(path, Instant::now() + START).await {
            Served::Ready(server) => server,
            Served::Starting(name) => {
                return Err(format!("{name} is still starting; try again shortly"));
            }
            Served::Absent(Some(problem)) => return Err(problem),
            Served::Absent(None) => {
                return Err(self.missing(path).unwrap_or_else(|| {
                    format!(
                        "no language server serves {}; goat knows none for this kind of file",
                        self.shown(path)
                    )
                }));
            }
        };
        self.sync(&server, path, false).await;
        let text = std::fs::read_to_string(path)
            .map_err(|err| format!("could not read {}: {err}", self.shown(path)))?;
        Ok((server, text))
    }

    async fn sync(&self, server: &Server, path: &Path, saves: bool) -> Option<i32> {
        let text = std::fs::read_to_string(path).ok()?;
        let uri = uri::from_path(path);
        let now = stamp(path);
        let opened = self
            .documents
            .borrow()
            .get(path)
            .map(|document| (document.version, document.stamp));
        let version = match opened {
            Some((version, before)) if before == now && !saves => return Some(version),
            Some((version, _)) => {
                server.client.change(&uri, version + 1, text).await;
                version + 1
            }
            None => {
                server.client.open(&uri, &server.language, text).await;
                1
            }
        };
        if saves {
            server.client.save(&uri).await;
        }
        self.documents.borrow_mut().insert(
            path.to_owned(),
            Document {
                server: server.client.name().into(),
                version,
                stamp: now,
            },
        );
        Some(version)
    }

    async fn serve(&self, path: &Path, deadline: Instant) -> Served {
        let mut problem = None;
        while let Some(server) = self.pick(path) {
            match server.client.ready(deadline).await {
                Ok(true) => return Served::Ready(server),
                Ok(false) => return Served::Starting(server.client.name().into()),
                Err(failed) => {
                    problem = Some(format!("{} stopped: {failed}", server.client.name()));
                }
            }
        }
        Served::Absent(problem)
    }

    fn pick(&self, path: &Path) -> Option<Server> {
        if !path.starts_with(&self.root) {
            return None;
        }
        let extension = path.extension()?.to_str()?;
        for entry in self
            .table
            .iter()
            .filter(|entry| entry.languages.contains_key(extension))
        {
            let language = entry.languages.get(extension)?.clone();
            if let Some(client) = self.client(entry) {
                return Some(Server { client, language });
            }
        }
        None
    }

    fn client(&self, entry: &Entry) -> Option<Rc<Client>> {
        let mut slots = self.slots.borrow_mut();
        let slot = slots
            .entry(entry.name.as_str().into())
            .or_insert_with(|| self.start(entry));
        if let Slot::Running(client) = slot
            && let Readiness::Failed(problem) = client.readiness()
        {
            *slot = Slot::Failed(problem);
        }
        match slot {
            Slot::Running(client) => Some(Rc::clone(client)),
            Slot::Missing | Slot::Failed(_) => None,
        }
    }

    fn start(&self, entry: &Entry) -> Slot {
        let Some((program, args)) = entry.command.split_first() else {
            return Slot::Failed("it has no command".to_owned());
        };
        let local = self.root.join(MODULES).join(program);
        let program = if local.is_file() {
            local
        } else {
            PathBuf::from(program)
        };
        let data = self.data.display().to_string();
        let args: Vec<String> = args.iter().map(|arg| arg.replace(DATA, &data)).collect();
        if entry.command.iter().any(|arg| arg.contains(DATA)) {
            std::fs::create_dir_all(&self.data).ok();
        }
        match Client::start(&entry.name, &program, &args, &self.root, entry.diagnostics) {
            Ok(client) => Slot::Running(Rc::new(client)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Slot::Missing,
            Err(err) => Slot::Failed(err.to_string()),
        }
    }

    fn missing(&self, path: &Path) -> Option<String> {
        if !path.starts_with(&self.root) {
            return None;
        }
        let extension = path.extension()?.to_str()?;
        let names: Vec<&str> = self
            .table
            .iter()
            .filter(|entry| entry.languages.contains_key(extension))
            .map(|entry| entry.name.as_str())
            .collect();
        if names.is_empty() {
            return None;
        }
        Some(format!(
            "no language server for .{extension} files is installed, so goat cannot check them; {} would",
            names.join(" or ")
        ))
    }

    fn hint(&self, path: &Path) -> Option<String> {
        let extension = path.extension()?.to_str()?.to_owned();
        let hint = self.missing(path)?;
        self.hinted.borrow_mut().insert(extension).then_some(hint)
    }

    fn scan(&self) -> HashMap<PathBuf, Stamp> {
        let served: HashSet<&str> = self
            .table
            .iter()
            .flat_map(|entry| entry.languages.keys())
            .map(String::as_str)
            .collect();
        WalkBuilder::new(&self.root)
            .build()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| served.contains(extension))
            })
            .take(SCANNED)
            .filter_map(|entry| Some((entry.path().to_owned(), stamp(entry.path())?)))
            .collect()
    }

    fn is_open(&self, path: &Path) -> bool {
        self.documents.borrow().contains_key(path)
    }

    fn others(&self, server: &str, synced: &[PathBuf]) -> Vec<String> {
        self.documents
            .borrow()
            .iter()
            .filter(|(path, document)| &*document.server == server && !synced.contains(path))
            .take(PULLED)
            .map(|(path, _)| uri::from_path(path))
            .collect()
    }

    fn running(&self) -> Option<Rc<Client>> {
        self.clients()
            .into_iter()
            .find(|client| client.readiness() == Readiness::Ready)
    }

    fn clients(&self) -> Vec<Rc<Client>> {
        self.slots
            .borrow()
            .values()
            .filter_map(|slot| match slot {
                Slot::Running(client) => Some(Rc::clone(client)),
                Slot::Missing | Slot::Failed(_) => None,
            })
            .collect()
    }

    fn shown(&self, path: &Path) -> String {
        path.strip_prefix(&self.workspace)
            .unwrap_or(path)
            .display()
            .to_string()
    }
}

#[derive(Default)]
struct Files(HashMap<PathBuf, Vec<String>>);

impl Files {
    fn tagged(&mut self, path: &Path, line: u32) -> String {
        let lines = self.0.entry(path.to_owned()).or_insert_with(|| {
            std::fs::read_to_string(path)
                .map_or_default(|text| text.lines().map(str::to_owned).collect())
        });
        let number = usize::try_from(line)
            .unwrap_or(usize::MAX)
            .saturating_add(1);
        let text = lines.get(number - 1).map_or("", String::as_str);
        hashline::tag_line(number, text)
    }
}

fn stamp(path: &Path) -> Option<Stamp> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(Stamp {
        modified: metadata.modified().ok()?,
        len: metadata.len(),
    })
}

fn identifier(path: &Path) -> TextDocumentIdentifier {
    TextDocumentIdentifier {
        uri: Uri(uri::from_path(path)),
    }
}

fn position(server: &Server, text: &str, target: &Target<'_>) -> Result<Position, String> {
    let lines: Vec<&str> = text.lines().collect();
    let number = hashline::locate(target.line, &lines)?;
    let line = lines[number - 1];
    let at = line
        .find(target.name)
        .ok_or_else(|| format!("line {number} does not contain {}", target.name))?;
    let before = &line[..at];
    let character = if server.client.is_utf8() {
        before.len()
    } else {
        before.encode_utf16().count()
    };
    Ok(Position {
        line: u32::try_from(number - 1).map_err(|err| err.to_string())?,
        character: u32::try_from(character).map_err(|err| err.to_string())?,
    })
}

fn locations(found: Definition) -> Vec<(String, u32)> {
    match found {
        Definition::Location(location) => vec![place(location)],
        Definition::LocationList(list) => list.into_iter().map(place).collect(),
    }
}

fn place(location: Location) -> (String, u32) {
    (location.uri.0, location.range.start.line)
}

fn linked(links: Vec<LocationLink>) -> Vec<(String, u32)> {
    links
        .into_iter()
        .map(|link| (link.target_uri.0, link.target_selection_range.start.line))
        .collect()
}

#[expect(
    deprecated,
    reason = "LSP 3.18 deprecates MarkedString, but servers still send it in hovers"
)]
fn hover_text(contents: Contents) -> String {
    use gen_lsp_types::MarkedString;

    let marked = |marked: MarkedString| match marked {
        MarkedString::String(text) => text,
        MarkedString::MarkedStringWithLanguage(code) => code.value,
    };
    match contents {
        Contents::MarkupContent(markup) => markup.value,
        Contents::MarkedString(one) => marked(one),
        Contents::MarkedStringList(list) => list
            .into_iter()
            .map(marked)
            .collect::<Vec<_>>()
            .join("\n\n"),
    }
}

fn flatten(
    found: &[DocumentSymbol],
    parent: &str,
    path: &Path,
    files: &mut Files,
    symbols: &mut Vec<Symbol>,
) {
    for symbol in found {
        let name = if parent.is_empty() {
            symbol.name.clone()
        } else {
            format!("{parent}.{}", symbol.name)
        };
        symbols.push(Symbol {
            name: name.clone(),
            kind: kind(symbol.kind),
            path: None,
            line: files.tagged(path, symbol.selection_range.start.line),
        });
        if let Some(children) = &symbol.children {
            flatten(children, &name, path, files, symbols);
        }
    }
}

fn kind(kind: gen_lsp_types::SymbolKind) -> String {
    format!("{kind:?}").to_lowercase()
}

fn edits(edit: WorkspaceEdit) -> Result<Vec<(String, Vec<TextEdit>)>, String> {
    if let Some(changes) = edit.document_changes {
        return changes
            .into_iter()
            .map(|change| match change {
                DocumentChange::TextDocumentEdit(document) => Ok((
                    document.text_document.text_document_identifier.uri.0,
                    document
                        .edits
                        .into_iter()
                        .filter_map(|edit| match edit {
                            Edit::TextEdit(edit) => Some(edit),
                            Edit::AnnotatedTextEdit(edit) => Some(edit.text_edit),
                            Edit::SnippetTextEdit(_) => None,
                        })
                        .collect(),
                )),
                DocumentChange::CreateFile(_)
                | DocumentChange::RenameFile(_)
                | DocumentChange::DeleteFile(_) => Err(
                    "the rename also creates, moves or deletes files, which goat does not do this way"
                        .to_owned(),
                ),
            })
            .collect();
    }
    Ok(edit
        .changes
        .unwrap_or_default()
        .into_iter()
        .map(|(uri, edits)| (uri.0, edits))
        .collect())
}

fn apply(text: &str, mut edits: Vec<TextEdit>, utf8: bool) -> String {
    let starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(at, _)| at + 1))
        .collect();
    let offset = |position: Position| {
        let line = usize::try_from(position.line).unwrap_or(usize::MAX);
        let Some(&start) = starts.get(line) else {
            return text.len();
        };
        let end = starts.get(line + 1).map_or(text.len(), |next| next - 1);
        let content = &text[start..end];
        let wanted = usize::try_from(position.character).unwrap_or(usize::MAX);
        if utf8 {
            return start + wanted.min(content.len());
        }
        let mut units = 0;
        for (at, ch) in content.char_indices() {
            if units >= wanted {
                return start + at;
            }
            units += ch.len_utf16();
        }
        end
    };
    edits
        .sort_by_key(|edit| std::cmp::Reverse((edit.range.start.line, edit.range.start.character)));
    let mut result = text.to_owned();
    for edit in edits {
        let from = offset(edit.range.start);
        let to = offset(edit.range.end).max(from);
        result.replace_range(from..to, &edit.new_text);
    }
    result
}

fn fresh(
    now: Vec<(String, Diagnostic)>,
    baseline: &[(String, Diagnostic)],
) -> (Vec<(String, Diagnostic)>, usize) {
    let key = |uri: &str, item: &Diagnostic| (uri.to_owned(), message(&item.message));
    let mut left: HashMap<(String, String), usize> = HashMap::new();
    for (uri, item) in baseline {
        *left.entry(key(uri, item)).or_default() += 1;
    }
    let mut new = Vec::new();
    let mut old = 0;
    for (uri, item) in now {
        match left.get_mut(&key(&uri, &item)) {
            Some(count) if *count > 0 => {
                *count -= 1;
                old += 1;
            }
            _ => new.push((uri, item)),
        }
    }
    (new, old)
}

fn render(
    workspace: &Path,
    found: &[(String, Diagnostic)],
    before: usize,
    notes: &[String],
) -> Option<String> {
    if found.is_empty() && notes.is_empty() {
        return None;
    }
    let mut text = String::new();
    if !found.is_empty() {
        text.push_str("Errors after this run:");
        let mut files = Files::default();
        let mut sorted: Vec<&(String, Diagnostic)> = found.iter().collect();
        sorted.sort_by(|a, b| (&a.0, a.1.range.start.line).cmp(&(&b.0, b.1.range.start.line)));
        for (uri, item) in sorted.iter().take(SHOWN_ERRORS) {
            let Some(path) = uri::to_path(uri) else {
                continue;
            };
            let shown = path
                .strip_prefix(workspace)
                .unwrap_or(&path)
                .display()
                .to_string();
            let line = files.tagged(&path, item.range.start.line);
            let first = message(&item.message);
            let first = first.lines().next().unwrap_or_default();
            write!(text, "\n{shown} {line}  {first}").ok();
        }
        if found.len() > SHOWN_ERRORS {
            write!(text, "\n… and {} more", found.len() - SHOWN_ERRORS).ok();
        }
        if before > 0 {
            write!(
                text,
                "\nNot listed: {before} that were there before this run."
            )
            .ok();
        }
    }
    for note in notes {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(note);
        text.push('.');
    }
    Some(text)
}

fn message(message: &Message) -> String {
    match message {
        Message::String(text) => text.clone(),
        Message::MarkupContent(markup) => markup.value.clone(),
    }
}

fn severity(item: &Diagnostic) -> &'static str {
    match item.severity {
        _ if is_error(item) => "error",
        Some(gen_lsp_types::DiagnosticSeverity::Warning) => "warning",
        Some(gen_lsp_types::DiagnosticSeverity::Information) => "information",
        _ => "hint",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Setup {
        _dir: tempfile::TempDir,
        work: PathBuf,
        servers: Servers,
    }

    fn setup(mode: &str) -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        let home = dir.path().join("home");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(home.join(".goat")).unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lsp/server.py");
        std::fs::write(
            home.join(".goat").join(PERSONAL),
            format!(
                "[[servers]]\nname = \"fake\"\ncommand = [\"python3\", \"{}\", \"{mode}\"]\ndiagnostics = \"{mode}\"\nlanguages = {{ fake = \"fake\" }}\n\n[[servers]]\nname = \"ghost\"\ncommand = [\"goat-no-such-server\"]\ndiagnostics = \"push\"\nlanguages = {{ ghost = \"ghost\" }}\n",
                fixture.display()
            ),
        )
        .unwrap();
        let servers = Servers::new(&work, &home);
        Setup {
            _dir: dir,
            work,
            servers,
        }
    }

    async fn errors_after_an_edit(mode: &str) -> Option<String> {
        let setup = setup(mode);
        let path = setup.work.join("a.fake");
        std::fs::write(&path, "def ok\nERROR old\n").unwrap();
        setup.servers.diagnostics(Some(&path)).await.unwrap();
        std::fs::write(&path, "def ok\nERROR old\nERROR new\n").unwrap();
        setup.servers.changed(&path);

        setup.servers.report().await
    }

    fn expected_errors() -> String {
        format!(
            "Errors after this run:\na.fake {}  bad line ERROR new\nNot listed: 1 that were there before this run.",
            hashline::tag_line(3, "ERROR new")
        )
    }

    #[tokio::test]
    async fn new_errors_a_pushing_server_finds_come_with_their_anchors() {
        assert_eq!(errors_after_an_edit("push").await, Some(expected_errors()));
    }

    #[tokio::test]
    async fn new_errors_a_pulling_server_finds_come_with_their_anchors() {
        assert_eq!(errors_after_an_edit("pull").await, Some(expected_errors()));
    }

    #[tokio::test]
    async fn a_run_without_changes_reports_nothing() {
        let setup = setup("push");

        assert_eq!(setup.servers.report().await, None);
    }

    #[tokio::test]
    async fn a_definition_comes_back_as_an_anchored_line() {
        let setup = setup("push");
        let path = setup.work.join("a.fake");
        std::fs::write(&path, "def greet\ncall greet\n").unwrap();
        let target = Target {
            path: &path,
            line: "2",
            name: "greet",
        };

        let found = setup
            .servers
            .locate(&target, Action::Definition)
            .await
            .unwrap();

        assert_eq!(
            found,
            serde_json::json!([{ "path": "a.fake", "line": hashline::tag_line(1, "def greet") }])
        );
    }

    #[tokio::test]
    async fn a_rename_returns_each_file_with_its_new_text() {
        let setup = setup("push");
        let path = setup.work.join("a.fake");
        std::fs::write(&path, "def greet\ncall greet\n").unwrap();
        let target = Target {
            path: &path,
            line: &hashline::tag_line(1, "def greet"),
            name: "greet",
        };

        let renamed = setup.servers.rename(&target, "hello").await.unwrap();

        assert_eq!(renamed, [(path, "def hello\ncall hello\n".to_owned())]);
    }

    #[tokio::test]
    async fn an_outline_lists_the_symbols_of_a_file() {
        let setup = setup("push");
        let path = setup.work.join("a.fake");
        std::fs::write(&path, "def greet\ncall greet\ndef part\n").unwrap();

        let symbols = setup.servers.outline(&path).await.unwrap();

        let names: Vec<(&str, &str)> = symbols
            .iter()
            .map(|symbol| (symbol.name.as_str(), symbol.kind.as_str()))
            .collect();
        assert_eq!(names, [("greet", "function"), ("part", "function")]);
    }

    #[tokio::test]
    async fn a_server_that_fails_to_start_gives_way_to_the_next_one() {
        let setup = setup("push");
        let home = setup.work.parent().unwrap().join("home");
        let personal = home.join(".goat").join(PERSONAL);
        let table = std::fs::read_to_string(&personal).unwrap();
        std::fs::write(
            &personal,
            format!(
                "[[servers]]\nname = \"broken\"\ncommand = [\"python3\", \"-c\", \"pass\"]\ndiagnostics = \"push\"\nlanguages = {{ fake = \"fake\" }}\n\n{table}"
            ),
        )
        .unwrap();
        let servers = Servers::new(&setup.work, &home);
        let path = setup.work.join("a.fake");
        std::fs::write(&path, "def greet\ncall greet\n").unwrap();
        let target = Target {
            path: &path,
            line: "2",
            name: "greet",
        };

        let found = servers.locate(&target, Action::Definition).await.unwrap();

        assert_eq!(found[0]["line"], hashline::tag_line(1, "def greet"));
        assert!(
            servers
                .states()
                .iter()
                .any(|state| state.starts_with("broken (could not start"))
        );
    }

    #[tokio::test]
    async fn a_missing_server_is_mentioned_once() {
        let setup = setup("push");
        let path = setup.work.join("a.ghost");
        std::fs::write(&path, "x\n").unwrap();

        setup.servers.changed(&path);
        let first = setup.servers.report().await;
        setup.servers.changed(&path);
        let second = setup.servers.report().await;

        assert_eq!(
            first.as_deref(),
            Some(
                "no language server for .ghost files is installed, so goat cannot check them; ghost would."
            )
        );
        assert_eq!(second, None);
    }

    async fn real(files: &[(&str, &str)], edited: &str, broken: &str) -> String {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        for (name, text) in files {
            let path = work.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        if work.join("package.json").exists() {
            let installed = std::process::Command::new("npm")
                .args(["install", "--silent", "--no-audit", "--no-fund"])
                .current_dir(&work)
                .status()
                .unwrap();
            assert!(installed.success());
        }
        let servers = Servers::new(&work, &dir.path().join("home"));
        let path = work.join(edited);
        servers.diagnostics(Some(&path)).await.unwrap();
        std::fs::write(&path, broken).unwrap();
        servers.changed(&path);
        servers.report().await.unwrap_or_default()
    }

    #[tokio::test]
    #[ignore = "needs rust-analyzer"]
    async fn the_real_rust_analyzer_reports_a_type_error() {
        let report = real(
            &[
                (
                    "Cargo.toml",
                    "[package]\nname = \"m\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
                ),
                (
                    "src/main.rs",
                    "fn main() {\n    let x: u32 = 1;\n    println!(\"{x}\");\n}\n",
                ),
            ],
            "src/main.rs",
            "fn main() {\n    let x: u32 = \"no\";\n    println!(\"{x}\");\n}\n",
        )
        .await;

        assert!(
            report.starts_with("Errors after this run:\nsrc/main.rs 2:"),
            "{report}"
        );
    }

    #[tokio::test]
    #[ignore = "needs gopls"]
    async fn the_real_gopls_reports_a_type_error() {
        let report = real(
            &[
                ("go.mod", "module m\n\ngo 1.22\n"),
                (
                    "main.go",
                    "package main\n\nfunc main() {\n\tvar x int = 1\n\t_ = x\n}\n",
                ),
            ],
            "main.go",
            "package main\n\nfunc main() {\n\tvar x int = \"no\"\n\t_ = x\n}\n",
        )
        .await;

        assert!(
            report.starts_with("Errors after this run:\nmain.go 4:"),
            "{report}"
        );
    }

    #[tokio::test]
    #[ignore = "needs basedpyright or pyright"]
    async fn the_real_pyright_reports_a_type_error() {
        let report = real(&[("a.py", "x: int = 1\n")], "a.py", "x: int = \"no\"\n").await;

        assert!(
            report.starts_with("Errors after this run:\na.py 1:"),
            "{report}"
        );
    }

    #[tokio::test]
    #[ignore = "needs typescript-language-server, npm and the network"]
    async fn the_real_typescript_server_reports_a_type_error() {
        let report = real(
            &[
                (
                    "package.json",
                    "{ \"private\": true, \"devDependencies\": { \"typescript\": \"5.9.3\" } }\n",
                ),
                (
                    "tsconfig.json",
                    "{ \"compilerOptions\": { \"strict\": true } }\n",
                ),
                ("a.ts", "const x: number = 1;\nconsole.log(x);\n"),
            ],
            "a.ts",
            "const x: number = \"no\";\nconsole.log(x);\n",
        )
        .await;

        assert!(
            report.starts_with("Errors after this run:\na.ts 1:"),
            "{report}"
        );
    }

    #[tokio::test]
    #[ignore = "needs clangd"]
    async fn the_real_clangd_reports_a_type_error() {
        let report = real(
            &[
                ("compile_flags.txt", "-xc\n"),
                (
                    "a.c",
                    "int main(void) {\n    int x = 1;\n    return x;\n}\n",
                ),
            ],
            "a.c",
            "int main(void) {\n    int x = 1;\n    return y;\n}\n",
        )
        .await;

        assert!(
            report.starts_with("Errors after this run:\na.c 3:"),
            "{report}"
        );
    }

    #[tokio::test]
    #[ignore = "needs sourcekit-lsp"]
    async fn the_real_sourcekit_lsp_reports_a_type_error() {
        let report = real(
            &[
                ("Package.swift", "// swift-tools-version:5.9\nimport PackageDescription\nlet package = Package(name: \"m\", targets: [.executableTarget(name: \"m\")])\n"),
                ("Sources/m/main.swift", "let x: Int = 1\nprint(x)\n"),
            ],
            "Sources/m/main.swift",
            "let x: Int = \"no\"\nprint(x)\n",
        )
        .await;

        assert!(
            report.starts_with("Errors after this run:\nSources/m/main.swift 1:"),
            "{report}"
        );
    }

    #[test]
    fn edits_apply_from_the_end_in_utf16_columns() {
        let edit = |line, from, to, text: &str| TextEdit {
            range: gen_lsp_types::Range {
                start: Position {
                    line,
                    character: from,
                },
                end: Position {
                    line,
                    character: to,
                },
            },
            new_text: text.to_owned(),
        };

        let changed = apply(
            "let 한 = a;\nuse a;\n",
            vec![edit(0, 8, 9, "b"), edit(1, 4, 5, "b")],
            false,
        );

        assert_eq!(changed, "let 한 = b;\nuse b;\n");
    }
}
