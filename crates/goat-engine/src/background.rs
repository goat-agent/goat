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
    Bash,
    Subagent,
}

impl Kind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Subagent => "subagent",
        }
    }
}

struct Bash {
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

struct Subagent {
    report: Option<Result<String, String>>,
    cancel: CancellationToken,
}

enum Detail {
    Bash(Bash),
    Subagent(Subagent),
}

struct Entry {
    title: String,
    state: ProcessState,
    observed: bool,
    kill_pending: bool,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    detail: Detail,
}

impl Entry {
    fn kind(&self) -> Kind {
        match self.detail {
            Detail::Bash(_) => Kind::Bash,
            Detail::Subagent(_) => Kind::Subagent,
        }
    }

    fn bash(&self) -> Option<&Bash> {
        match &self.detail {
            Detail::Bash(bash) => Some(bash),
            Detail::Subagent(_) => None,
        }
    }

    fn bash_mut(&mut self) -> Option<&mut Bash> {
        match &mut self.detail {
            Detail::Bash(bash) => Some(bash),
            Detail::Subagent(_) => None,
        }
    }

    fn abort_tasks(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }

    fn watched(&self) -> bool {
        self.bash().is_some_and(|bash| bash.watched)
    }

    fn info(&self, id: RunId) -> ProcessInfo {
        ProcessInfo {
            id,
            command: self.title.clone(),
            state: self.state,
            watched: self.watched(),
            exit_code: self.bash().and_then(|bash| bash.exit_code),
        }
    }

