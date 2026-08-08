use std::{collections::HashMap, fmt::Write as _, path::Path, process::Stdio, sync::Arc};

use goat_protocol::{Event, ProcessExitReason, ProcessInfo, ProcessState, RunId};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::{Mutex, Notify, mpsc},
};
use tokio_util::sync::CancellationToken;

const RING_CAPACITY: usize = 2000;
const MAX_LIVE_PROCESSES: usize = 16;
const WATCH_FLOOD_LINES: usize = 500;

struct Line {
    stream: Stream,
    text: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stream {
    Out,
    Err,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Process,
    Child,
}

struct ProcessDetail {
    db_id: Option<i64>,
    lines: std::collections::VecDeque<Line>,
    dropped: usize,
    seen_cursor: usize,
    exit_code: Option<i32>,
    watched: bool,
    watch_flooded: bool,
    stdin: Option<tokio::process::ChildStdin>,
    kill_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

struct ChildDetail {
    report: Option<Result<String, String>>,
    cancel: CancellationToken,
}

enum Detail {
    Process(ProcessDetail),
    Child(ChildDetail),
}

struct Entry {
    label: String,
    title: String,
    state: ProcessState,
    update_sent: bool,
    kill_pending: bool,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    detail: Detail,
}

impl Entry {
    fn kind(&self) -> Kind {
        match self.detail {
            Detail::Process(_) => Kind::Process,
            Detail::Child(_) => Kind::Child,
        }
    }

    fn process(&self) -> Option<&ProcessDetail> {
        match &self.detail {
            Detail::Process(process) => Some(process),
            Detail::Child(_) => None,
        }
    }

    fn process_mut(&mut self) -> Option<&mut ProcessDetail> {
        match &mut self.detail {
            Detail::Process(process) => Some(process),
            Detail::Child(_) => None,
        }
    }

    fn abort_tasks(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }

    fn watched(&self) -> bool {
        self.process().is_some_and(|process| process.watched)
    }

    fn info(&self, id: RunId) -> ProcessInfo {
        ProcessInfo {
            id,
            command: self.title.clone(),
            state: self.state,
            watched: self.watched(),
            exit_code: self.process().and_then(|process| process.exit_code),
        }
    }

    fn run_info(&self, id: RunId) -> RunInfo {
        RunInfo {
            id,
            label: self.label.clone(),
            title: self.title.clone(),
            watched: self.watched(),
        }
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        self.abort_tasks();
    }
}

struct Inner {
    entries: HashMap<RunId, Entry>,
    next_id: u64,
}

pub(crate) struct Runs {
    inner: Mutex<Inner>,
    events: mpsc::Sender<Event>,
    wake: Arc<Notify>,
    store: Option<goat_code_store::CodeStore>,
}

#[derive(Clone, Copy)]
pub(crate) enum WatchMode {
    CompletionOnly,
    OutputAndCompletion,
}

#[derive(Debug)]
pub(crate) enum SpawnError {
    TooMany,
    Spawn(String),
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooMany => write!(
                f,
                "too many background runs are already going (limit {MAX_LIVE_PROCESSES}); stop one first"
            ),
            Self::Spawn(msg) => write!(f, "failed to start process: {msg}"),
        }
    }
}

pub(crate) struct Started {
    pub(crate) id: RunId,
    pub(crate) pgid: Option<i32>,
}

pub(crate) struct RunInfo {
    pub(crate) id: RunId,
    pub(crate) label: String,
    pub(crate) title: String,
    pub(crate) watched: bool,
}

