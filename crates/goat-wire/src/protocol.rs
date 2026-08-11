use serde::{Deserialize, Deserializer, Serialize, Serializer};

use goat_protocol::{
    AccountEntry, Event, Mode, ModelEntry, ModelTarget, Op, ProcessInfo, RateLimitSnapshot,
    SkillInfo, TaskId, TranscriptEntry, Usage,
};

pub fn wire_fingerprint() -> &'static str {
    include_str!("wire_fingerprint.txt").trim_ascii_end()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BuildId {
    pub path: String,
    pub len: u64,
    pub mtime: i64,
}

impl BuildId {
    pub fn current() -> Option<Self> {
        Self::of(&std::env::current_exe().ok()?)
    }

    pub fn of(path: &std::path::Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        let mtime = meta
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?;
        Some(Self {
            path: path.display().to_string(),
            len: meta.len(),
            mtime: i64::try_from(mtime.as_millis()).unwrap_or(i64::MAX),
        })
    }
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct Busy {
    pub sessions: usize,
    pub turns: usize,
}

impl Busy {
    pub fn is_idle(self) -> bool {
        self.sessions == 0 && self.turns == 0
    }
}

fn id_json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    <String as schemars::JsonSchema>::json_schema(generator)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(pub u64);

impl Serialize for SessionId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        goat_protocol::id_serde::serialize(&self.0, s)
    }
}

impl<'de> Deserialize<'de> for SessionId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        goat_protocol::id_serde::deserialize(d).map(Self)
    }
}

impl schemars::JsonSchema for SessionId {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "SessionId".into()
    }
    fn schema_id() -> std::borrow::Cow<'static, str> {
        concat!(module_path!(), "::SessionId").into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        id_json_schema(generator)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClientId(pub u64);

impl Serialize for ClientId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        goat_protocol::id_serde::serialize(&self.0, s)
    }
}

impl<'de> Deserialize<'de> for ClientId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        goat_protocol::id_serde::deserialize(d).map(Self)
    }
}

impl schemars::JsonSchema for ClientId {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ClientId".into()
    }
    fn schema_id() -> std::borrow::Cow<'static, str> {
        concat!(module_path!(), "::ClientId").into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        id_json_schema(generator)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type")]
