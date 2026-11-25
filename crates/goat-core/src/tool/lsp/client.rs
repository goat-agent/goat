use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use gen_lsp_types::{
    Diagnostic, DidChangeTextDocumentNotification, DidChangeTextDocumentParams,
    DidChangeWatchedFilesNotification, DidChangeWatchedFilesParams,
    DidOpenTextDocumentNotification, DidOpenTextDocumentParams, DidSaveTextDocumentNotification,
    DidSaveTextDocumentParams, DocumentDiagnosticParams, DocumentDiagnosticReport,
    DocumentDiagnosticRequest, FileChangeType, FileEvent, InitializeResult, LanguageKind,
    Notification, PositionEncodingKind, PublishDiagnosticsParams, Request,
    TextDocumentContentChangeEvent, TextDocumentContentChangeWholeDocument, TextDocumentIdentifier,
    TextDocumentItem, Uri, VersionedTextDocumentIdentifier,
};
use rustix::process::{Pid, Signal, kill_process_group};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::task::JoinHandle;

use super::{Mode, uri};

const OUTGOING: usize = 64;
const INITIALIZE: i64 = 0;
const QUIET: Duration = Duration::from_millis(300);
const NOTHING: Duration = Duration::from_millis(1500);
const REGISTRATION: Duration = Duration::from_secs(2);
const PULL: Duration = Duration::from_secs(5);
const TICK: Duration = Duration::from_millis(50);
const WARMUP: Duration = Duration::from_secs(30);
const HEADER_BYTES: usize = 64 * 1024;
const BODY_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug)]
pub(super) struct Client {
    name: Box<str>,
    mode: Mode,
    outgoing: mpsc::Sender<Vec<u8>>,
    shared: Arc<Shared>,
    next: AtomicI64,
    started: Instant,
    group: Option<Pid>,
    tasks: [JoinHandle<()>; 2],
    _child: Child,
}

#[derive(Debug, Default)]
struct Shared {
    state: Mutex<State>,
    changed: Notify,
}

#[derive(Debug, Default)]
struct State {
    readiness: Readiness,
    utf8: bool,
    pulls: bool,
    published: HashMap<String, Published>,
    progress: HashMap<String, Instant>,
    refreshes: u64,
    last: Option<Instant>,
    waiting: HashMap<i64, oneshot::Sender<Result<Value, String>>>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) enum Readiness {
    #[default]
    Starting,
    Ready,
    Failed(String),
}

#[derive(Debug)]
struct Published {
    version: Option<i32>,
    at: Instant,
    items: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Settled {
    Done,
    Checking,
}

pub(super) struct Sync<'a> {
    pub(super) uri: &'a str,
    pub(super) version: i32,
}

impl Client {
    pub(super) fn start(
        name: &str,
        program: &Path,
        args: &[String],
        root: &Path,
        mode: Mode,
    ) -> std::io::Result<Self> {
        let mut child = Command::new(program)
            .args(args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()?;
        let group = child
            .id()
            .and_then(|id| i32::try_from(id).ok())
            .and_then(Pid::from_raw);
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(std::io::Error::other("the server has no stdio"));
        };
        let (outgoing, queue) = mpsc::channel(OUTGOING);
        let shared = Arc::new(Shared::default());
        let root_uri = uri::from_path(root);
        let reader = tokio::spawn(read(
            stdout,
            Arc::clone(&shared),
            outgoing.clone(),
            root_uri.clone(),
        ));
        let writer = tokio::spawn(write(stdin, queue));
        let client = Self {
            name: name.into(),
            mode,
            outgoing,
            shared,
            next: AtomicI64::new(INITIALIZE + 1),
            started: Instant::now(),
            group,
            tasks: [reader, writer],
            _child: child,
        };
        client.queue(&frame(&json!({
            "jsonrpc": "2.0",
            "id": INITIALIZE,
            "method": "initialize",
            "params": initialize(&root_uri, root, mode),
        })))?;
        Ok(client)
    }

    pub(super) fn name(&self) -> &str {
        &self.name
    }

    pub(super) fn readiness(&self) -> Readiness {
        self.lock().readiness.clone()
    }

    pub(super) fn is_utf8(&self) -> bool {
        self.lock().utf8
    }

    pub(super) async fn ready(&self, deadline: Instant) -> Result<bool, String> {
        loop {
            match self.readiness() {
                Readiness::Ready => return Ok(true),
                Readiness::Failed(problem) => return Err(problem),
                Readiness::Starting if Instant::now() >= deadline => return Ok(false),
                Readiness::Starting => self.pause(deadline).await,
            }
        }
    }