impl Runs {
    pub(crate) fn new(
        events: mpsc::Sender<Event>,
        wake: Arc<Notify>,
        store: Option<goat_code_store::CodeStore>,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                entries: HashMap::new(),
                next_id: 1,
            }),
            events,
            wake,
            store,
        })
    }

    #[cfg(test)]
    pub(crate) async fn spawn(
        self: &Arc<Self>,
        command: &str,
        name: Option<&str>,
        cwd: &Path,
        watch_mode: WatchMode,
    ) -> Result<Started, SpawnError> {
        self.spawn_labeled(command, name, cwd, watch_mode, "process")
            .await
    }

    pub(crate) async fn spawn_labeled(
        self: &Arc<Self>,
        command: &str,
        name: Option<&str>,
        cwd: &Path,
        watch_mode: WatchMode,
        label: &str,
    ) -> Result<Started, SpawnError> {
        let watched = matches!(watch_mode, WatchMode::OutputAndCompletion);
        let id = {
            let inner = self.inner.lock().await;
            let live = inner
                .entries
                .values()
                .filter(|e| {
                    e.state == ProcessState::Running && matches!(e.detail, Detail::Process(_))
                })
                .count();
            if live >= MAX_LIVE_PROCESSES {
                return Err(SpawnError::TooMany);
            }
            RunId(inner.next_id)
        };

        let mut builder = shell_command(command);
        builder
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        set_process_group(&mut builder);

        let mut child = builder
            .spawn()
            .map_err(|err| SpawnError::Spawn(err.to_string()))?;

        let pgid = child.id().and_then(|pid| i32::try_from(pid).ok());

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stdin = child.stdin.take();

        let mut tasks = Vec::with_capacity(3);
        if let Some(pipe) = stdout {
            tasks.push(self.spawn_reader(id, pipe, Stream::Out));
        }
        if let Some(pipe) = stderr {
            tasks.push(self.spawn_reader(id, pipe, Stream::Err));
        }
        let (kill_tx, kill_rx) = tokio::sync::oneshot::channel();
        tasks.push(self.spawn_waiter(id, child, pgid, kill_rx));

        {
            let mut inner = self.inner.lock().await;
            inner.next_id += 1;
            inner.entries.insert(
                id,
                Entry {
                    label: label.to_owned(),
                    title: name.unwrap_or(command).to_owned(),
                    state: ProcessState::Running,
                    update_sent: false,
                    kill_pending: false,
                    tasks,
                    detail: Detail::Process(ProcessDetail {
                        db_id: None,
                        lines: std::collections::VecDeque::new(),
                        dropped: 0,
                        seen_cursor: 0,
                        exit_code: None,
                        watched,
                        watch_flooded: false,
                        stdin,
                        kill_tx: Some(kill_tx),
                    }),
                },
            );
        }

        let _ = self
            .events
            .send(Event::ProcessStarted {
                process: id,
                command: command.to_owned(),
                watched,
            })
            .await;
        self.broadcast_list().await;

        Ok(Started { id, pgid })
    }

    #[cfg(test)]
    pub(crate) async fn register_child(&self, name: &str, cancel: CancellationToken) -> RunId {
        self.register_child_labeled(name, cancel, "subagent").await
    }

    pub(crate) async fn register_child_labeled(
        &self,
        name: &str,
        cancel: CancellationToken,
        label: &str,
    ) -> RunId {
        let mut inner = self.inner.lock().await;
        let id = RunId(inner.next_id);
        inner.next_id += 1;
        let title = name.to_owned();
        inner.entries.insert(
            id,
            Entry {
                label: label.to_owned(),
                title,
                state: ProcessState::Running,
                update_sent: false,
                kill_pending: false,
                tasks: Vec::new(),
                detail: Detail::Child(ChildDetail {
                    report: None,
                    cancel,
                }),
            },
        );
        id
    }

    pub(crate) async fn finish_child(&self, id: RunId, result: Result<String, String>) {
        let wake = {
            let mut inner = self.inner.lock().await;
            let Some(entry) = inner.entries.get_mut(&id) else {
                return;
            };
            if entry.state == ProcessState::Exited {
                return;
            }
            entry.state = ProcessState::Exited;
            if let Detail::Child(child) = &mut entry.detail {
                child.report = Some(result);
            }
            if entry.kill_pending {
                entry.update_sent = true;
            }
            !entry.update_sent
        };
        if wake {
            self.wake.notify_one();
        }
    }

    pub(crate) async fn set_db_id(&self, id: RunId, db_id: i64) {
        let mut inner = self.inner.lock().await;
        if let Some(process) = inner.entries.get_mut(&id).and_then(Entry::process_mut) {
            process.db_id = Some(db_id);
        }
    }

    fn spawn_reader<R>(
        self: &Arc<Self>,
        id: RunId,
        pipe: R,
        stream: Stream,
    ) -> tokio::task::JoinHandle<()>
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
    {
        let registry = Arc::clone(self);
        tokio::spawn(async move {
            let mut reader = BufReader::new(pipe).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                registry.append_line(id, stream, line).await;
            }
        })
    }

    fn spawn_waiter(
        self: &Arc<Self>,
        id: RunId,
        mut child: tokio::process::Child,
        pgid: Option<i32>,
        kill_rx: tokio::sync::oneshot::Receiver<()>,
    ) -> tokio::task::JoinHandle<()> {
        let registry = Arc::clone(self);
        tokio::spawn(async move {
            let status = tokio::select! {
                status = child.wait() => status,
                _ = kill_rx => {
                    kill_group(pgid);
                    let _ = child.start_kill();
                    child.wait().await
                }
            };
            let code = status.ok().and_then(|s| s.code());
            registry
                .mark_exited(id, code, ProcessExitReason::Natural)
                .await;
        })
    }

    async fn append_line(self: &Arc<Self>, id: RunId, stream: Stream, text: String) {
        let should_wake = {
            let mut inner = self.inner.lock().await;
            let Some(process) = inner.entries.get_mut(&id).and_then(Entry::process_mut) else {
                return;
            };
            if process.lines.len() >= RING_CAPACITY {
                process.lines.pop_front();
                process.dropped += 1;
                if process.seen_cursor > 0 {
                    process.seen_cursor -= 1;
                }
            }
            process.lines.push_back(Line {
                stream,
                text: text.clone(),
            });
            if process.watched && !process.watch_flooded {
                let pending = process.lines.len() - process.seen_cursor;
                if pending > WATCH_FLOOD_LINES {
                    process.watched = false;
                    process.watch_flooded = true;
                }
            }
            process.watched
        };
        let _ = self
            .events
            .send(Event::ProcessOutput {
                process: id,
                chunk: text,
            })
            .await;
        if should_wake {
            self.wake.notify_one();
        }
    }

    async fn mark_exited(
        self: &Arc<Self>,
        id: RunId,
        code: Option<i32>,
        natural: ProcessExitReason,
    ) {
        let (wake, reason, db_id) = {
            let mut inner = self.inner.lock().await;
            let Some(entry) = inner.entries.get_mut(&id) else {
                return;
            };
            if entry.state == ProcessState::Exited {
                return;
            }
            entry.state = ProcessState::Exited;
            let reason = if entry.kill_pending {
                ProcessExitReason::Killed
            } else {
                natural
            };
            if reason == ProcessExitReason::Killed {
                entry.update_sent = true;
            }
            let db_id = match &mut entry.detail {
                Detail::Process(process) => {
                    process.exit_code = code;
                    process.db_id
                }
                Detail::Child(_) => None,
            };
            (!entry.update_sent, reason, db_id)
        };
        if let (Some(store), Some(db_id)) = (self.store.as_ref(), db_id) {
            let _ = store.finish_process(db_id, crate::persist::now_ms()).await;
        }
        let _ = self
            .events
            .send(Event::ProcessExited {
                process: id,
                code,
                reason,
            })
            .await;
        self.broadcast_list().await;
        if wake {
            self.wake.notify_one();
        }
    }

    pub(crate) async fn read_new(&self, id: RunId) -> Option<ReadChunk> {
        let mut inner = self.inner.lock().await;
        let entry = inner.entries.get_mut(&id)?;
        let exited = entry.state == ProcessState::Exited;
        if exited {
            entry.update_sent = true;
        }
        let state = entry.state;
        let process = entry.process_mut()?;
        let chunk = collect_from(process, process.seen_cursor);
        process.seen_cursor = process.lines.len();
        Some(ReadChunk {
            text: chunk,
            state,
            exit_code: process.exit_code,
        })
    }

    pub(crate) async fn take_pending_updates(&self) -> Vec<(RunId, RunUpdate)> {
        let mut inner = self.inner.lock().await;
        let ids: Vec<RunId> = inner.entries.keys().copied().collect();
        let mut out = Vec::new();
        for id in ids {
            let Some(entry) = inner.entries.get_mut(&id) else {
                continue;
            };
            let finished_unseen = entry.state == ProcessState::Exited && !entry.update_sent;
            let label = entry.label.clone();
            let title = entry.title.clone();
            let state = entry.state;
            let update = match &mut entry.detail {
                Detail::Process(process) => {
                    let has_new = process.watched && process.lines.len() > process.seen_cursor;
                    if !has_new && !finished_unseen {
                        continue;
                    }
                    let text = collect_tail_from(process, process.seen_cursor);
                    process.seen_cursor = process.lines.len();
                    RunUpdate {
                        label,
                        title,
                        output: text,
                        state,
                        exit_code: process.exit_code,
                        ok: None,
                    }
                }
                Detail::Child(child) => {
                    if !finished_unseen {
                        continue;
                    }
                    let (output, ok) = match child.report.take() {
                        Some(Ok(report)) => (report, Some(true)),
                        Some(Err(message)) => (message, Some(false)),
                        None => ("(no report)".to_owned(), Some(false)),
                    };
                    RunUpdate {
                        label,
                        title,
                        output,
                        state,
                        exit_code: None,
                        ok,
                    }
                }
            };
            entry.update_sent = true;
            out.push((id, update));
        }
        out.sort_by_key(|(id, _)| id.0);
        out
    }

    #[cfg(test)]
    async fn buffered_lines(&self, id: RunId) -> usize {
        let inner = self.inner.lock().await;
        inner
            .entries
            .get(&id)
            .and_then(Entry::process)
            .map_or(0, |process| process.lines.len())
    }

    pub(crate) async fn write_stdin(&self, id: RunId, text: &str) -> Result<(), String> {
        let mut stdin = {
            let mut inner = self.inner.lock().await;
            let entry = inner
                .entries
                .get_mut(&id)
                .ok_or_else(|| format!("no background run #{id}"))?;
            if entry.state == ProcessState::Exited {
                return Err(format!(
                    "run #{id} has exited; start a new process to continue"
                ));
            }
            let process = entry
                .process_mut()
                .ok_or_else(|| format!("run #{id} is not a process run"))?;
            process
                .stdin
                .take()
                .ok_or_else(|| format!("run #{id} does not accept input"))?
        };
        let write = async {
            stdin.write_all(text.as_bytes()).await?;
            stdin.flush().await
        };
        let result = write.await;
        let mut inner = self.inner.lock().await;
        if let Some(process) = inner.entries.get_mut(&id).and_then(Entry::process_mut) {
            process.stdin = Some(stdin);
        }
        result.map_err(|err| format!("failed to write to run #{id}: {err}"))
    }

    pub(crate) async fn set_watch(&self, id: RunId, on: bool) -> Result<(), String> {
        {
            let mut inner = self.inner.lock().await;
            let process = inner
                .entries
                .get_mut(&id)
                .and_then(Entry::process_mut)
                .ok_or_else(|| format!("no background process run #{id}"))?;
            process.watched = on;
            if on {
                process.watch_flooded = false;
            }
        }
        self.broadcast_list().await;
        Ok(())
    }

    pub(crate) async fn kill(&self, id: RunId, kind: Option<Kind>) -> Result<(), String> {
        let stop = {
            let mut inner = self.inner.lock().await;
            let entry = inner
                .entries
                .get_mut(&id)
                .ok_or_else(|| format!("no background run #{id}"))?;
            if let Some(kind) = kind
                && entry.kind() != kind
            {
                return Err(format!(
                    "run #{id} has kind {:?}, not {kind:?}",
                    entry.kind()
                ));
            }
            if entry.state == ProcessState::Exited {
                return Ok(());
            }
            entry.kill_pending = true;
            match &mut entry.detail {
                Detail::Process(process) => {
                    process.stdin.take();
                    Stop::Process(process.kill_tx.take())
                }
                Detail::Child(child) => Stop::Child(child.cancel.clone()),
            }
        };
        match stop {
            Stop::Process(Some(tx)) => {
                let _ = tx.send(());
            }
            Stop::Process(None) => {}
            Stop::Child(cancel) => cancel.cancel(),
        }
        Ok(())
    }

    pub(crate) async fn roster(&self) -> Vec<RunInfo> {
        let inner = self.inner.lock().await;
        let mut infos: Vec<RunInfo> = inner
            .entries
            .iter()
            .filter(|(_, entry)| entry.state == ProcessState::Running)
            .map(|(id, entry)| entry.run_info(*id))
            .collect();
        infos.sort_by_key(|i| i.id.0);
        infos
    }

    #[cfg(test)]
    pub(crate) async fn list(&self) -> Vec<ProcessInfo> {
        let inner = self.inner.lock().await;
        collect_infos(&inner)
    }

    async fn broadcast_list(&self) {
        let processes = {
            let inner = self.inner.lock().await;
            collect_infos(&inner)
        };
        let _ = self
            .events
            .send(Event::ProcessListChanged { processes })
            .await;
    }

    pub(crate) async fn shutdown_all(&self) {
        let mut entries: Vec<Entry> = {
            let mut inner = self.inner.lock().await;
            inner.entries.drain().map(|(_, entry)| entry).collect()
        };
        for entry in &mut entries {
            match &mut entry.detail {
                Detail::Process(process) => {
                    if let Some(tx) = process.kill_tx.take() {
                        let _ = tx.send(());
                    }
                }
                Detail::Child(child) => child.cancel.cancel(),
            }
        }
        for entry in &mut entries {
            for task in entry.tasks.drain(..) {
                let _ = task.await;
            }
        }
    }
}