pub enum ClientFrame {
    OpenSession {
        cwd: String,
        resume: ResumeMode,
    },
    Attach {
        session: SessionId,
    },
    Submit {
        session: SessionId,
        #[serde(with = "goat_protocol::id_serde")]
        #[schemars(with = "String")]
        correlation: u64,
        op: Op,
    },
    Control {
        session: SessionId,
        op: Op,
    },
    ListSessions {},
    ListConversations {
        cwd: String,
    },
    ListDirectory {
        path: String,
        recursive: bool,
    },
    KillSession {
        session: SessionId,
    },
    PairDevice {
        label: String,
    },
    ListDevices {},
    RevokeDevice {
        device: String,
    },
    StopDaemon {},
    ReloadAgents {
        agent: Option<String>,
    },
    Goodbye {},
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type")]
pub enum ResumeMode {
    New {},
    Latest {},
    Conversation { conversation_id: i64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type")]
pub enum ServerFrame {
    Welcome {
        wire: String,
        build: Option<BuildId>,
        busy: Busy,
        version: String,
        pid: u32,
        started_at: i64,
        ready: bool,
        client_id: ClientId,
    },
    SessionOpened {
        session: SessionId,
        cwd: String,
    },
    Detached {
        session: SessionId,
    },
    Snapshot {
        session: SessionId,
        watermark: u64,
        target: Box<Option<ModelTarget>>,
        transcript: Vec<TranscriptEntry>,
        pending: Vec<Event>,
        context_tokens: Option<u32>,
        compaction_threshold: Option<u32>,
        skills: Vec<SkillInfo>,
        accounts: Vec<AccountEntry>,
        model_list: Vec<ModelEntry>,
        selected: Box<Option<ModelTarget>>,
        rate_limits: Vec<RateLimitEntry>,
        mode: ModeEntry,
        processes: Vec<ProcessInfo>,
        usage: Vec<UsageEntry>,
        active: Option<TaskId>,
        retry: Box<Option<RetryEntry>>,
    },
    Event {
        session: SessionId,
        seq: u64,
        event: Event,
    },
    Sessions {
        sessions: Vec<SessionInfo>,
    },
    Conversations {
        conversations: Vec<ConversationInfo>,
    },
    Directory {
        path: String,
        children: Vec<DirEntry>,
    },
    CorrelationAssigned {
        session: SessionId,
        #[serde(with = "goat_protocol::id_serde")]
        #[schemars(with = "String")]
        correlation: u64,
        task: goat_protocol::TaskId,
    },
    Presence {
        session: SessionId,
        clients: Vec<ClientId>,
    },
    PairingCode {
        code: String,
        server_fingerprint: String,
        advertised: Vec<String>,
    },
    Devices {
        devices: Vec<DeviceInfo>,
    },
    DeviceRevoked {
        ok: bool,
    },
    Error {
        message: String,
    },
    Reloaded {
        report: ReloadReport,
    },
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReloadReport {
    pub reloaded: Vec<String>,
    pub unchanged: Vec<String>,
    pub failed: Vec<ReloadFailure>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReloadFailure {
    pub agent: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RateLimitEntry {
    pub provider: String,
    pub account: String,
    pub snapshot: RateLimitSnapshot,
    pub cached_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ModeEntry {
    pub mode: Mode,
    pub plan_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct UsageEntry {
    pub provider: String,
    pub account: String,
    pub usage: Usage,
    pub context_window: Option<u32>,
    pub compaction_threshold: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RetryEntry {
    pub id: TaskId,
    pub attempt: u32,
    pub max_attempts: u32,
    pub delay_ms: u64,
    pub reason: String,
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DeviceInfo {
    pub id: String,
    pub label: String,
    pub paired_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SessionInfo {
    pub session: SessionId,
    pub cwd: String,
    pub state: SessionLiveState,
    pub windows: usize,
    pub age_ms: i64,
    pub tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ConversationInfo {
    pub conversation_id: i64,
    pub cwd: String,
    pub title: Option<String>,
    pub model: String,
    pub updated_at: i64,
    pub live: Option<SessionId>,
    pub state: Option<SessionLiveState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type")]
pub enum SessionLiveState {
    Idle {},
    Active {},
    WaitingOnAsk {},
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DirEntry {
    pub name: String,
    pub kind: DirEntryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type")]
pub enum DirEntryKind {
    Directory {},
    File {},
    Symlink {},
}

#[cfg(test)]
mod fingerprint {
    use std::fmt::Write as _;

    use sha2::{Digest, Sha256};

    use super::{ClientFrame, ServerFrame, wire_fingerprint};

    fn render() -> String {
        let schemas = [
            serde_json::to_value(schemars::schema_for!(ClientFrame)).unwrap(),
            serde_json::to_value(schemars::schema_for!(ServerFrame)).unwrap(),
            serde_json::to_value(schemars::schema_for!(goat_protocol::Op)).unwrap(),
            serde_json::to_value(schemars::schema_for!(goat_protocol::Event)).unwrap(),
        ];
        let mut out = String::new();
        for schema in schemas {
            out.push_str(&serde_json::to_string_pretty(&schema).unwrap());
            out.push('\n');
        }
        out
    }

    fn digest() -> String {
        let hash = Sha256::digest(render().as_bytes());
        let mut out = String::with_capacity(16);
        for byte in hash.iter().take(8) {
            let _ = write!(out, "{byte:02x}");
        }
        out
    }

    #[test]
    fn matches_fixture() {
        assert_eq!(digest(), wire_fingerprint());
    }

    #[test]
    #[ignore = "rewrites wire_fingerprint.txt; run after a deliberate wire change"]
    fn regenerate() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/wire_fingerprint.txt");
        std::fs::write(path, format!("{}\n", digest())).unwrap();
    }
}