    fn run_info(&self, id: RunId) -> RunInfo {
        RunInfo {
            id,
            kind: self.kind(),
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
    store: Option<goat_store::CodeStore>,
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
                "too many background runs are already going (limit {MAX_LIVE_PROCESSES}); stop one with BashKill first"
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
    pub(crate) kind: Kind,
    pub(crate) title: String,
    pub(crate) watched: bool,
}

impl Runs {
    pub(crate) fn new(
        events: mpsc::Sender<Event>,
        wake: Arc<Notify>,
        store: Option<goat_store::CodeStore>,
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

    pub(crate) async fn spawn(
        self: &Arc<Self>,
        command: &str,
        cwd: &Path,
        watched: bool,
    ) -> Result<Started, SpawnError> {
        let id = {
            let inner = self.inner.lock().await;
            let live = inner
                .entries
                .values()
                .filter(|e| e.state == ProcessState::Running && matches!(e.detail, Detail::Bash(_)))
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
                    title: command.to_owned(),
                    state: ProcessState::Running,
                    observed: false,
                    kill_pending: false,
                    tasks,
                    detail: Detail::Bash(Bash {
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

    pub(crate) async fn register_subagent(
        &self,
        subagent_type: &str,
        label: &str,
        cancel: CancellationToken,
    ) -> RunId {
        let mut inner = self.inner.lock().await;
        let id = RunId(inner.next_id);
        inner.next_id += 1;
        let title = if label.is_empty() {
            subagent_type.to_owned()
        } else {
            format!("{subagent_type} — {label}")
        };
        inner.entries.insert(
            id,
            Entry {
                title,
                state: ProcessState::Running,
                observed: false,
                kill_pending: false,
                tasks: Vec::new(),
                detail: Detail::Subagent(Subagent {
                    report: None,
                    cancel,
                }),
            },
        );
        id
    }

    pub(crate) async fn finish_subagent(&self, id: RunId, result: Result<String, String>) {
        let wake = {
            let mut inner = self.inner.lock().await;
            let Some(entry) = inner.entries.get_mut(&id) else {
                return;
            };
            if entry.state == ProcessState::Exited {
                return;
            }
            entry.state = ProcessState::Exited;
            if let Detail::Subagent(subagent) = &mut entry.detail {
                subagent.report = Some(result);
            }
            if entry.kill_pending {
                entry.observed = true;
            }
            !entry.observed
        };
        if wake {
            self.wake.notify_one();
        }
    }

    pub(crate) async fn set_db_id(&self, id: RunId, db_id: i64) {
        let mut inner = self.inner.lock().await;
        if let Some(bash) = inner.entries.get_mut(&id).and_then(Entry::bash_mut) {
            bash.db_id = Some(db_id);
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
            let Some(bash) = inner.entries.get_mut(&id).and_then(Entry::bash_mut) else {
                return;
            };
            if bash.lines.len() >= RING_CAPACITY {
                bash.lines.pop_front();
                bash.dropped += 1;
                if bash.seen_cursor > 0 {
                    bash.seen_cursor -= 1;
                }
            }
            bash.lines.push_back(Line {
                stream,
                text: text.clone(),
            });
            if bash.watched && !bash.watch_flooded {
                let pending = bash.lines.len() - bash.seen_cursor;
                if pending > WATCH_FLOOD_LINES {
                    bash.watched = false;
                    bash.watch_flooded = true;
                }
            }
            bash.watched
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
                entry.observed = true;
            }
            let db_id = match &mut entry.detail {
                Detail::Bash(bash) => {
                    bash.exit_code = code;
                    bash.db_id
                }
                Detail::Subagent(_) => None,
            };
            (!entry.observed, reason, db_id)
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
            entry.observed = true;
        }
        let state = entry.state;
        let bash = entry.bash_mut()?;
        let chunk = collect_from(bash, bash.seen_cursor);
        bash.seen_cursor = bash.lines.len();
        Some(ReadChunk {
            text: chunk,
            state,
            exit_code: bash.exit_code,
        })
    }

    pub(crate) async fn take_pending_observations(&self) -> Vec<(RunId, Observation)> {
        let mut inner = self.inner.lock().await;
        let ids: Vec<RunId> = inner.entries.keys().copied().collect();
        let mut out = Vec::new();
        for id in ids {
            let Some(entry) = inner.entries.get_mut(&id) else {
                continue;
            };
            let finished_unseen = entry.state == ProcessState::Exited && !entry.observed;
            let kind = entry.kind();
            let title = entry.title.clone();
            let state = entry.state;
            let observation = match &mut entry.detail {
                Detail::Bash(bash) => {
                    let has_new = bash.watched && bash.lines.len() > bash.seen_cursor;
                    if !has_new && !finished_unseen {
                        continue;
                    }
                    let text = collect_tail_from(bash, bash.seen_cursor);
                    bash.seen_cursor = bash.lines.len();
                    Observation {
                        kind,
                        title,
                        output: text,
                        state,
                        exit_code: bash.exit_code,
                        ok: None,
                    }
                }
                Detail::Subagent(subagent) => {
                    if !finished_unseen {
                        continue;
                    }
                    let (output, ok) = match subagent.report.take() {
                        Some(Ok(report)) => (report, Some(true)),
                        Some(Err(message)) => (message, Some(false)),
                        None => ("(no report)".to_owned(), Some(false)),
                    };
                    Observation {
                        kind,
                        title,
                        output,
                        state,
                        exit_code: None,
                        ok,
                    }
                }
            };
            entry.observed = true;
            out.push((id, observation));
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
            .and_then(Entry::bash)
            .map_or(0, |bash| bash.lines.len())
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
                    "run #{id} has exited; start it again with Bash(background=true)"
                ));
            }
            let bash = entry
                .bash_mut()
                .ok_or_else(|| format!("run #{id} is not a bash run"))?;
            bash.stdin
                .take()
                .ok_or_else(|| format!("run #{id} does not accept input"))?
        };
        let write = async {
            stdin.write_all(text.as_bytes()).await?;
            stdin.flush().await
        };
        let result = write.await;
        let mut inner = self.inner.lock().await;
        if let Some(bash) = inner.entries.get_mut(&id).and_then(Entry::bash_mut) {
            bash.stdin = Some(stdin);
        }
        result.map_err(|err| format!("failed to write to run #{id}: {err}"))
    }

    pub(crate) async fn set_watch(&self, id: RunId, on: bool) -> Result<(), String> {
        {
            let mut inner = self.inner.lock().await;
            let bash = inner
                .entries
                .get_mut(&id)
                .and_then(Entry::bash_mut)
                .ok_or_else(|| format!("no background bash run #{id}"))?;
            bash.watched = on;
            if on {
                bash.watch_flooded = false;
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
                    "run #{id} is a {} run, not a {} run",
                    entry.kind().label(),
                    kind.label()
                ));
            }
            if entry.state == ProcessState::Exited {
                return Ok(());
            }
            entry.kill_pending = true;
            match &mut entry.detail {
                Detail::Bash(bash) => {
                    bash.stdin.take();
                    Stop::Bash(bash.kill_tx.take())
                }
                Detail::Subagent(subagent) => Stop::Subagent(subagent.cancel.clone()),
            }
        };
        match stop {
            Stop::Bash(Some(tx)) => {
                let _ = tx.send(());
            }
            Stop::Bash(None) => {}
            Stop::Subagent(cancel) => cancel.cancel(),
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
                Detail::Bash(bash) => {
                    if let Some(tx) = bash.kill_tx.take() {
                        let _ = tx.send(());
                    }
                }
                Detail::Subagent(subagent) => subagent.cancel.cancel(),
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
    Bash(Option<tokio::sync::oneshot::Sender<()>>),
    Subagent(CancellationToken),
}

pub(crate) struct ReadChunk {
    pub(crate) text: String,
    pub(crate) state: ProcessState,
    pub(crate) exit_code: Option<i32>,
}

pub(crate) struct Observation {
    pub(crate) kind: Kind,
    pub(crate) title: String,
    pub(crate) output: String,
    pub(crate) state: ProcessState,
    pub(crate) exit_code: Option<i32>,
    pub(crate) ok: Option<bool>,
}

fn collect_tail_from(bash: &Bash, cursor: usize) -> String {
    let unread = bash.lines.len() - cursor;
    if unread <= WATCH_FLOOD_LINES {
        return collect_from(bash, cursor);
    }
    let dropped = unread - WATCH_FLOOD_LINES;
    let mut out = format!("[{dropped} earlier lines dropped]\n");
    out.push_str(&collect_from(bash, bash.lines.len() - WATCH_FLOOD_LINES));
    out
}

fn collect_from(bash: &Bash, cursor: usize) -> String {
    let mut out = String::new();
    if cursor == 0 && bash.dropped > 0 {
        let _ = writeln!(out, "[{} earlier lines dropped]", bash.dropped);
    }
    for line in bash.lines.iter().skip(cursor) {
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
        .filter(|(_, entry)| matches!(entry.detail, Detail::Bash(_)))
        .map(|(id, entry)| entry.info(*id))
        .collect();
    infos.sort_by_key(|i| i.id.0);
    infos
}

fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut builder = Command::new("cmd");
        builder.arg("/C").arg(command);
        builder
    }
    #[cfg(not(windows))]
    {
        let mut builder = Command::new("sh");
        builder.arg("-c").arg(command);
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
    use super::{Kind, Runs};
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
            .spawn(plat::TWO_ECHOES, &cwd, false)
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
        let started = registry.spawn(plat::ECHO_ONE, &cwd, false).await.unwrap();
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
            .spawn(plat::ECHO_STDERR, &cwd, false)
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
        let started = registry.spawn(plat::ECHO_PING, &cwd, true).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), wake.notified())
            .await
            .expect("should wake");
        let obs = registry.take_pending_observations().await;
        assert!(
            obs.iter()
                .any(|(id, o)| *id == started.id && o.output.contains("ping")),
            "got: {obs:?}",
            obs = obs
                .iter()
                .map(|(_, o)| o.output.clone())
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn unwatched_run_does_not_wake_while_it_runs() {
        let (registry, _events, wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry.spawn(plat::SLEEP_LONG, &cwd, false).await.unwrap();
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
        let started = registry.spawn(plat::ECHO_QUIET, &cwd, false).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), wake.notified())
            .await
            .expect("an exit must wake the agent even without watch");
        let obs = registry.take_pending_observations().await;
        assert!(
            obs.iter()
                .any(|(id, o)| *id == started.id && o.state == ProcessState::Exited),
            "the exit must be reported as an observation"
        );
    }

    #[tokio::test]
    async fn kill_terminates_running_run() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry.spawn(plat::SLEEP_LONG, &cwd, false).await.unwrap();
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
    async fn a_flood_of_unread_output_is_observed_by_its_tail() {
        let (registry, mut events, _wake) = harness();
        tokio::spawn(async move { while events.recv().await.is_some() {} });
        let cwd = std::env::temp_dir();
        let started = registry
            .spawn(plat::COUNT_TO_600, &cwd, false)
            .await
            .unwrap();
        wait_until_exited(&registry, started.id).await;
        wait_until_buffered(&registry, started.id, 600).await;

        let obs = registry.take_pending_observations().await;
        let (_, observation) = obs
            .iter()
            .find(|(id, _)| *id == started.id)
            .expect("the exit must be observed");
        assert!(
            observation.output.contains("earlier lines dropped"),
            "a flood must be trimmed, got {} bytes",
            observation.output.len()
        );
        assert!(
            observation.output.lines().count() <= super::WATCH_FLOOD_LINES + 2,
            "the wake notice must not dump the whole ring buffer"
        );
        assert!(
            observation.output.contains("600"),
            "the tail is what matters, got: {}",
            &observation.output[observation.output.len().saturating_sub(40)..]
        );
    }

    #[tokio::test]
    async fn killed_run_does_not_wake() {
        let (registry, _events, wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry.spawn(plat::SLEEP_LONG, &cwd, true).await.unwrap();
        registry.kill(started.id, None).await.unwrap();
        wait_until_exited(&registry, started.id).await;
        let result = tokio::time::timeout(Duration::from_millis(200), wake.notified()).await;
        assert!(
            result.is_err(),
            "a run the agent killed itself must not wake it"
        );
        let pending = registry.take_pending_observations().await;
        assert!(
            pending.is_empty(),
            "a killed run must not be reported as an observation"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdin_write_reaches_run() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry.spawn(plat::CAT, &cwd, false).await.unwrap();
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
        let started = registry.spawn(plat::CAT, &cwd, false).await.unwrap();
        registry.write_stdin(started.id, "typed\n").await.unwrap();
        registry.kill(started.id, None).await.unwrap();
        wait_until_exited(&registry, started.id).await;
    }

    #[tokio::test]
    async fn write_to_exited_run_errors() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry.spawn(plat::TRUE, &cwd, false).await.unwrap();
        wait_until_exited(&registry, started.id).await;
        let result = registry.write_stdin(started.id, "x\n").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn watch_can_be_toggled() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry.spawn(plat::SLEEP_LONG, &cwd, false).await.unwrap();
        registry.set_watch(started.id, true).await.unwrap();
        registry.write_stdin(started.id, "").await.ok();
        registry.set_watch(started.id, false).await.unwrap();
        registry.kill(started.id, None).await.unwrap();
    }

    #[tokio::test]
    async fn reading_output_leaves_no_pending_observation() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry.spawn(plat::ECHO_PING, &cwd, true).await.unwrap();
        wait_until_exited(&registry, started.id).await;

        let chunk = registry.read_new(started.id).await.unwrap();
        assert!(chunk.text.contains("ping"), "got: {}", chunk.text);
        assert_eq!(chunk.state, ProcessState::Exited);

        let pending = registry.take_pending_observations().await;
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
    async fn unread_output_still_observed_after_exit() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry.spawn(plat::ECHO_PING, &cwd, true).await.unwrap();
        wait_until_exited(&registry, started.id).await;

        let pending = registry.take_pending_observations().await;
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
        let a = registry.spawn(plat::SLEEP_LONG, &cwd, false).await.unwrap();
        let b = registry.spawn(plat::SLEEP_LONG, &cwd, false).await.unwrap();
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
        let started = registry.spawn(plat::SLEEP_LONG, &cwd, false).await.unwrap();
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
        let started = registry.spawn(plat::SLEEP_LONG, &cwd, false).await.unwrap();
        let pgid = started.pgid.expect("spawned run has a group");

        registry.kill(started.id, None).await.unwrap();
        wait_until_exited(&registry, started.id).await;

        wait_until_group_gone(pgid).await;
    }

    #[tokio::test]
    async fn kill_after_natural_exit_signals_nothing() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry.spawn(plat::TRUE, &cwd, false).await.unwrap();
        wait_until_exited(&registry, started.id).await;

        registry.kill(started.id, None).await.unwrap();
    }