enum Stop {
    Process(Option<tokio::sync::oneshot::Sender<()>>),
    Child(CancellationToken),
}

pub(crate) struct ReadChunk {
    pub(crate) text: String,
    pub(crate) state: ProcessState,
    pub(crate) exit_code: Option<i32>,
}

pub(crate) struct RunUpdate {
    pub(crate) label: String,
    pub(crate) title: String,
    pub(crate) output: String,
    pub(crate) state: ProcessState,
    pub(crate) exit_code: Option<i32>,
    pub(crate) ok: Option<bool>,
}

fn collect_tail_from(process: &ProcessDetail, cursor: usize) -> String {
    let unread = process.lines.len() - cursor;
    if unread <= WATCH_FLOOD_LINES {
        return collect_from(process, cursor);
    }
    let dropped = unread - WATCH_FLOOD_LINES;
    let mut out = format!("[{dropped} earlier lines dropped]\n");
    out.push_str(&collect_from(
        process,
        process.lines.len() - WATCH_FLOOD_LINES,
    ));
    out
}

fn collect_from(process: &ProcessDetail, cursor: usize) -> String {
    let mut out = String::new();
    if cursor == 0 && process.dropped > 0 {
        let _ = writeln!(out, "[{} earlier lines dropped]", process.dropped);
    }
    for line in process.lines.iter().skip(cursor) {
        if line.stream == Stream::Err {
            out.push_str("[err] ");
        }
        out.push_str(&line.text);
        out.push('\n');
    }
    out
}

