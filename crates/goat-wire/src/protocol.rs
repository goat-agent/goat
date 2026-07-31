use serde::{Deserialize, Deserializer, Serialize, Serializer};

use goat_protocol::{
    AccountEntry, Event, ModelEntry, ModelTarget, Op, RateLimitSnapshot, SkillInfo, TranscriptEntry,
};

pub const PROTOCOL_VERSION: u32 = 7;

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
    Hello {
        version: u32,
    },
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
    ListThreads {
        cwd: String,
    },
    ListDirectory {
        path: String,
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
    Goodbye {},
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type")]
pub enum ResumeMode {
    New {},
    Latest {},
    Thread { thread_id: i64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type")]
pub enum ServerFrame {
    Welcome {
        version: u32,
        client_id: ClientId,
    },
    SessionOpened {
        session: SessionId,
    },
    Detached {
        session: SessionId,
    },
    Snapshot {
        session: SessionId,
        watermark: u64,
        target: Option<ModelTarget>,
        transcript: Vec<TranscriptEntry>,
        context_tokens: Option<u32>,
        compaction_threshold: Option<u32>,
        skills: Vec<SkillInfo>,
        accounts: Vec<AccountEntry>,
        model_list: Vec<ModelEntry>,
        selected: Option<ModelTarget>,
        rate_limits: Vec<RateLimitEntry>,
    },
    Event {
        session: SessionId,
        seq: u64,
        event: Event,
    },
    Sessions {
        sessions: Vec<SessionInfo>,
    },
    Threads {
        threads: Vec<ThreadInfo>,
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
    VersionMismatch {
        daemon_version: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RateLimitEntry {
    pub provider: String,
    pub account: String,
    pub snapshot: RateLimitSnapshot,
    pub cached_at: i64,
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
pub struct ThreadInfo {
    pub thread_id: i64,
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