    pub(super) async fn request<R: Request>(
        &self,
        params: R::Params,
        timeout: Duration,
    ) -> Result<R::Result, String> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.lock().waiting.insert(id, tx);
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": R::METHOD.as_str(),
            "params": params,
        });
        if self.outgoing.send(frame(&message)).await.is_err() {
            self.lock().waiting.remove(&id);
            return Err(format!("{} stopped", self.name));
        }
        let reply = match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => Err(format!("{} stopped", self.name)),
            Err(_) => {
                self.lock().waiting.remove(&id);
                Err(format!("{} did not answer in time", self.name))
            }
        }?;
        serde_json::from_value(reply)
            .map_err(|err| format!("{} gave an answer goat cannot read: {err}", self.name))
    }

    pub(super) async fn notify<N: Notification>(&self, params: N::Params) {
        let message = json!({
            "jsonrpc": "2.0",
            "method": N::METHOD.as_str(),
            "params": params,
        });
        self.outgoing.send(frame(&message)).await.ok();
    }

    pub(super) async fn open(&self, uri: &str, language: &str, text: String) {
        self.notify::<DidOpenTextDocumentNotification>(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: Uri(uri.to_owned()),
                language_id: LanguageKind::from(language.to_owned()),
                version: 1,
                text,
            },
        })
        .await;
    }

    pub(super) async fn change(&self, uri: &str, version: i32, text: String) {
        self.notify::<DidChangeTextDocumentNotification>(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                version,
                text_document_identifier: TextDocumentIdentifier {
                    uri: Uri(uri.to_owned()),
                },
            },
            content_changes: vec![TextDocumentContentChangeEvent::from(
                TextDocumentContentChangeWholeDocument { text },
            )],
        })
        .await;
    }

    pub(super) async fn save(&self, uri: &str) {
        self.notify::<DidSaveTextDocumentNotification>(DidSaveTextDocumentParams {
            text_document: TextDocumentIdentifier {
                uri: Uri(uri.to_owned()),
            },
            text: None,
        })
        .await;
    }

    pub(super) async fn watched(&self, uris: &[String]) {
        let changes = uris
            .iter()
            .map(|uri| FileEvent {
                uri: Uri(uri.clone()),
                kind: FileChangeType::Changed,
            })
            .collect();
        self.notify::<DidChangeWatchedFilesNotification>(DidChangeWatchedFilesParams { changes })
            .await;
    }

    pub(super) fn errors(&self) -> Vec<(String, Diagnostic)> {
        self.lock()
            .published
            .iter()
            .flat_map(|(uri, published)| {
                published
                    .items
                    .iter()
                    .filter(|item| is_error(item))
                    .map(|item| (uri.clone(), item.clone()))
            })
            .collect()
    }

    pub(super) fn diagnostics(&self, uri: &str) -> Vec<Diagnostic> {
        self.lock()
            .published
            .get(uri)
            .map_or_default(|published| published.items.clone())
    }

    pub(super) async fn settle(
        &self,
        synced: &[Sync<'_>],
        others: &[String],
        since: Instant,
        deadline: Instant,
    ) -> Settled {
        if self.mode == Mode::Pull && self.pulls(deadline.min(since + REGISTRATION)).await {
            loop {
                let seen = self.lock().refreshes;
                self.idle(deadline).await;
                let uris = synced
                    .iter()
                    .map(|sync| sync.uri)
                    .chain(others.iter().map(String::as_str));
                for uri in uris {
                    self.pull(uri, deadline).await;
                }
                let is_warm = seen > 0 || since.duration_since(self.started) >= WARMUP;
                let until = deadline.min(Instant::now() + NOTHING);
                if is_warm || !self.refreshed(seen, until).await {
                    return Settled::Done;
                }
            }
        }
        self.wait(synced, since, deadline).await
    }

    async fn pulls(&self, until: Instant) -> bool {
        loop {
            if self.lock().pulls {
                return true;
            }
            if Instant::now() >= until {
                return false;
            }
            self.pause(until).await;
        }
    }

    async fn refreshed(&self, seen: u64, until: Instant) -> bool {
        loop {
            if self.lock().refreshes > seen {
                return true;
            }
            if Instant::now() >= until {
                return false;
            }
            self.pause(until).await;
        }
    }

    async fn idle(&self, deadline: Instant) {
        loop {
            let now = Instant::now();
            let is_idle = {
                let state = self.lock();
                state.progress.is_empty()
                    && state
                        .last
                        .is_none_or(|last| now.duration_since(last) >= QUIET)
            };
            if is_idle || now >= deadline {
                return;
            }
            self.pause(deadline).await;
        }
    }

    async fn pull(&self, uri: &str, deadline: Instant) {
        let params = DocumentDiagnosticParams {
            text_document: TextDocumentIdentifier {
                uri: Uri(uri.to_owned()),
            },
            identifier: None,
            previous_result_id: None,
            work_done_progress_params: gen_lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: gen_lsp_types::PartialResultParams::default(),
        };
        let timeout = deadline
            .saturating_duration_since(Instant::now())
            .min(PULL)
            .max(TICK);
        let Ok(report) = self
            .request::<DocumentDiagnosticRequest>(params, timeout)
            .await
        else {
            return;
        };
        let DocumentDiagnosticReport::RelatedFullDocumentDiagnosticReport(full) = report else {
            return;
        };
        self.lock().published.insert(
            uri.to_owned(),
            Published {
                version: None,
                at: Instant::now(),
                items: full.full_document_diagnostic_report.items,
            },
        );
    }

    async fn wait(&self, synced: &[Sync<'_>], since: Instant, deadline: Instant) -> Settled {
        loop {
            let now = Instant::now();
            let (is_published, is_busy, is_quiet, is_silent) = {
                let state = self.lock();
                let is_published = synced.iter().all(|sync| {
                    state.published.get(sync.uri).is_some_and(|published| {
                        published.at >= since
                            && published
                                .version
                                .is_none_or(|version| version >= sync.version)
                    })
                });
                let is_busy = state.progress.values().any(|begun| *begun >= since);
                let last = state.last.filter(|last| *last >= since);
                let is_quiet = last.is_none_or(|last| now.duration_since(last) >= QUIET);
                let is_silent = last.is_none() && now.duration_since(since) >= NOTHING;
                (is_published, is_busy, is_quiet, is_silent)
            };
            if (is_published && !is_busy && is_quiet) || is_silent {
                return Settled::Done;
            }
            if now >= deadline {
                return Settled::Checking;
            }
            self.pause(deadline).await;
        }
    }

    async fn pause(&self, deadline: Instant) {
        let wait = deadline.saturating_duration_since(Instant::now()).min(TICK);
        tokio::time::timeout(wait, self.shared.changed.notified())
            .await
            .ok();
    }

    fn queue(&self, bytes: &[u8]) -> std::io::Result<()> {
        self.outgoing
            .try_send(bytes.to_vec())
            .map_err(|_| std::io::Error::other("the server stopped reading"))
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.shared.lock()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        if let Some(group) = self.group {
            kill_process_group(group, Signal::KILL).ok();
        }
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

pub(super) fn is_error(item: &Diagnostic) -> bool {
    item.severity
        .is_none_or(|severity| severity == gen_lsp_types::DiagnosticSeverity::Error)
}

fn initialize(root_uri: &str, root: &Path, mode: Mode) -> Value {
    let mut text_document = json!({
        "synchronization": { "didSave": true },
        "publishDiagnostics": { "versionSupport": true },
        "definition": { "linkSupport": true },
        "implementation": { "linkSupport": true },
        "references": {},
        "hover": { "contentFormat": ["plaintext", "markdown"] },
        "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
        "rename": {},
    });
    if mode == Mode::Pull {
        text_document["diagnostic"] = json!({ "dynamicRegistration": true });
    }
    let name = root
        .file_name()
        .map_or_default(|name| name.to_string_lossy().into_owned());
    json!({
        "processId": std::process::id(),
        "clientInfo": { "name": "goat" },
        "rootUri": root_uri,
        "workspaceFolders": [{ "uri": root_uri, "name": name }],
        "capabilities": {
            "general": { "positionEncodings": ["utf-8", "utf-16"] },
            "window": { "workDoneProgress": true },
            "workspace": {
                "configuration": true,
                "workspaceFolders": true,
                "didChangeWatchedFiles": { "dynamicRegistration": true },
                "diagnostics": { "refreshSupport": true },
            },
            "textDocument": text_document,
        },
    })
}

fn frame(message: &Value) -> Vec<u8> {
    let body = message.to_string();
    let mut bytes = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    bytes.extend_from_slice(body.as_bytes());
    bytes
}

async fn write(mut stdin: ChildStdin, mut queue: mpsc::Receiver<Vec<u8>>) {
    while let Some(bytes) = queue.recv().await {
        if stdin.write_all(&bytes).await.is_err() || stdin.flush().await.is_err() {
            return;
        }
    }
}

async fn read(
    stdout: ChildStdout,
    shared: Arc<Shared>,
    outgoing: mpsc::Sender<Vec<u8>>,
    root_uri: String,
) {
    let mut stdout = BufReader::new(stdout);
    while let Some(message) = next(&mut stdout).await {
        let reply = handle(&shared, &message, &root_uri);
        if let Some(reply) = reply {
            outgoing.send(frame(&reply)).await.ok();
        }
        shared.lock().last = Some(Instant::now());
        shared.changed.notify_waiters();
    }
    let mut state = shared.lock();
    if state.readiness == Readiness::Starting {
        state.readiness = Readiness::Failed("the server exited while starting".to_owned());
    }
    state.waiting.clear();
    drop(state);
    shared.changed.notify_waiters();
}

async fn next(stdout: &mut BufReader<ChildStdout>) -> Option<Value> {
    let mut length = None;
    let mut seen = 0;
    loop {
        let mut line = String::new();
        let read = stdout.read_line(&mut line).await.ok()?;
        seen += read;
        if read == 0 || seen > HEADER_BYTES {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let length = length.filter(|length| *length <= BODY_BYTES)?;
    let mut body = vec![0; length];
    stdout.read_exact(&mut body).await.ok()?;
    serde_json::from_slice(&body).ok()
}

fn handle(shared: &Shared, message: &Value, root_uri: &str) -> Option<Value> {
    let method = message.get("method").and_then(Value::as_str);
    let id = message.get("id").cloned();
    match (method, id) {
        (None, Some(id)) => answered(shared, &id, message)
            .then(|| json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} })),
        (Some(method), Some(id)) => {
            let result = asked(shared, method, message.get("params"), root_uri);
            Some(match result {
                Some(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                None => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": format!("goat does not handle {method}") },
                }),
            })
        }
        (Some(method), None) => {
            told(shared, method, message.get("params"));
            None
        }
        (None, None) => None,
    }
}

fn answered(shared: &Shared, id: &Value, message: &Value) -> bool {
    let Some(id) = id.as_i64() else {
        return false;
    };
    let outcome = match (message.get("result"), message.get("error")) {
        (_, Some(error)) => Err(error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("the server reported an error")
            .to_owned()),
        (result, None) => Ok(result.cloned().unwrap_or(Value::Null)),
    };
    let mut state = shared.lock();
    if id == INITIALIZE {
        match outcome.and_then(|value| {
            serde_json::from_value::<InitializeResult>(value).map_err(|err| err.to_string())
        }) {
            Ok(result) => {
                state.utf8 =
                    result.capabilities.position_encoding == Some(PositionEncodingKind::UTF8);
                state.pulls = result.capabilities.diagnostic_provider.is_some();
                state.readiness = Readiness::Ready;
                return true;
            }
            Err(problem) => state.readiness = Readiness::Failed(problem),
        }
        return false;
    }
    if let Some(waiting) = state.waiting.remove(&id) {
        waiting.send(outcome).ok();
    }
    false
}

fn asked(shared: &Shared, method: &str, params: Option<&Value>, root_uri: &str) -> Option<Value> {
    match method {
        "workspace/configuration" => {
            let count = params
                .and_then(|params| params.get("items"))
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            Some(Value::Array(vec![Value::Null; count]))
        }
        "client/registerCapability" => {
            let registers_pull = params
                .and_then(|params| params.get("registrations"))
                .and_then(Value::as_array)
                .is_some_and(|registrations| {
                    registrations.iter().any(|registration| {
                        registration.get("method").and_then(Value::as_str)
                            == Some("textDocument/diagnostic")
                    })
                });
            if registers_pull {
                shared.lock().pulls = true;
            }
            Some(Value::Null)
        }
        "workspace/workspaceFolders" => Some(json!([{ "uri": root_uri, "name": "workspace" }])),
        "workspace/diagnostic/refresh" => {
            shared.lock().refreshes += 1;
            Some(Value::Null)
        }
        "workspace/applyEdit" => Some(json!({ "applied": false })),
        "window/workDoneProgress/create"
        | "client/unregisterCapability"
        | "window/showMessageRequest"
        | "window/showDocument"
        | "workspace/semanticTokens/refresh"
        | "workspace/inlayHint/refresh"
        | "workspace/inlineValue/refresh"
        | "workspace/codeLens/refresh"
        | "workspace/foldingRange/refresh" => Some(Value::Null),
        _ => None,
    }
}

fn told(shared: &Shared, method: &str, params: Option<&Value>) {
    let Some(params) = params else {
        return;
    };
    match method {
        "textDocument/publishDiagnostics" => {
            let Ok(published) = serde_json::from_value::<PublishDiagnosticsParams>(params.clone())
            else {
                return;
            };
            shared.lock().published.insert(
                published.uri.0,
                Published {
                    version: published.version,
                    at: Instant::now(),
                    items: published.diagnostics,
                },
            );
        }
        "$/progress" => {
            let token = match params.get("token") {
                Some(Value::String(token)) => token.clone(),
                Some(token) => token.to_string(),
                None => return,
            };
            let kind = params
                .get("value")
                .and_then(|value| value.get("kind"))
                .and_then(Value::as_str);
            let mut state = shared.lock();
            match kind {
                Some("begin") => {
                    state.progress.insert(token, Instant::now());
                }
                Some("end") => {
                    state.progress.remove(&token);
                }
                _ => {}
            }
        }
        _ => {}
    }
}