fn collect_infos(inner: &Inner) -> Vec<ProcessInfo> {
    let mut infos: Vec<ProcessInfo> = inner
        .entries
        .iter()
        .filter(|(_, entry)| matches!(entry.detail, Detail::Process(_)))
        .map(|(id, entry)| entry.info(*id))
        .collect();
    infos.sort_by_key(|i| i.id.0);
    infos
}

fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut builder = Command::new("cmd");
        builder
            .arg("/C")
            .arg(command)
            .env_clear()
            .envs(goat_process::child_environment());
        builder
    }
    #[cfg(not(windows))]
    {
        let mut builder = Command::new("sh");
        builder
            .arg("-c")
            .arg(command)
            .env_clear()
            .envs(goat_process::child_environment());
        builder
    }
}

fn set_process_group(builder: &mut Command) {
    #[cfg(unix)]
    builder.process_group(0);
    #[cfg(windows)]
    {
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        builder.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(not(any(unix, windows)))]
    let _ = builder;
}

fn kill_group(pgid: Option<i32>) {
    let Some(pgid) = pgid else {
        return;
    };
    if let Err(err) = goat_process::kill_group(pgid) {
        tracing::warn!(%err, pgid, "failed to kill process group");
    }
}

#[cfg(test)]
mod tests {
    use super::{Kind, Runs, WatchMode};
    use goat_protocol::{Event, ProcessState};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::{Notify, mpsc};
    use tokio_util::sync::CancellationToken;