    #[tokio::test]
    async fn finished_subagent_wakes_once_with_its_report() {
        let (registry, _events, wake) = harness();
        let id = registry
            .register_subagent("explore", "map the auth flow", CancellationToken::new())
            .await;
        registry
            .finish_subagent(id, Ok("auth goes through goat-auth".to_owned()))
            .await;
        tokio::time::timeout(Duration::from_secs(5), wake.notified())
            .await
            .expect("a finished subagent must wake the agent");

        let obs = registry.take_pending_observations().await;
        let (_, observation) = obs.iter().find(|(o, _)| *o == id).expect("observed once");
        assert_eq!(observation.kind, Kind::Subagent);
        assert_eq!(observation.ok, Some(true));
        assert!(observation.output.contains("goat-auth"));
        assert!(observation.title.contains("explore"));

        assert!(
            registry.take_pending_observations().await.is_empty(),
            "a report already delivered must not wake the agent again"
        );
    }

    #[tokio::test]
    async fn killed_subagent_does_not_wake() {
        let (registry, _events, wake) = harness();
        let cancel = CancellationToken::new();
        let id = registry
            .register_subagent("general", "", cancel.clone())
            .await;

        registry.kill(id, Some(Kind::Subagent)).await.unwrap();
        assert!(cancel.is_cancelled(), "kill must cancel the subagent token");

        registry
            .finish_subagent(id, Err("subagent interrupted".to_owned()))
            .await;
        let result = tokio::time::timeout(Duration::from_millis(200), wake.notified()).await;
        assert!(
            result.is_err(),
            "a subagent the agent killed itself must not wake it"
        );
        assert!(registry.take_pending_observations().await.is_empty());
    }

