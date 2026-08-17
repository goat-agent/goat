use goat_protocol::{
    AccountEntry, Event, Mode, ModelEntry, ModelTarget, Op, ProcessInfo, RateLimitSnapshot,
    SkillInfo, TaskId, TranscriptEntry, Usage,
};
use goat_wire::envelope::{StreamClass, id_serde};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::cursor::Cursor;
use crate::registry::{Direction, Grant, Method, Shape};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct SessionId(pub u64);

impl Serialize for SessionId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        id_serde::serialize(&self.0, s)
    }
}

impl<'de> Deserialize<'de> for SessionId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        id_serde::deserialize(d).map(Self)
    }
}

impl JsonSchema for SessionId {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "SessionId".into()
    }
    fn schema_id() -> std::borrow::Cow<'static, str> {
        concat!(module_path!(), "::SessionId").into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <String as JsonSchema>::json_schema(generator)
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Empty {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DaemonStatus2 {
    pub version: String,
    pub pid: u32,
    pub started_at: i64,
    pub ready: bool,
    pub epoch: String,
    pub sessions: usize,
    pub turns: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type")]
pub enum SessionLiveState {
    Idle {},
    Active {},
    WaitingOnAsk {},
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionInfo {
    pub session: SessionId,
    pub cwd: String,
    pub state: SessionLiveState,
    pub windows: usize,
    pub age_ms: i64,
    pub tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationInfo {
    pub conversation_id: i64,
    pub cwd: String,
    pub title: Option<String>,
    pub model: String,
    pub updated_at: i64,
    pub live: Option<SessionId>,
    pub state: Option<SessionLiveState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type")]
pub enum ResumeMode {
    New {},
    Latest {},
    Conversation { conversation_id: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageEntry {
    pub provider: String,
    pub account: String,
    pub usage: Usage,
    pub context_window: Option<u32>,
    pub compaction_threshold: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RetryEntry {
    pub id: TaskId,
    pub attempt: u32,
    pub max_attempts: u32,
    pub delay_ms: u64,
    pub reason: String,
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RateLimitEntry {
    pub provider: String,
    pub account: String,
    pub snapshot: RateLimitSnapshot,
    pub cached_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionSnapshot {
    pub session: SessionId,
    pub cwd: String,
    pub target: Option<ModelTarget>,
    pub transcript: Vec<TranscriptEntry>,
    pub pending: Vec<Event>,
    pub context_tokens: Option<u32>,
    pub compaction_threshold: Option<u32>,
    pub skills: Vec<SkillInfo>,
    pub accounts: Vec<AccountEntry>,
    pub models: Vec<ModelEntry>,
    pub selected: Option<ModelTarget>,
    pub mode: Mode,
    pub plan_path: Option<String>,
    pub processes: Vec<ProcessInfo>,
    pub usage: Vec<UsageEntry>,
    pub rate_limits: Vec<RateLimitEntry>,
    pub active: Option<TaskId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum WatchItem {
    Snapshot {
        cursor: Cursor,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        reset: bool,
        state: Box<SessionSnapshot>,
    },
    Event {
        cursor: Cursor,
        event: Box<Event>,
    },
    Presence {
        cursor: Cursor,
        clients: usize,
    },
}

impl WatchItem {
    pub fn cursor(&self) -> &Cursor {
        match self {
            Self::Snapshot { cursor, .. }
            | Self::Event { cursor, .. }
            | Self::Presence { cursor, .. } => cursor,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type")]
pub enum WatchFrom {
    Snapshot {},
    Cursor { cursor: CursorRef },
}

pub type CursorRef = Cursor;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DirEntry {
    pub name: String,
    pub kind: DirEntryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type")]
pub enum DirEntryKind {
    Directory {},
    File {},
    Symlink {},
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileChunk {
    pub path: String,
    pub content: String,
    pub offset: u64,
    pub len: u64,
    pub total: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DiffFile {
    pub path: String,
    pub status: String,
    pub added: u32,
    pub removed: u32,
    pub patch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CapabilityOffer {
    pub id: String,
    pub version: u16,
    pub max_in_flight: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CapabilityProvider {
    pub instance: String,
    pub label: String,
    pub capability: String,
    pub version: u16,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bound: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnswerOutcome {
    Accepted,
    AlreadyAnswered,
}

macro_rules! method {
    (
        $ty:ident, $name:literal, $version:literal,
        $shape:expr, $grant:expr, $direction:expr,
        $params:ty, $output:ty, $item:ty
    ) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct $ty;

        impl Method for $ty {
            const NAME: &'static str = $name;
            const VERSION: u16 = $version;
            const SHAPE: Shape = $shape;
            const GRANT: Grant = $grant;
            const DIRECTION: Direction = $direction;
            type Params = $params;
            type Output = $output;
            type Item = $item;
        }
    };
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionListOutput {
    pub sessions: Vec<SessionInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationListParams {
    pub cwd: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationListOutput {
    pub conversations: Vec<ConversationInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionOpenParams {
    pub cwd: String,
    pub resume: ResumeMode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionOpenOutput {
    pub session: SessionId,
    pub cwd: String,
    pub epoch: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionSubmitParams {
    pub session: SessionId,
    pub op: Op,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionSubmitOutput {
    pub task: TaskId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionControlParams {
    pub session: SessionId,
    pub op: Op,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionKillParams {
    pub session: SessionId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "edit", rename_all = "snake_case")]
pub enum ConfigEdit {
    ProviderSet {
        name: String,
        endpoint: String,
    },
    ProviderRemove {
        name: String,
    },
    SearchAccountSet {
        account: serde_json::Value,
    },
    SearchAccountRemove {
        target: String,
    },
    SearchDefaultSet {
        target: Option<String>,
    },
    IntegrationSet {
        kind: String,
        config: serde_json::Value,
    },
    IntegrationRemove {
        kind: String,
    },
    BrowserSet {
        enabled: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminConfigEditParams {
    pub edits: Vec<ConfigEdit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminConfigEditOutput {
    pub changed: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminDaemonStopParams {
    #[serde(default)]
    pub if_idle: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum AdminDaemonStopOutput {
    Stopping,
    Busy { sessions: usize, turns: usize },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionWatchParams {
    pub session: SessionId,
    pub from: WatchFrom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AskAnswerParams {
    pub session: SessionId,
    pub prompt: i64,
    pub revision: i64,
    pub answers: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AskAnswerOutput {
    pub outcome: AnswerOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsListParams {
    pub path: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub recursive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsListOutput {
    pub path: String,
    pub entries: Vec<DirEntry>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsReadParams {
    pub path: String,
    #[serde(default)]
    pub offset: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub len: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsWriteParams {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsWriteOutput {
    pub path: String,
    pub len: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitDiffParams {
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitDiffOutput {
    pub files: Vec<DiffFile>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PtyOpenParams {
    pub cwd: String,
    pub cols: u16,
    pub rows: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum PtyItem {
    Opened { pty: String },
    Output { data: String },
    Exited { code: Option<i32> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PtyWriteParams {
    pub pty: String,
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PtyResizeParams {
    pub pty: String,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CapabilityAdvertiseParams {
    pub instance: String,
    pub label: String,
    pub boot_epoch: u64,
    #[serde(default)]
    pub offers: Vec<CapabilityOffer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CapabilityListParams {
    pub capability: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CapabilityListOutput {
    pub providers: Vec<CapabilityProvider>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CapabilityBindParams {
    pub session: SessionId,
    pub capability: String,
    pub instance: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentWatchParams {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<String>,
    pub from: WatchFrom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum AgentActivity {
    TurnStarted {
        cursor: Cursor,
        agent: String,
        run: i64,
        trigger: String,
    },
    TurnFinished {
        cursor: Cursor,
        agent: String,
        run: i64,
        ok: bool,
    },
    ToolStarted {
        cursor: Cursor,
        agent: String,
        run: i64,
        tool: String,
    },
    ScheduleFired {
        cursor: Cursor,
        agent: String,
        run: i64,
        schedule: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminAgentReloadParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReloadFailure {
    pub agent: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminAgentReloadOutput {
    pub reloaded: Vec<String>,
    pub unchanged: Vec<String>,
    pub failed: Vec<ReloadFailure>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminDevicePairParams {
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminDevicePairOutput {
    pub code: String,
    pub server_fingerprint: String,
    pub advertised: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceInfo {
    pub id: String,
    pub label: String,
    pub paired_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminDeviceListOutput {
    pub devices: Vec<DeviceInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminDeviceRevokeParams {
    pub device: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminDeviceRevokeOutput {
    pub ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostOrigin {
    pub session: SessionId,
    pub task: TaskId,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HostBrowserParams {
    pub origin: HostOrigin,
    pub action: String,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostBrowserOutput {
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostNotifyParams {
    pub origin: HostOrigin,
    pub title: String,
    pub body: String,
}

method!(
    DaemonStatus,
    "daemon.status",
    1,
    Shape::Unary,
    Grant::Any,
    Direction::ToDaemon,
    Empty,
    DaemonStatus2,
    ()
);
method!(
    SessionList,
    "session.list",
    1,
    Shape::Unary,
    Grant::Any,
    Direction::ToDaemon,
    Empty,
    SessionListOutput,
    ()
);
method!(
    ConversationList,
    "conversation.list",
    1,
    Shape::Unary,
    Grant::Any,
    Direction::ToDaemon,
    ConversationListParams,
    ConversationListOutput,
    ()
);
method!(
    SessionOpen,
    "session.open",
    1,
    Shape::Unary,
    Grant::Any,
    Direction::ToDaemon,
    SessionOpenParams,
    SessionOpenOutput,
    ()
);
method!(
    SessionSubmit,
    "session.submit",
    1,
    Shape::Unary,
    Grant::Any,
    Direction::ToDaemon,
    SessionSubmitParams,
    SessionSubmitOutput,
    ()
);
method!(
    SessionControl,
    "session.control",
    1,
    Shape::Unary,
    Grant::Any,
    Direction::ToDaemon,
    SessionControlParams,
    Empty,
    ()
);
method!(
    SessionKill,
    "session.kill",
    1,
    Shape::Unary,
    Grant::Any,
    Direction::ToDaemon,
    SessionKillParams,
    Empty,
    ()
);
method!(
    SessionWatch,
    "session.watch",
    1,
    Shape::Stream(StreamClass::Reliable),
    Grant::Any,
    Direction::ToDaemon,
    SessionWatchParams,
    Empty,
    WatchItem
);
method!(
    AskAnswer,
    "ask.answer",
    1,
    Shape::Unary,
    Grant::Any,
    Direction::ToDaemon,
    AskAnswerParams,
    AskAnswerOutput,
    ()
);
method!(
    FsList,
    "fs.list",
    1,
    Shape::Unary,
    Grant::Any,
    Direction::ToDaemon,
    FsListParams,
    FsListOutput,
    ()
);
method!(
    FsRead,
    "fs.read",
    1,
    Shape::Unary,
    Grant::Any,
    Direction::ToDaemon,
    FsReadParams,
    FileChunk,
    ()
);
method!(
    FsWrite,
    "fs.write",
    1,
    Shape::Unary,
    Grant::Any,
    Direction::ToDaemon,
    FsWriteParams,
    FsWriteOutput,
    ()
);
method!(
    GitDiff,
    "git.diff",
    1,
    Shape::Unary,
    Grant::Any,
    Direction::ToDaemon,
    GitDiffParams,
    GitDiffOutput,
    ()
);
method!(
    PtyOpen,
    "pty.open",
    1,
    Shape::Stream(StreamClass::Lossy),
    Grant::Any,
    Direction::ToDaemon,
    PtyOpenParams,
    Empty,
    PtyItem
);
method!(
    PtyWrite,
    "pty.write",
    1,
    Shape::Unary,
    Grant::Any,
    Direction::ToDaemon,
    PtyWriteParams,
    Empty,
    ()
);
method!(
    PtyResize,
    "pty.resize",
    1,
    Shape::Unary,
    Grant::Any,
    Direction::ToDaemon,
    PtyResizeParams,
    Empty,
    ()
);
method!(
    CapabilityAdvertise,
    "capability.advertise",
    1,
    Shape::Unary,
    Grant::Any,
    Direction::ToDaemon,
    CapabilityAdvertiseParams,
    Empty,
    ()
);
method!(
    CapabilityList,
    "capability.list",
    1,
    Shape::Unary,
    Grant::Any,
    Direction::ToDaemon,
    CapabilityListParams,
    CapabilityListOutput,
    ()
);
method!(
    CapabilityBind,
    "capability.bind",
    1,
    Shape::Unary,
    Grant::Any,
    Direction::ToDaemon,
    CapabilityBindParams,
    Empty,
    ()
);
method!(
    AgentWatch,
    "agent.watch",
    1,
    Shape::Stream(StreamClass::Reliable),
    Grant::Any,
    Direction::ToDaemon,
    AgentWatchParams,
    Empty,
    AgentActivity
);
method!(
    AdminAgentReload,
    "admin.agent_reload",
    1,
    Shape::Unary,
    Grant::Admin,
    Direction::ToDaemon,
    AdminAgentReloadParams,
    AdminAgentReloadOutput,
    ()
);
method!(
    AdminConfigEdit,
    "admin.config_edit",
    1,
    Shape::Unary,
    Grant::Admin,
    Direction::ToDaemon,
    AdminConfigEditParams,
    AdminConfigEditOutput,
    ()
);
method!(
    AdminDaemonStop,
    "admin.daemon_stop",
    1,
    Shape::Unary,
    Grant::Admin,
    Direction::ToDaemon,
    AdminDaemonStopParams,
    AdminDaemonStopOutput,
    ()
);
method!(
    AdminDevicePair,
    "admin.device_pair",
    1,
    Shape::Unary,
    Grant::Admin,
    Direction::ToDaemon,
    AdminDevicePairParams,
    AdminDevicePairOutput,
    ()
);
method!(
    AdminDeviceList,
    "admin.device_list",
    1,
    Shape::Unary,
    Grant::Admin,
    Direction::ToDaemon,
    Empty,
    AdminDeviceListOutput,
    ()
);
method!(
    AdminDeviceRevoke,
    "admin.device_revoke",
    1,
    Shape::Unary,
    Grant::Admin,
    Direction::ToDaemon,
    AdminDeviceRevokeParams,
    AdminDeviceRevokeOutput,
    ()
);
method!(
    HostBrowser,
    "host.browser",
    1,
    Shape::Unary,
    Grant::Any,
    Direction::ToClient,
    HostBrowserParams,
    HostBrowserOutput,
    ()
);
method!(
    HostNotify,
    "host.notify",
    1,
    Shape::Unary,
    Grant::Any,
    Direction::ToClient,
    HostNotifyParams,
    Empty,
    ()
);

#[cfg(test)]
mod tests {
    use super::{
        AnswerOutcome, AskAnswerParams, Cursor, Empty, HostBrowserParams, HostOrigin, PtyItem,
        SessionId, WatchFrom, WatchItem,
    };
    use goat_protocol::TaskId;

    #[test]
    fn session_id_serializes_as_a_string_outside_the_js_safe_range() {
        let id = SessionId(9_007_199_254_740_993);
        assert_eq!(serde_json::to_string(&id).unwrap(), r#""9007199254740993""#);
        let back: SessionId = serde_json::from_str(r#""9007199254740993""#).unwrap();
        assert_eq!(back, id);
        let from_number: SessionId = serde_json::from_str("7").unwrap();
        assert_eq!(from_number, SessionId(7));
    }

    #[test]
    fn empty_params_serialize_as_an_object() {
        assert_eq!(serde_json::to_string(&Empty {}).unwrap(), "{}");
        let _: Empty = serde_json::from_str("{}").unwrap();
    }

    #[test]
    fn watch_from_distinguishes_snapshot_and_resume() {
        let snapshot = WatchFrom::Snapshot {};
        assert_eq!(
            serde_json::to_string(&snapshot).unwrap(),
            r#"{"type":"Snapshot"}"#
        );
        let resume = WatchFrom::Cursor {
            cursor: Cursor::new("e7", 1042),
        };
        assert_eq!(
            serde_json::to_string(&resume).unwrap(),
            r#"{"type":"Cursor","cursor":"e7:1042"}"#
        );
    }

    #[test]
    fn watch_items_expose_their_cursor() {
        let event = WatchItem::Event {
            cursor: Cursor::new("e7", 5),
            event: Box::new(goat_protocol::Event::TaskDone {
                id: TaskId(1),
                interrupted: false,
            }),
        };
        assert_eq!(event.cursor(), &Cursor::new("e7", 5));
        let presence = WatchItem::Presence {
            cursor: Cursor::new("e7", 6),
            clients: 2,
        };
        assert_eq!(presence.cursor(), &Cursor::new("e7", 6));
        let text = serde_json::to_string(&presence).unwrap();
        assert!(text.contains(r#""t":"presence""#));
        let back: WatchItem = serde_json::from_str(&text).unwrap();
        assert_eq!(back, presence);
    }

    #[test]
    fn ask_answer_carries_the_revision_that_makes_it_a_compare_and_set() {
        let params = AskAnswerParams {
            session: SessionId(1),
            prompt: 42,
            revision: 3,
            answers: vec!["main".to_owned()],
        };
        let text = serde_json::to_string(&params).unwrap();
        let back: AskAnswerParams = serde_json::from_str(&text).unwrap();
        assert_eq!(back, params);
        assert_ne!(
            serde_json::to_string(&AnswerOutcome::Accepted).unwrap(),
            serde_json::to_string(&AnswerOutcome::AlreadyAnswered).unwrap()
        );
    }

    #[test]
    fn a_host_call_always_names_its_origin() {
        let params = HostBrowserParams {
            origin: HostOrigin {
                session: SessionId(7),
                task: TaskId(9),
                label: "fix the flake".to_owned(),
            },
            action: "navigate".to_owned(),
            arguments: serde_json::json!({"url": "https://example.com"}),
        };
        let text = serde_json::to_string(&params).unwrap();
        assert!(text.contains(r#""origin""#));
        let back: HostBrowserParams = serde_json::from_str(&text).unwrap();
        assert_eq!(back, params);
    }

    #[test]
    fn pty_items_round_trip() {
        for item in [
            PtyItem::Opened {
                pty: "p_2".to_owned(),
            },
            PtyItem::Output {
                data: "$ ls\n".to_owned(),
            },
            PtyItem::Exited { code: Some(0) },
            PtyItem::Exited { code: None },
        ] {
            let text = serde_json::to_string(&item).unwrap();
            let back: PtyItem = serde_json::from_str(&text).unwrap();
            assert_eq!(back, item);
        }
    }
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