    #[cfg(not(windows))]
    mod plat {
        pub const TWO_ECHOES: &str = "echo hello; echo world";
        pub const ECHO_ONE: &str = "echo one";
        pub const ECHO_STDERR: &str = "echo oops 1>&2";
        pub const ECHO_PING: &str = "echo ping";
        pub const ECHO_QUIET: &str = "echo quiet";
        pub const SLEEP_LONG: &str = "sleep 30";
        pub const CAT: &str = "cat";
        pub const TRUE: &str = "true";
        pub const COUNT_TO_600: &str = "seq 1 600";
    }

    #[cfg(windows)]
    mod plat {
        pub const TWO_ECHOES: &str = "echo hello& echo world";
        pub const ECHO_ONE: &str = "echo one";
        pub const ECHO_STDERR: &str = "echo oops 1>&2";
        pub const ECHO_PING: &str = "echo ping";
        pub const ECHO_QUIET: &str = "echo quiet";
        pub const SLEEP_LONG: &str = "ping -n 31 127.0.0.1 >nul";
        pub const CAT: &str = "findstr \"^\"";
        pub const TRUE: &str = "type nul";
        pub const COUNT_TO_600: &str = "for /L %i in (1,1,600) do @echo %i";
    }

    fn harness() -> (Arc<Runs>, mpsc::Receiver<Event>, Arc<Notify>) {
        let (event_tx, event_rx) = mpsc::channel(256);
        let wake = Arc::new(Notify::new());
        let registry = Runs::new(event_tx, wake.clone(), None);
        (registry, event_rx, wake)
    }