    #[tokio::test]
    async fn kill_rejects_a_kind_mismatch() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry.spawn(plat::SLEEP_LONG, &cwd, false).await.unwrap();

        let err = registry
            .kill(started.id, Some(Kind::Subagent))
            .await
            .expect_err("a bash run is not killable as a subagent");
        assert!(err.contains("bash run"), "got: {err}");

        registry.kill(started.id, Some(Kind::Bash)).await.unwrap();
        wait_until_exited(&registry, started.id).await;
    }

    #[tokio::test]
    async fn roster_carries_both_kinds_and_hides_finished_runs() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let bash = registry.spawn(plat::SLEEP_LONG, &cwd, false).await.unwrap();
        let agent = registry
            .register_subagent("explore", "read the docs", CancellationToken::new())
            .await;

        let roster = registry.roster().await;
        assert_eq!(roster.len(), 2);
        assert_eq!(roster[0].kind, Kind::Bash);
        assert_eq!(roster[1].kind, Kind::Subagent);
        assert!(roster[1].title.contains("read the docs"));

        assert_eq!(
            registry.list().await.len(),
            1,
            "the process list stays bash-only so the tui does not double-count subagents"
        );

        registry.finish_subagent(agent, Ok(String::new())).await;
        assert_eq!(registry.roster().await.len(), 1, "finished runs drop out");

        registry.kill(bash.id, None).await.unwrap();
    }

    #[tokio::test]
    async fn output_stays_readable_after_a_flood_clears_watch() {
        let (registry, mut events, _wake) = harness();
        tokio::spawn(async move { while events.recv().await.is_some() {} });
        let cwd = std::env::temp_dir();
        let started = registry
            .spawn(plat::COUNT_TO_600, &cwd, true)
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