    #[cfg(unix)]
    async fn wait_until_group_gone(pgid: i32) {
        for _ in 0..500 {
            if !goat_process::group_is_alive(pgid) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("process group {pgid} never went away");
    }

    async fn wait_until_exited(registry: &Runs, id: goat_protocol::RunId) {
        for _ in 0..1000 {
            let list = registry.list().await;
            if list
                .iter()
                .any(|p| p.id == id && p.state == ProcessState::Exited)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("run did not exit in time");
    }

    #[tokio::test]
    async fn spawn_reads_output_and_exits() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry
            .spawn(plat::TWO_ECHOES, None, &cwd, WatchMode::CompletionOnly)
            .await
            .unwrap_or_else(|e| panic!("spawn failed: {e}"));
        wait_until_exited(&registry, started.id).await;
        let chunk = registry.read_new(started.id).await.unwrap();
        assert!(chunk.text.contains("hello"), "got: {}", chunk.text);
        assert!(chunk.text.contains("world"), "got: {}", chunk.text);
        assert_eq!(chunk.state, ProcessState::Exited);
        assert_eq!(chunk.exit_code, Some(0));
    }

    #[tokio::test]
    async fn read_new_is_cursor_based() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry
            .spawn(plat::ECHO_ONE, None, &cwd, WatchMode::CompletionOnly)
            .await
            .unwrap();
        wait_until_exited(&registry, started.id).await;
        let first = registry.read_new(started.id).await.unwrap();
        assert!(first.text.contains("one"));
        let second = registry.read_new(started.id).await.unwrap();
        assert!(
            !second.text.contains("one"),
            "second read should be empty of old output"
        );
    }

    #[tokio::test]
    async fn stderr_is_tagged() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry
            .spawn(plat::ECHO_STDERR, None, &cwd, WatchMode::CompletionOnly)
            .await
            .unwrap();
        wait_until_exited(&registry, started.id).await;
        let chunk = registry.read_new(started.id).await.unwrap();
        assert!(chunk.text.contains("[err] oops"), "got: {}", chunk.text);
    }

    #[tokio::test]
    async fn watched_run_wakes_on_output() {
        let (registry, _events, wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry
            .spawn(plat::ECHO_PING, None, &cwd, WatchMode::OutputAndCompletion)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), wake.notified())
            .await
            .expect("should wake");
        let updates = registry.take_pending_updates().await;
        assert!(
            updates
                .iter()
                .any(|(id, o)| *id == started.id && o.output.contains("ping")),
            "got: {updates:?}",
            updates = updates
                .iter()
                .map(|(_, o)| o.output.clone())
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn unwatched_run_does_not_wake_while_it_runs() {
        let (registry, _events, wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry
            .spawn(plat::SLEEP_LONG, None, &cwd, WatchMode::CompletionOnly)
            .await
            .unwrap();
        let result = tokio::time::timeout(Duration::from_millis(200), wake.notified()).await;
        assert!(
            result.is_err(),
            "an unwatched run must not wake the agent for output"
        );
        registry.kill(started.id, None).await.unwrap();
        wait_until_exited(&registry, started.id).await;
    }

    #[tokio::test]
    async fn every_run_wakes_on_exit() {
        let (registry, _events, wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry
            .spawn(plat::ECHO_QUIET, None, &cwd, WatchMode::CompletionOnly)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), wake.notified())
            .await
            .expect("an exit must wake the agent even without watch");
        let updates = registry.take_pending_updates().await;
        assert!(
            updates
                .iter()
                .any(|(id, o)| *id == started.id && o.state == ProcessState::Exited),
            "the exit must be reported as a run update"
        );
    }

    #[tokio::test]
    async fn kill_terminates_running_run() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry
            .spawn(plat::SLEEP_LONG, None, &cwd, WatchMode::CompletionOnly)
            .await
            .unwrap();
        let running = registry.list().await;
        assert_eq!(running[0].state, ProcessState::Running);
        registry.kill(started.id, None).await.unwrap();
        wait_until_exited(&registry, started.id).await;
    }

    async fn wait_until_buffered(registry: &Runs, id: goat_protocol::RunId, n: usize) {
        for _ in 0..1000 {
            if registry.buffered_lines(id).await >= n {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("run never buffered {n} lines");
    }

    #[tokio::test]
    async fn a_flood_of_unread_output_is_reported_by_its_tail() {
        let (registry, mut events, _wake) = harness();
        tokio::spawn(async move { while events.recv().await.is_some() {} });
        let cwd = std::env::temp_dir();
        let started = registry
            .spawn(plat::COUNT_TO_600, None, &cwd, WatchMode::CompletionOnly)
            .await
            .unwrap();
        wait_until_exited(&registry, started.id).await;
        wait_until_buffered(&registry, started.id, 600).await;

        let updates = registry.take_pending_updates().await;
        let (_, update) = updates
            .iter()
            .find(|(id, _)| *id == started.id)
            .expect("the exit must be reported");
        assert!(
            update.output.contains("earlier lines dropped"),
            "a flood must be trimmed, got {} bytes",
            update.output.len()
        );
        assert!(
            update.output.lines().count() <= super::WATCH_FLOOD_LINES + 2,
            "the wake notice must not dump the whole ring buffer"
        );
        assert!(
            update.output.contains("600"),
            "the tail is what matters, got: {}",
            &update.output[update.output.len().saturating_sub(40)..]
        );
    }

    #[tokio::test]
    async fn killed_run_does_not_wake() {
        let (registry, _events, wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry
            .spawn(plat::SLEEP_LONG, None, &cwd, WatchMode::OutputAndCompletion)
            .await
            .unwrap();
        registry.kill(started.id, None).await.unwrap();
        wait_until_exited(&registry, started.id).await;
        let result = tokio::time::timeout(Duration::from_millis(200), wake.notified()).await;
        assert!(
            result.is_err(),
            "a run the agent killed itself must not wake it"
        );
        let pending = registry.take_pending_updates().await;
        assert!(
            pending.is_empty(),
            "a killed run must not be reported as an update"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdin_write_reaches_run() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry
            .spawn(plat::CAT, None, &cwd, WatchMode::CompletionOnly)
            .await
            .unwrap();
        registry.write_stdin(started.id, "typed\n").await.unwrap();
        let mut echoed = String::new();
        let mut got = false;
        for _ in 0..200 {
            let chunk = registry.read_new(started.id).await.unwrap();
            echoed.push_str(&chunk.text);
            if echoed.contains("typed") {
                got = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        registry.kill(started.id, None).await.unwrap();
        wait_until_exited(&registry, started.id).await;
        assert!(got, "stdin was not echoed back");
    }

    #[cfg(not(unix))]
    #[tokio::test]
    async fn stdin_write_succeeds() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry
            .spawn(plat::CAT, None, &cwd, WatchMode::CompletionOnly)
            .await
            .unwrap();
        registry.write_stdin(started.id, "typed\n").await.unwrap();
        registry.kill(started.id, None).await.unwrap();
        wait_until_exited(&registry, started.id).await;
    }

    #[tokio::test]
    async fn write_to_exited_run_errors() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry
            .spawn(plat::TRUE, None, &cwd, WatchMode::CompletionOnly)
            .await
            .unwrap();
        wait_until_exited(&registry, started.id).await;
        let result = registry.write_stdin(started.id, "x\n").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn watch_can_be_toggled() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry
            .spawn(plat::SLEEP_LONG, None, &cwd, WatchMode::CompletionOnly)
            .await
            .unwrap();
        registry.set_watch(started.id, true).await.unwrap();
        registry.write_stdin(started.id, "").await.ok();
        registry.set_watch(started.id, false).await.unwrap();
        registry.kill(started.id, None).await.unwrap();
    }

    #[tokio::test]
    async fn reading_output_leaves_no_pending_update() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry
            .spawn(plat::ECHO_PING, None, &cwd, WatchMode::OutputAndCompletion)
            .await
            .unwrap();
        wait_until_exited(&registry, started.id).await;

        let chunk = registry.read_new(started.id).await.unwrap();
        assert!(chunk.text.contains("ping"), "got: {}", chunk.text);
        assert_eq!(chunk.state, ProcessState::Exited);

        let pending = registry.take_pending_updates().await;
        assert!(
            pending.is_empty(),
            "output already read via BashOutput must not wake the agent again, got: {:?}",
            pending
                .iter()
                .map(|(_, o)| o.output.clone())
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn unread_output_still_reported_after_exit() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry
            .spawn(plat::ECHO_PING, None, &cwd, WatchMode::OutputAndCompletion)
            .await
            .unwrap();
        wait_until_exited(&registry, started.id).await;

        let pending = registry.take_pending_updates().await;
        assert!(
            pending
                .iter()
                .any(|(id, o)| *id == started.id && o.output.contains("ping")),
            "output the agent never read must still wake it"
        );
    }

    #[tokio::test]
    async fn shutdown_all_terminates_running_runs() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let a = registry
            .spawn(plat::SLEEP_LONG, None, &cwd, WatchMode::CompletionOnly)
            .await
            .unwrap();
        let b = registry
            .spawn(plat::SLEEP_LONG, None, &cwd, WatchMode::CompletionOnly)
            .await
            .unwrap();
        assert_eq!(registry.list().await.len(), 2);

        registry.shutdown_all().await;

        assert!(registry.list().await.is_empty());
        assert!(registry.read_new(a.id).await.is_none());
        assert!(registry.read_new(b.id).await.is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_all_leaves_no_live_group() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry
            .spawn(plat::SLEEP_LONG, None, &cwd, WatchMode::CompletionOnly)
            .await
            .unwrap();
        let pgid = started.pgid.expect("spawned run has a group");
        assert!(goat_process::group_is_alive(pgid));

        registry.shutdown_all().await;

        wait_until_group_gone(pgid).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn kill_reaps_the_whole_group() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry
            .spawn(plat::SLEEP_LONG, None, &cwd, WatchMode::CompletionOnly)
            .await
            .unwrap();
        let pgid = started.pgid.expect("spawned run has a group");

        registry.kill(started.id, None).await.unwrap();
        wait_until_exited(&registry, started.id).await;

        wait_until_group_gone(pgid).await;
    }

    #[tokio::test]
    async fn kill_after_natural_exit_signals_nothing() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry
            .spawn(plat::TRUE, None, &cwd, WatchMode::CompletionOnly)
            .await
            .unwrap();
        wait_until_exited(&registry, started.id).await;

        registry.kill(started.id, None).await.unwrap();
    }

    #[tokio::test]
    async fn finished_child_wakes_once_with_its_report() {
        let (registry, _events, wake) = harness();
        let id = registry
            .register_child("map the auth flow", CancellationToken::new())
            .await;
        registry
            .finish_child(id, Ok("auth goes through goat-auth".to_owned()))
            .await;
        tokio::time::timeout(Duration::from_secs(5), wake.notified())
            .await
            .expect("a finished child must wake the agent");

        let updates = registry.take_pending_updates().await;
        let (_, update) = updates
            .iter()
            .find(|(o, _)| *o == id)
            .expect("reported once");
        assert_eq!(update.label, "subagent");
        assert_eq!(update.ok, Some(true));
        assert!(update.output.contains("goat-auth"));
        assert_eq!(update.title, "map the auth flow");

        assert!(
            registry.take_pending_updates().await.is_empty(),
            "a report already delivered must not wake the agent again"
        );
    }

    #[tokio::test]
    async fn killed_child_does_not_wake() {
        let (registry, _events, wake) = harness();
        let cancel = CancellationToken::new();
        let id = registry.register_child("general", cancel.clone()).await;

        registry.kill(id, Some(Kind::Child)).await.unwrap();
        assert!(cancel.is_cancelled(), "kill must cancel the child token");

        registry
            .finish_child(id, Err("child interrupted".to_owned()))
            .await;
        let result = tokio::time::timeout(Duration::from_millis(200), wake.notified()).await;
        assert!(
            result.is_err(),
            "a child the agent killed itself must not wake it"
        );
        assert!(registry.take_pending_updates().await.is_empty());
    }

    #[tokio::test]
    async fn kill_rejects_a_kind_mismatch() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry
            .spawn(plat::SLEEP_LONG, None, &cwd, WatchMode::CompletionOnly)
            .await
            .unwrap();

        let err = registry
            .kill(started.id, Some(Kind::Child))
            .await
            .expect_err("a process run is not killable as a child");
        assert!(err.contains("kind Process"), "got: {err}");

        registry
            .kill(started.id, Some(Kind::Process))
            .await
            .unwrap();
        wait_until_exited(&registry, started.id).await;
    }

    #[tokio::test]
    async fn roster_carries_both_kinds_and_hides_finished_runs() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let process = registry
            .spawn(plat::SLEEP_LONG, None, &cwd, WatchMode::CompletionOnly)
            .await
            .unwrap();
        let agent = registry
            .register_child("read the docs", CancellationToken::new())
            .await;

        let roster = registry.roster().await;
        assert_eq!(roster.len(), 2);
        assert_eq!(roster[0].label, "process");
        assert_eq!(roster[1].label, "subagent");
        assert_eq!(roster[1].title, "read the docs");

        assert_eq!(
            registry.list().await.len(),
            1,
            "the process list stays process-only so the tui does not double-count childs"
        );

        registry.finish_child(agent, Ok(String::new())).await;
        assert_eq!(registry.roster().await.len(), 1, "finished runs drop out");

        registry.kill(process.id, None).await.unwrap();
    }

    #[tokio::test]
    async fn output_stays_readable_after_a_flood_clears_watch() {
        let (registry, mut events, _wake) = harness();
        tokio::spawn(async move { while events.recv().await.is_some() {} });
        let cwd = std::env::temp_dir();
        let started = registry
            .spawn(
                plat::COUNT_TO_600,
                None,
                &cwd,
                WatchMode::OutputAndCompletion,
            )
            .await
            .unwrap();
        wait_until_exited(&registry, started.id).await;
        wait_until_buffered(&registry, started.id, 600).await;

        let roster = registry.roster().await;
        assert!(
            roster.is_empty(),
            "an exited run leaves the roster regardless of watch"
        );

        let chunk = registry.read_new(started.id).await.expect("still readable");
        assert!(
            chunk.text.contains("600"),
            "a flood that switched watch off must still be readable via BashOutput"
        );
    }
}
