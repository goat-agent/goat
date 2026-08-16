use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use goat_types::{
    AgentId, Attachment, ChannelId, ConversationId, IncomingMessage, InstanceId, MessageId,
    UserHandle,
};
use sqlx::ConnectOptions;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use thiserror::Error;
use tracing::info;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlx: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("migrate: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid uuid: {0}")]
    Uuid(#[from] uuid::Error),
    #[error("invalid timestamp: {0}")]
    Timestamp(String),
    #[error("invalid enum value: {field}={value}")]
    InvalidEnum { field: &'static str, value: String },
}

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Clone, Debug)]
pub struct HistoryRow {
    pub direction: Direction,
    pub sender: Option<MessageSender>,
    pub text: String,
    pub attachments: Vec<Attachment>,
    pub reply_to: Option<MessageId>,
    pub ts: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MessageSender {
    User(UserHandle),
    Agent(AgentId),
}

#[derive(Clone, Debug)]
pub struct ConversationSummary {
    pub summary: String,
    pub summarized_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    In,
    Out,
}

#[derive(Clone, Debug)]
pub struct ToolInvocationRecord {
    pub agent: AgentId,
    pub conversation: ConversationId,
    pub call_id: String,
    pub tool_name: String,
    pub args_json: serde_json::Value,
    pub status: ToolInvocationStatus,
    pub output_preview: Option<String>,
    pub error: Option<String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub finished_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolInvocationStatus {
    Ok,
    Error,
}

impl ToolInvocationStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ToolLogRow {
    pub tool_name: String,
    pub status: String,
    pub started_at: DateTime<Utc>,
    pub detail: Option<String>,
}

#[derive(Clone, Debug)]
pub enum ScheduleKind {
    Once(DateTime<Utc>),
    Cron(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScheduleStatus {
    Active,
    Cancelled,
    Done,
}

impl ScheduleStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Cancelled => "cancelled",
            Self::Done => "done",
        }
    }

    fn parse(s: &str) -> StoreResult<Self> {
        match s {
            "active" => Ok(Self::Active),
            "cancelled" => Ok(Self::Cancelled),
            "done" => Ok(Self::Done),
            other => Err(StoreError::InvalidEnum {
                field: "scheduled_tasks.status",
                value: other.to_string(),
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScheduleRunStatus {
    Pending,
    Running,
    Done,
    Failed,
    Skipped,
}

impl ScheduleRunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }

    fn parse(s: &str) -> StoreResult<Self> {
        match s {
            "pending" => Ok(Self::Pending),
            "running" => Ok(Self::Running),
            "done" => Ok(Self::Done),
            "failed" => Ok(Self::Failed),
            "skipped" => Ok(Self::Skipped),
            other => Err(StoreError::InvalidEnum {
                field: "task_runs.status",
                value: other.to_string(),
            }),
        }
    }
}

#[derive(Clone, Debug)]
pub struct NewSchedule {
    pub agent: AgentId,
    pub instruction: String,
    pub tools: Vec<String>,
    pub origin_conv: ConversationId,
    pub schedule: ScheduleKind,
    pub timezone: Option<String>,
    pub created_by_msg_id: Option<MessageId>,
}

#[derive(Clone, Debug)]
pub struct Schedule {
    pub id: i64,
    pub agent: AgentId,
    pub instruction: String,
    pub tools: Vec<String>,
    pub origin_conv: ConversationId,
    pub schedule: ScheduleKind,
    pub timezone: Option<String>,
    pub status: ScheduleStatus,
    pub created_at: DateTime<Utc>,
    pub created_by_msg_id: Option<MessageId>,
}

#[derive(Clone, Debug)]
pub struct ScheduleRun {
    pub id: i64,
    pub schedule_id: i64,
    pub instruction_snapshot: String,
    pub run_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status: ScheduleRunStatus,
    pub running_since: Option<DateTime<Utc>>,
    pub attempts: i64,
    pub result_summary: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoalStatus {
    Active,
    Blocked,
    Waiting,
    Done,
    Dropped,
}

impl GoalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Blocked => "blocked",
            Self::Waiting => "waiting",
            Self::Done => "done",
            Self::Dropped => "dropped",
        }
    }

    fn parse(s: &str) -> StoreResult<Self> {
        match s {
            "active" => Ok(Self::Active),
            "blocked" => Ok(Self::Blocked),
            "waiting" => Ok(Self::Waiting),
            "done" => Ok(Self::Done),
            "dropped" => Ok(Self::Dropped),
            other => Err(StoreError::InvalidEnum {
                field: "goals.status",
                value: other.to_string(),
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoalOrigin {
    Owner,
    SelfFormed,
}

impl GoalOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::SelfFormed => "self",
        }
    }

    fn parse(s: &str) -> StoreResult<Self> {
        match s {
            "owner" => Ok(Self::Owner),
            "self" => Ok(Self::SelfFormed),
            other => Err(StoreError::InvalidEnum {
                field: "goals.origin",
                value: other.to_string(),
            }),
        }
    }
}

#[derive(Clone, Debug)]
pub struct NewGoal {
    pub agent: AgentId,
    pub title: String,
    pub detail: Option<String>,
    pub priority: i64,
    pub origin: GoalOrigin,
    pub origin_conv: Option<ConversationId>,
    pub next_review_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Clone, Debug)]
pub struct GoalRecord {
    pub id: i64,
    pub agent: AgentId,
    pub title: String,
    pub detail: Option<String>,
    pub status: GoalStatus,
    pub priority: i64,
    pub origin: GoalOrigin,
    pub origin_conv: Option<ConversationId>,
    pub next_review_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_reviewed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[async_trait]
pub trait Store: Send + Sync + 'static {
    async fn ensure_agent(&self, id: AgentId, slug: &str, display: &str) -> StoreResult<()>;

    async fn ensure_conversation(&self, conv: &ConversationId, agent: AgentId) -> StoreResult<()>;

    async fn append_incoming(&self, msg: &IncomingMessage) -> StoreResult<()>;

    async fn append_incoming_text(
        &self,
        agent: AgentId,
        conversation: &ConversationId,
        text: &str,
    ) -> StoreResult<()>;

    async fn has_agent_activity(
        &self,
        agent: AgentId,
        conversation: &ConversationId,
    ) -> StoreResult<bool>;

    async fn append_outgoing_text(
        &self,
        agent: AgentId,
        conv: &ConversationId,
        text: &str,
        reply_to: Option<&MessageId>,
    ) -> StoreResult<()>;

    async fn upsert_outgoing_text(
        &self,
        agent: AgentId,
        conv: &ConversationId,
        id: &str,
        text: &str,
        reply_to: Option<&MessageId>,
    ) -> StoreResult<()>;

    async fn append_tool_invocation(&self, record: ToolInvocationRecord) -> StoreResult<()>;

    async fn recent_tool_invocations(&self, limit: usize) -> StoreResult<Vec<ToolLogRow>>;

    async fn recent(
        &self,
        agent: AgentId,
        conv: &ConversationId,
        limit: usize,
    ) -> StoreResult<Vec<HistoryRow>>;

    async fn message_count(&self, agent: AgentId, conv: &ConversationId) -> StoreResult<usize>;

    async fn messages_from(
        &self,
        agent: AgentId,
        conv: &ConversationId,
        offset: usize,
        limit: usize,
    ) -> StoreResult<Vec<HistoryRow>>;

    async fn get_conversation_summary(
        &self,
        agent: AgentId,
        conv: &ConversationId,
    ) -> StoreResult<Option<ConversationSummary>>;

    async fn upsert_conversation_summary(
        &self,
        agent: AgentId,
        conv: &ConversationId,
        summary: &str,
        summarized_count: usize,
    ) -> StoreResult<()>;

    async fn insert_schedule(&self, new: NewSchedule) -> StoreResult<i64>;

    async fn insert_schedule_run(
        &self,
        schedule_id: i64,
        run_at: DateTime<Utc>,
        instruction_snapshot: String,
    ) -> StoreResult<i64>;

    async fn claim_due_run(
        &self,
        now: DateTime<Utc>,
    ) -> StoreResult<Option<(ScheduleRun, Schedule)>>;

    async fn finish_run(
        &self,
        run_id: i64,
        status: ScheduleRunStatus,
        result_summary: Option<String>,
    ) -> StoreResult<()>;

    async fn cancel_schedule(&self, schedule_id: i64) -> StoreResult<bool>;

    async fn cancel_schedules_by_match(
        &self,
        agent: AgentId,
        match_text: &str,
    ) -> StoreResult<Vec<i64>>;

    async fn list_active_schedules(
        &self,
        agent: AgentId,
    ) -> StoreResult<Vec<(Schedule, Option<DateTime<Utc>>)>>;

    async fn get_schedule(&self, id: i64) -> StoreResult<Option<Schedule>>;

    async fn similar_active_schedules(
        &self,
        agent: AgentId,
        needle: &str,
    ) -> StoreResult<Vec<Schedule>>;

    async fn reclaim_stale_runs(&self, stale_before: DateTime<Utc>) -> StoreResult<usize>;

    async fn cron_schedules_missing_next_run(&self) -> StoreResult<Vec<Schedule>>;

    async fn all_pending_runs(&self) -> StoreResult<Vec<(i64, i64, DateTime<Utc>)>>;

    async fn latest_conversation(&self, agent: AgentId) -> StoreResult<Option<ConversationId>>;

    async fn create_goal(&self, new: NewGoal) -> StoreResult<i64>;

    async fn update_goal_status(&self, id: i64, status: GoalStatus) -> StoreResult<()>;

    async fn set_goal_review(
        &self,
        id: i64,
        next_review_at: Option<DateTime<Utc>>,
    ) -> StoreResult<()>;

    async fn active_goals(&self, agent: AgentId) -> StoreResult<Vec<GoalRecord>>;

    async fn get_goal(&self, id: i64) -> StoreResult<Option<GoalRecord>>;

    async fn integration_state(
        &self,
        agent: AgentId,
        integration: &str,
        account: &str,
        state_key: &str,
    ) -> StoreResult<Option<String>>;

    async fn set_integration_state(
        &self,
        agent: AgentId,
        integration: &str,
        account: &str,
        state_key: &str,
        state: &str,
    ) -> StoreResult<()>;

    async fn migrate_integration_state(
        &self,
        agent: AgentId,
        integration: &str,
        account: &str,
        legacy_key: &str,
        state_key: &str,
    ) -> StoreResult<Option<String>>;

    async fn record_observation(&self, new: NewObservation) -> StoreResult<i64>;

    async fn get_observation(&self, id: i64) -> StoreResult<Option<ObservationRecord>>;

    async fn observations_by_ref(
        &self,
        agent: AgentId,
        integration: &str,
        external_ref: &str,
        limit: i64,
    ) -> StoreResult<Vec<ObservationRecord>>;

    async fn record_activity(&self, new: NewActivity) -> StoreResult<i64>;

    async fn activity_since(
        &self,
        agents: &[AgentId],
        after: i64,
        limit: i64,
    ) -> StoreResult<Vec<ActivityRecord>>;

    async fn activity_watermark(&self) -> StoreResult<i64>;
}

#[derive(Clone, Debug)]
pub struct NewObservation {
    pub agent: AgentId,
    pub integration: String,
    pub account: String,
    pub external_ref: String,
    pub kind: String,
    pub payload: serde_json::Value,
}

pub const ACTIVITY_RETAINED_PER_AGENT: i64 = 2000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActivityKind {
    TurnStarted,
    TurnFinished,
    ToolStarted,
    ScheduleFired,
}

impl ActivityKind {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TurnStarted => "turn_started",
            Self::TurnFinished => "turn_finished",
            Self::ToolStarted => "tool_started",
            Self::ScheduleFired => "schedule_fired",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "turn_started" => Some(Self::TurnStarted),
            "turn_finished" => Some(Self::TurnFinished),
            "tool_started" => Some(Self::ToolStarted),
            "schedule_fired" => Some(Self::ScheduleFired),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct NewActivity {
    pub agent: AgentId,
    pub kind: ActivityKind,
    pub run_id: i64,
    pub detail: Option<String>,
    pub ok: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityRecord {
    pub id: i64,
    pub agent: AgentId,
    pub kind: ActivityKind,
    pub run_id: i64,
    pub detail: Option<String>,
    pub ok: Option<bool>,
    pub at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct ObservationRecord {
    pub id: i64,
    pub agent: AgentId,
    pub integration: String,
    pub account: String,
    pub external_ref: String,
    pub kind: String,
    pub payload: serde_json::Value,
    pub observed_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct SqliteStore {
    pool: Arc<SqlitePool>,
}

impl SqliteStore {
    pub async fn open(path: &Path) -> StoreResult<Self> {
        goat_sqlite_vec::register();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let url = format!("sqlite://{}", path.display());
        let opts: sqlx::sqlite::SqliteConnectOptions = url
            .parse::<sqlx::sqlite::SqliteConnectOptions>()?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .busy_timeout(std::time::Duration::from_secs(5))
            .disable_statement_logging();
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(opts)
            .await?;
        inherit_legacy_migrations(&pool).await?;
        let mut migrator = sqlx::migrate!("./migrations");
        migrator.dangerous_set_table_name("_sqlx_migrations_agent");
        migrator.run(&pool).await?;
        restrict_db_permissions(path);
        info!(path = %path.display(), "opened sqlite store");
        Ok(Self {
            pool: Arc::new(pool),
        })
    }
}

async fn inherit_legacy_migrations(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _sqlx_migrations_agent (
             version BIGINT PRIMARY KEY,
             description TEXT NOT NULL,
             installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
             success BOOLEAN NOT NULL,
             checksum BLOB NOT NULL,
             execution_time BIGINT NOT NULL
         )",
    )
    .execute(pool)
    .await?;
    let legacy: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations')",
    )
    .fetch_one(pool)
    .await?;
    if legacy {
        sqlx::query(
            "INSERT OR IGNORE INTO _sqlx_migrations_agent
             SELECT * FROM _sqlx_migrations
             WHERE version IN (1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 12, 13, 14, 16, 17, 18, 20, 22, 23)",
        )
        .execute(pool)
        .await?;
    }
    Ok(())
}

#[cfg(unix)]
fn restrict_db_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    for suffix in ["", "-wal", "-shm"] {
        let mut candidate = path.to_path_buf();
        if !suffix.is_empty() {
            let mut name = candidate
                .file_name()
                .map(std::ffi::OsStr::to_os_string)
                .unwrap_or_default();
            name.push(suffix);
            candidate.set_file_name(name);
        }
        if !candidate.exists() {
            continue;
        }
        if let Err(e) = std::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o600))
        {
            tracing::warn!(path = %candidate.display(), error = ?e, "failed to chmod 0600");
        }
    }
}

#[cfg(not(unix))]
fn restrict_db_permissions(_path: &Path) {}

#[async_trait]
impl Store for SqliteStore {
    async fn ensure_agent(&self, id: AgentId, slug: &str, display: &str) -> StoreResult<()> {
        sqlx::query(
            r"INSERT INTO agents (id, slug, display, created_at)
               VALUES (?, ?, ?, ?)
               ON CONFLICT(id) DO UPDATE SET slug = excluded.slug, display = excluded.display",
        )
        .bind(id.to_string())
        .bind(slug)
        .bind(display)
        .bind(Utc::now().to_rfc3339())
        .execute(&*self.pool)
        .await?;
        Ok(())
    }

    async fn ensure_conversation(&self, conv: &ConversationId, agent: AgentId) -> StoreResult<()> {
        sqlx::query(
            r"INSERT INTO conversations (id, agent_id, channel, instance, external, created_at)
               VALUES (?, ?, ?, ?, ?, ?)
               ON CONFLICT(id) DO NOTHING",
        )
        .bind(conv.to_key())
        .bind(agent.to_string())
        .bind(conv.channel.as_str())
        .bind(conv.instance.to_string())
        .bind(&conv.external)
        .bind(Utc::now().to_rfc3339())
        .execute(&*self.pool)
        .await?;
        Ok(())
    }

    async fn latest_conversation(&self, agent: AgentId) -> StoreResult<Option<ConversationId>> {
        let row: Option<(String, String, String)> = sqlx::query_as(
            r"SELECT c.channel, c.instance, c.external
               FROM conversations c
               JOIN messages m ON m.conversation_id = c.id
               WHERE c.agent_id = ?
               GROUP BY c.id
               ORDER BY MAX(m.ts) DESC
               LIMIT 1",
        )
        .bind(agent.to_string())
        .fetch_optional(&*self.pool)
        .await?;

        match row {
            None => Ok(None),
            Some((channel, instance, external)) => {
                let instance = InstanceId(Uuid::parse_str(&instance).map_err(StoreError::Uuid)?);
                Ok(Some(ConversationId::new(
                    ChannelId::new(channel),
                    instance,
                    external,
                )))
            }
        }
    }

    async fn append_incoming(&self, msg: &IncomingMessage) -> StoreResult<()> {
        self.ensure_conversation(&msg.conversation, msg.agent)
            .await?;
        sqlx::query(
            r"INSERT INTO messages
               (id, conversation_id, agent_id, direction, sender_kind, sender_key, sender_display,
                body_kind, text, attachment_ref, attachments, reply_to, ts, raw)
               VALUES (?, ?, ?, 'in', 'user', ?, ?, 'text', ?, NULL, ?, ?, ?, ?)
               ON CONFLICT(id) DO NOTHING",
        )
        .bind(&msg.id.0)
        .bind(msg.conversation.to_key())
        .bind(msg.agent.to_string())
        .bind(&msg.from.external)
        .bind(&msg.from.display)
        .bind(&msg.text)
        .bind(serde_json::to_string(&msg.attachments)?)
        .bind(&msg.parent)
        .bind(msg.ts.to_rfc3339())
        .bind(msg.raw.to_string())
        .execute(&*self.pool)
        .await?;
        Ok(())
    }

    async fn append_incoming_text(
        &self,
        agent: AgentId,
        conversation: &ConversationId,
        text: &str,
    ) -> StoreResult<()> {
        self.ensure_conversation(conversation, agent).await?;
        sqlx::query(
            r"INSERT INTO messages
               (id, conversation_id, agent_id, direction, sender_kind, sender_key,
                body_kind, text, attachment_ref, reply_to, ts, raw)
               VALUES (?, ?, ?, 'in', 'agent', ?, 'text', ?, NULL, NULL, ?, NULL)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(conversation.to_key())
        .bind(agent.to_string())
        .bind(agent.to_string())
        .bind(text)
        .bind(Utc::now().to_rfc3339())
        .execute(&*self.pool)
        .await?;
        Ok(())
    }

    async fn has_agent_activity(
        &self,
        agent: AgentId,
        conversation: &ConversationId,
    ) -> StoreResult<bool> {
        let row: (bool,) = sqlx::query_as(
            r"SELECT EXISTS(
                SELECT 1 FROM messages
                WHERE agent_id = ? AND conversation_id = ? AND direction = 'out'
                LIMIT 1
            )",
        )
        .bind(agent.to_string())
        .bind(conversation.to_key())
        .fetch_one(&*self.pool)
        .await?;
        Ok(row.0)
    }

    async fn append_outgoing_text(
        &self,
        agent: AgentId,
        conv: &ConversationId,
        text: &str,
        reply_to: Option<&MessageId>,
    ) -> StoreResult<()> {
        self.ensure_conversation(conv, agent).await?;
        sqlx::query(
            r"INSERT INTO messages
               (id, conversation_id, agent_id, direction, sender_kind, sender_key,
                body_kind, text, attachment_ref, reply_to, ts, raw)
               VALUES (?, ?, ?, 'out', 'agent', ?, 'text', ?, NULL, ?, ?, NULL)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(conv.to_key())
        .bind(agent.to_string())
        .bind(agent.to_string())
        .bind(text)
        .bind(reply_to.map(|m| m.0.clone()))
        .bind(Utc::now().to_rfc3339())
        .execute(&*self.pool)
        .await?;
        Ok(())
    }

    async fn upsert_outgoing_text(
        &self,
        agent: AgentId,
        conv: &ConversationId,
        id: &str,
        text: &str,
        reply_to: Option<&MessageId>,
    ) -> StoreResult<()> {
        self.ensure_conversation(conv, agent).await?;
        sqlx::query(
            r"INSERT INTO messages
               (id, conversation_id, agent_id, direction, sender_kind, sender_key,
                body_kind, text, attachment_ref, reply_to, ts, raw)
               VALUES (?, ?, ?, 'out', 'agent', ?, 'text', ?, NULL, ?, ?, NULL)
               ON CONFLICT(id) DO UPDATE SET text = excluded.text, ts = excluded.ts",
        )
        .bind(id)
        .bind(conv.to_key())
        .bind(agent.to_string())
        .bind(agent.to_string())
        .bind(text)
        .bind(reply_to.map(|m| m.0.clone()))
        .bind(Utc::now().to_rfc3339())
        .execute(&*self.pool)
        .await?;
        Ok(())
    }

    async fn append_tool_invocation(&self, record: ToolInvocationRecord) -> StoreResult<()> {
        self.ensure_conversation(&record.conversation, record.agent)
            .await?;
        sqlx::query(
            r"INSERT INTO tool_invocations
               (id, conversation_id, agent_id, call_id, tool_name, args_json, status,
                output_preview, error, started_at, finished_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(record.conversation.to_key())
        .bind(record.agent.to_string())
        .bind(record.call_id)
        .bind(record.tool_name)
        .bind(record.args_json.to_string())
        .bind(record.status.as_str())
        .bind(record.output_preview)
        .bind(record.error)
        .bind(record.started_at.to_rfc3339())
        .bind(record.finished_at.to_rfc3339())
        .execute(&*self.pool)
        .await?;
        Ok(())
    }

    async fn recent_tool_invocations(&self, limit: usize) -> StoreResult<Vec<ToolLogRow>> {
        let rows: Vec<(String, String, String, Option<String>, Option<String>)> = sqlx::query_as(
            r"SELECT tool_name, status, started_at, output_preview, error
               FROM tool_invocations
               ORDER BY started_at DESC
               LIMIT ?",
        )
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&*self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(tool_name, status, started_at, preview, error)| ToolLogRow {
                    tool_name,
                    status,
                    started_at: parse_ts(&started_at).unwrap_or_else(|_| Utc::now()),
                    detail: error.or(preview),
                },
            )
            .collect())
    }

    async fn recent(
        &self,
        agent: AgentId,
        conv: &ConversationId,
        limit: usize,
    ) -> StoreResult<Vec<HistoryRow>> {
        let limit = i64::try_from(limit.max(1)).unwrap_or(i64::MAX);
        let rows = sqlx::query_as::<_, HistorySqlRow>(
            r"SELECT direction, text, attachments, reply_to, ts,
                      sender_kind, sender_key, sender_display
               FROM messages
               WHERE agent_id = ? AND conversation_id = ? AND text IS NOT NULL
               ORDER BY ts DESC
               LIMIT ?",
        )
        .bind(agent.to_string())
        .bind(conv.to_key())
        .bind(limit)
        .fetch_all(&*self.pool)
        .await?;

        let mut history: Vec<HistoryRow> = rows
            .into_iter()
            .map(history_row)
            .collect::<StoreResult<_>>()?;
        history.reverse();
        Ok(history)
    }

    async fn message_count(&self, agent: AgentId, conv: &ConversationId) -> StoreResult<usize> {
        let row: (i64,) = sqlx::query_as(
            r"SELECT COUNT(*) FROM messages
               WHERE agent_id = ? AND conversation_id = ? AND text IS NOT NULL",
        )
        .bind(agent.to_string())
        .bind(conv.to_key())
        .fetch_one(&*self.pool)
        .await?;
        Ok(row.0.max(0) as usize)
    }

    async fn messages_from(
        &self,
        agent: AgentId,
        conv: &ConversationId,
        offset: usize,
        limit: usize,
    ) -> StoreResult<Vec<HistoryRow>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let rows = sqlx::query_as::<_, HistorySqlRow>(
            r"SELECT direction, text, attachments, reply_to, ts,
                      sender_kind, sender_key, sender_display
               FROM messages
               WHERE agent_id = ? AND conversation_id = ? AND text IS NOT NULL
               ORDER BY ts ASC, id ASC
               LIMIT ? OFFSET ?",
        )
        .bind(agent.to_string())
        .bind(conv.to_key())
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .bind(i64::try_from(offset).unwrap_or(i64::MAX))
        .fetch_all(&*self.pool)
        .await?;
        rows.into_iter().map(history_row).collect()
    }

    async fn get_conversation_summary(
        &self,
        agent: AgentId,
        conv: &ConversationId,
    ) -> StoreResult<Option<ConversationSummary>> {
        let row: Option<(String, i64)> = sqlx::query_as(
            r"SELECT summary, summarized_count FROM conversation_summary
               WHERE agent_id = ? AND conversation_id = ?",
        )
        .bind(agent.to_string())
        .bind(conv.to_key())
        .fetch_optional(&*self.pool)
        .await?;
        Ok(row.map(|(summary, count)| ConversationSummary {
            summary,
            summarized_count: count.max(0) as usize,
        }))
    }

    async fn upsert_conversation_summary(
        &self,
        agent: AgentId,
        conv: &ConversationId,
        summary: &str,
        summarized_count: usize,
    ) -> StoreResult<()> {
        self.ensure_conversation(conv, agent).await?;
        sqlx::query(
            r"INSERT INTO conversation_summary
               (conversation_id, agent_id, summary, summarized_count, updated_at)
               VALUES (?, ?, ?, ?, ?)
               ON CONFLICT(conversation_id) DO UPDATE SET
                 summary = excluded.summary,
                 summarized_count = excluded.summarized_count,
                 updated_at = excluded.updated_at",
        )
        .bind(conv.to_key())
        .bind(agent.to_string())
        .bind(summary)
        .bind(i64::try_from(summarized_count).unwrap_or(i64::MAX))
        .bind(Utc::now().to_rfc3339())
        .execute(&*self.pool)
        .await?;
        Ok(())
    }

    async fn insert_schedule(&self, new: NewSchedule) -> StoreResult<i64> {
        self.ensure_conversation(&new.origin_conv, new.agent)
            .await?;
        let (kind_str, once_at, cron) = match &new.schedule {
            ScheduleKind::Once(at) => ("once", Some(at.to_rfc3339()), None),
            ScheduleKind::Cron(expr) => ("cron", None, Some(expr.clone())),
        };
        let tools_json = serde_json::to_string(&new.tools)?;
        let row: (i64,) = sqlx::query_as(
            r"INSERT INTO scheduled_tasks
               (agent_id, task, tools, origin_conv, schedule_kind, once_at,
                cron, timezone, status, created_at, created_by_msg_id)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'active', ?, ?)
               RETURNING id",
        )
        .bind(new.agent.to_string())
        .bind(&new.instruction)
        .bind(tools_json)
        .bind(new.origin_conv.to_key())
        .bind(kind_str)
        .bind(once_at)
        .bind(cron)
        .bind(new.timezone)
        .bind(Utc::now().to_rfc3339())
        .bind(new.created_by_msg_id.as_ref().map(|m| m.0.clone()))
        .fetch_one(&*self.pool)
        .await?;
        Ok(row.0)
    }

    async fn insert_schedule_run(
        &self,
        schedule_id: i64,
        run_at: DateTime<Utc>,
        instruction_snapshot: String,
    ) -> StoreResult<i64> {
        let row: (i64,) = sqlx::query_as(
            r"INSERT INTO task_runs
               (task_id, task_snapshot, run_at, status, attempts)
               VALUES (?, ?, ?, 'pending', 0)
               RETURNING id",
        )
        .bind(schedule_id)
        .bind(&instruction_snapshot)
        .bind(run_at.to_rfc3339())
        .fetch_one(&*self.pool)
        .await?;
        Ok(row.0)
    }

    async fn claim_due_run(
        &self,
        now: DateTime<Utc>,
    ) -> StoreResult<Option<(ScheduleRun, Schedule)>> {
        let now_str = now.to_rfc3339();
        #[allow(clippy::type_complexity)]
        let claimed: Option<(
            i64,
            i64,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
            Option<String>,
            i64,
            Option<String>,
        )> = sqlx::query_as(
            r"UPDATE task_runs
               SET status = 'running',
                   started_at = ?,
                   running_since = ?,
                   attempts = attempts + 1
               WHERE id = (
                   SELECT id FROM task_runs
                   WHERE status = 'pending' AND run_at <= ?
                   ORDER BY run_at
                   LIMIT 1
               ) AND status = 'pending'
               RETURNING id, task_id, task_snapshot, run_at, started_at,
                         finished_at, status, running_since, attempts, result_summary",
        )
        .bind(&now_str)
        .bind(&now_str)
        .bind(&now_str)
        .fetch_optional(&*self.pool)
        .await?;

        let Some(row) = claimed else {
            return Ok(None);
        };

        let run = ScheduleRun {
            id: row.0,
            schedule_id: row.1,
            instruction_snapshot: row.2,
            run_at: parse_ts(&row.3)?,
            started_at: row.4.as_deref().map(parse_ts).transpose()?,
            finished_at: row.5.as_deref().map(parse_ts).transpose()?,
            status: ScheduleRunStatus::parse(&row.6)?,
            running_since: row.7.as_deref().map(parse_ts).transpose()?,
            attempts: row.8,
            result_summary: row.9,
        };

        let task = load_schedule(&self.pool, run.schedule_id).await?;
        Ok(Some((run, task)))
    }

    async fn finish_run(
        &self,
        run_id: i64,
        status: ScheduleRunStatus,
        result_summary: Option<String>,
    ) -> StoreResult<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r"UPDATE task_runs
               SET status = ?, finished_at = ?, running_since = NULL, result_summary = ?
               WHERE id = ?",
        )
        .bind(status.as_str())
        .bind(now)
        .bind(result_summary)
        .bind(run_id)
        .execute(&*self.pool)
        .await?;
        Ok(())
    }

    async fn cancel_schedule(&self, schedule_id: i64) -> StoreResult<bool> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            r"UPDATE scheduled_tasks
               SET status = 'cancelled'
               WHERE id = ? AND status = 'active'",
        )
        .bind(schedule_id)
        .execute(&mut *tx)
        .await?;
        let changed = result.rows_affected() > 0;
        if changed {
            sqlx::query(
                r"UPDATE task_runs
                   SET status = 'skipped', finished_at = ?
                   WHERE task_id = ? AND status = 'pending'",
            )
            .bind(Utc::now().to_rfc3339())
            .bind(schedule_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(changed)
    }

    async fn cancel_schedules_by_match(
        &self,
        agent: AgentId,
        match_text: &str,
    ) -> StoreResult<Vec<i64>> {
        let pattern = format!("%{match_text}%");
        let ids: Vec<(i64,)> = sqlx::query_as(
            r"SELECT id FROM scheduled_tasks
               WHERE agent_id = ? AND status = 'active' AND task LIKE ?",
        )
        .bind(agent.to_string())
        .bind(pattern)
        .fetch_all(&*self.pool)
        .await?;
        let mut cancelled = Vec::new();
        for (id,) in ids {
            if self.cancel_schedule(id).await? {
                cancelled.push(id);
            }
        }
        Ok(cancelled)
    }

    async fn get_schedule(&self, id: i64) -> StoreResult<Option<Schedule>> {
        let exists: Option<(i64,)> = sqlx::query_as(r"SELECT id FROM scheduled_tasks WHERE id = ?")
            .bind(id)
            .fetch_optional(&*self.pool)
            .await?;
        match exists {
            Some(_) => Ok(Some(load_schedule(&self.pool, id).await?)),
            None => Ok(None),
        }
    }

    async fn similar_active_schedules(
        &self,
        agent: AgentId,
        needle: &str,
    ) -> StoreResult<Vec<Schedule>> {
        let trimmed = needle.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }
        let pattern = format!("%{}%", trimmed.to_lowercase());
        let ids: Vec<(i64,)> = sqlx::query_as(
            r"SELECT id FROM scheduled_tasks
               WHERE agent_id = ? AND status = 'active'
                 AND LOWER(task) LIKE ?
               ORDER BY created_at",
        )
        .bind(agent.to_string())
        .bind(pattern)
        .fetch_all(&*self.pool)
        .await?;
        let mut out = Vec::with_capacity(ids.len());
        for (id,) in ids {
            out.push(load_schedule(&self.pool, id).await?);
        }
        Ok(out)
    }

    async fn reclaim_stale_runs(&self, stale_before: DateTime<Utc>) -> StoreResult<usize> {
        let stale_str = stale_before.to_rfc3339();
        let result = sqlx::query(
            r"UPDATE task_runs
               SET status = 'pending',
                   running_since = NULL
               WHERE status = 'running'
                 AND running_since IS NOT NULL
                 AND running_since < ?",
        )
        .bind(stale_str)
        .execute(&*self.pool)
        .await?;
        Ok(result.rows_affected() as usize)
    }

    async fn all_pending_runs(&self) -> StoreResult<Vec<(i64, i64, DateTime<Utc>)>> {
        let rows: Vec<(i64, i64, String)> = sqlx::query_as(
            r"SELECT id, task_id, run_at FROM task_runs
               WHERE status = 'pending'
               ORDER BY run_at",
        )
        .fetch_all(&*self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for (id, schedule_id, ts) in rows {
            out.push((id, schedule_id, parse_ts(&ts)?));
        }
        Ok(out)
    }

    async fn cron_schedules_missing_next_run(&self) -> StoreResult<Vec<Schedule>> {
        let ids: Vec<(i64,)> = sqlx::query_as(
            r"SELECT s.id FROM scheduled_tasks s
               WHERE s.status = 'active' AND s.schedule_kind = 'cron'
                 AND NOT EXISTS (
                     SELECT 1 FROM task_runs r
                     WHERE r.task_id = s.id AND r.status = 'pending'
                 )
               ORDER BY s.id",
        )
        .fetch_all(&*self.pool)
        .await?;
        let mut out = Vec::with_capacity(ids.len());
        for (id,) in ids {
            out.push(load_schedule(&self.pool, id).await?);
        }
        Ok(out)
    }

    async fn list_active_schedules(
        &self,
        agent: AgentId,
    ) -> StoreResult<Vec<(Schedule, Option<DateTime<Utc>>)>> {
        let rows: Vec<(i64,)> = sqlx::query_as(
            r"SELECT id FROM scheduled_tasks
               WHERE agent_id = ? AND status = 'active'
               ORDER BY created_at",
        )
        .bind(agent.to_string())
        .fetch_all(&*self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for (id,) in rows {
            let task = load_schedule(&self.pool, id).await?;
            let next: Option<(String,)> = sqlx::query_as(
                r"SELECT run_at FROM task_runs
                   WHERE task_id = ? AND status = 'pending'
                   ORDER BY run_at
                   LIMIT 1",
            )
            .bind(id)
            .fetch_optional(&*self.pool)
            .await?;
            let next_at = match next {
                Some((s,)) => Some(parse_ts(&s)?),
                None => None,
            };
            out.push((task, next_at));
        }
        Ok(out)
    }

    async fn create_goal(&self, new: NewGoal) -> StoreResult<i64> {
        if let Some(conv) = &new.origin_conv {
            self.ensure_conversation(conv, new.agent).await?;
        }
        let now = Utc::now().to_rfc3339();
        let row: (i64,) = sqlx::query_as(
            r"INSERT INTO goals
               (agent_id, title, detail, status, priority, origin,
                origin_conv, next_review_at, created_at, updated_at)
               VALUES (?, ?, ?, 'active', ?, ?, ?, ?, ?, ?)
               RETURNING id",
        )
        .bind(new.agent.to_string())
        .bind(&new.title)
        .bind(new.detail)
        .bind(new.priority)
        .bind(new.origin.as_str())
        .bind(
            new.origin_conv
                .as_ref()
                .map(goat_types::ConversationId::to_key),
        )
        .bind(new.next_review_at.map(|d| d.to_rfc3339()))
        .bind(&now)
        .bind(&now)
        .fetch_one(&*self.pool)
        .await?;
        Ok(row.0)
    }

    async fn update_goal_status(&self, id: i64, status: GoalStatus) -> StoreResult<()> {
        sqlx::query(r"UPDATE goals SET status = ?, updated_at = ? WHERE id = ?")
            .bind(status.as_str())
            .bind(Utc::now().to_rfc3339())
            .bind(id)
            .execute(&*self.pool)
            .await?;
        Ok(())
    }

    async fn set_goal_review(
        &self,
        id: i64,
        next_review_at: Option<DateTime<Utc>>,
    ) -> StoreResult<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r"UPDATE goals
               SET last_reviewed_at = ?, next_review_at = ?, updated_at = ?
               WHERE id = ?",
        )
        .bind(&now)
        .bind(next_review_at.map(|d| d.to_rfc3339()))
        .bind(&now)
        .bind(id)
        .execute(&*self.pool)
        .await?;
        Ok(())
    }

    async fn active_goals(&self, agent: AgentId) -> StoreResult<Vec<GoalRecord>> {
        let ids: Vec<(i64,)> = sqlx::query_as(
            r"SELECT id FROM goals
               WHERE agent_id = ? AND status = 'active'
               ORDER BY priority ASC, id ASC",
        )
        .bind(agent.to_string())
        .fetch_all(&*self.pool)
        .await?;
        let mut out = Vec::with_capacity(ids.len());
        for (id,) in ids {
            out.push(load_goal(&self.pool, id).await?);
        }
        Ok(out)
    }

    async fn get_goal(&self, id: i64) -> StoreResult<Option<GoalRecord>> {
        let exists: Option<(i64,)> = sqlx::query_as(r"SELECT id FROM goals WHERE id = ?")
            .bind(id)
            .fetch_optional(&*self.pool)
            .await?;
        match exists {
            Some(_) => Ok(Some(load_goal(&self.pool, id).await?)),
            None => Ok(None),
        }
    }

    async fn integration_state(
        &self,
        agent: AgentId,
        integration: &str,
        account: &str,
        state_key: &str,
    ) -> StoreResult<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as(
            r"SELECT state FROM integration_state
               WHERE agent_id = ? AND integration = ? AND account = ? AND state_key = ?",
        )
        .bind(agent.to_string())
        .bind(integration)
        .bind(account)
        .bind(state_key)
        .fetch_optional(&*self.pool)
        .await?;
        Ok(row.map(|(state,)| state))
    }

    async fn set_integration_state(
        &self,
        agent: AgentId,
        integration: &str,
        account: &str,
        state_key: &str,
        state: &str,
    ) -> StoreResult<()> {
        sqlx::query(
            r"INSERT INTO integration_state
               (agent_id, integration, account, state_key, state, updated_at)
               VALUES (?, ?, ?, ?, ?, ?)
               ON CONFLICT(agent_id, integration, account, state_key)
               DO UPDATE SET state = excluded.state, updated_at = excluded.updated_at",
        )
        .bind(agent.to_string())
        .bind(integration)
        .bind(account)
        .bind(state_key)
        .bind(state)
        .bind(Utc::now().to_rfc3339())
        .execute(&*self.pool)
        .await?;
        Ok(())
    }

    async fn migrate_integration_state(
        &self,
        agent: AgentId,
        integration: &str,
        account: &str,
        legacy_key: &str,
        state_key: &str,
    ) -> StoreResult<Option<String>> {
        let agent = agent.to_string();
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r"UPDATE OR IGNORE integration_state SET state_key = ?
               WHERE agent_id = ? AND integration = ? AND account = ? AND state_key = ?",
        )
        .bind(state_key)
        .bind(&agent)
        .bind(integration)
        .bind(account)
        .bind(legacy_key)
        .execute(&mut *tx)
        .await?;
        let row: Option<(String,)> = sqlx::query_as(
            r"SELECT state FROM integration_state
               WHERE agent_id = ? AND integration = ? AND account = ? AND state_key = ?",
        )
        .bind(&agent)
        .bind(integration)
        .bind(account)
        .bind(state_key)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row.map(|(state,)| state))
    }

    async fn record_observation(&self, new: NewObservation) -> StoreResult<i64> {
        let row: (i64,) = sqlx::query_as(
            r"INSERT INTO integration_observations
               (agent_id, integration, account, external_ref, kind, payload, observed_at)
               VALUES (?, ?, ?, ?, ?, ?, ?)
               RETURNING id",
        )
        .bind(new.agent.to_string())
        .bind(&new.integration)
        .bind(&new.account)
        .bind(&new.external_ref)
        .bind(&new.kind)
        .bind(new.payload.to_string())
        .bind(Utc::now().to_rfc3339())
        .fetch_one(&*self.pool)
        .await?;
        Ok(row.0)
    }

    async fn get_observation(&self, id: i64) -> StoreResult<Option<ObservationRecord>> {
        let row: Option<(i64, String, String, String, String, String, String, String)> =
            sqlx::query_as(
                r"SELECT id, agent_id, integration, account, external_ref, kind,
                          payload, observed_at
                   FROM integration_observations WHERE id = ?",
            )
            .bind(id)
            .fetch_optional(&*self.pool)
            .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(ObservationRecord {
            id: row.0,
            agent: AgentId(Uuid::parse_str(&row.1)?),
            integration: row.2,
            account: row.3,
            external_ref: row.4,
            kind: row.5,
            payload: serde_json::from_str(&row.6)?,
            observed_at: parse_ts(&row.7)?,
        }))
    }

    async fn observations_by_ref(
        &self,
        agent: AgentId,
        integration: &str,
        external_ref: &str,
        limit: i64,
    ) -> StoreResult<Vec<ObservationRecord>> {
        let rows: Vec<(i64, String, String, String, String, String, String, String)> =
            sqlx::query_as(
                r"SELECT id, agent_id, integration, account, external_ref, kind,
                          payload, observed_at
                   FROM integration_observations
                   WHERE agent_id = ? AND integration = ? AND external_ref = ?
                   ORDER BY id DESC
                   LIMIT ?",
            )
            .bind(agent.to_string())
            .bind(integration)
            .bind(external_ref)
            .bind(limit.max(1))
            .fetch_all(&*self.pool)
            .await?;
        rows.into_iter()
            .map(|row| {
                Ok(ObservationRecord {
                    id: row.0,
                    agent: AgentId(Uuid::parse_str(&row.1)?),
                    integration: row.2,
                    account: row.3,
                    external_ref: row.4,
                    kind: row.5,
                    payload: serde_json::from_str(&row.6)?,
                    observed_at: parse_ts(&row.7)?,
                })
            })
            .collect()
    }

    async fn record_activity(&self, new: NewActivity) -> StoreResult<i64> {
        let agent = new.agent.to_string();
        let row: (i64,) = sqlx::query_as(
            r"INSERT INTO agent_activity (agent_id, kind, run_id, detail, ok, at)
               VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(&agent)
        .bind(new.kind.as_str())
        .bind(new.run_id)
        .bind(new.detail.as_deref())
        .bind(new.ok.map(i64::from))
        .bind(Utc::now().to_rfc3339())
        .fetch_one(&*self.pool)
        .await?;

        sqlx::query(
            r"DELETE FROM agent_activity
               WHERE agent_id = ?
                 AND id <= (
                   SELECT id FROM agent_activity
                    WHERE agent_id = ?
                    ORDER BY id DESC
                    LIMIT 1 OFFSET ?
                 )",
        )
        .bind(&agent)
        .bind(&agent)
        .bind(ACTIVITY_RETAINED_PER_AGENT)
        .execute(&*self.pool)
        .await?;

        Ok(row.0)
    }

    async fn activity_since(
        &self,
        agents: &[AgentId],
        after: i64,
        limit: i64,
    ) -> StoreResult<Vec<ActivityRecord>> {
        let wanted: std::collections::HashSet<String> =
            agents.iter().map(ToString::to_string).collect();
        let scan = if wanted.is_empty() {
            limit.max(1)
        } else {
            limit
                .max(1)
                .saturating_mul(4)
                .min(ACTIVITY_RETAINED_PER_AGENT * 8)
        };

        #[allow(clippy::type_complexity)]
        let rows: Vec<(
            i64,
            String,
            String,
            i64,
            Option<String>,
            Option<i64>,
            String,
        )> = sqlx::query_as(
            r"SELECT id, agent_id, kind, run_id, detail, ok, at
                   FROM agent_activity
                   WHERE id > ?
                   ORDER BY id ASC
                   LIMIT ?",
        )
        .bind(after)
        .bind(scan)
        .fetch_all(&*self.pool)
        .await?;

        rows.into_iter()
            .filter(|row| wanted.is_empty() || wanted.contains(&row.1))
            .take(usize::try_from(limit.max(1)).unwrap_or(usize::MAX))
            .map(|row| {
                Ok(ActivityRecord {
                    id: row.0,
                    agent: AgentId(Uuid::parse_str(&row.1)?),
                    kind: ActivityKind::parse(&row.2).unwrap_or(ActivityKind::TurnStarted),
                    run_id: row.3,
                    detail: row.4,
                    ok: row.5.map(|value| value != 0),
                    at: parse_ts(&row.6)?,
                })
            })
            .collect()
    }

    async fn activity_watermark(&self) -> StoreResult<i64> {
        let row: (Option<i64>,) = sqlx::query_as("SELECT MAX(id) FROM agent_activity")
            .fetch_one(&*self.pool)
            .await?;
        Ok(row.0.unwrap_or(0))
    }
}

async fn load_schedule(pool: &SqlitePool, id: i64) -> StoreResult<Schedule> {
    #[allow(clippy::type_complexity)]
    let row: (
        i64,
        String,
        String,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        String,
        Option<String>,
        String,
        String,
        String,
    ) = sqlx::query_as(
        r"SELECT s.id, s.agent_id, s.task, s.tools, s.origin_conv,
                  s.schedule_kind, s.once_at, s.cron, s.timezone, s.status, s.created_at,
                  s.created_by_msg_id, c.channel, c.instance, c.external
           FROM scheduled_tasks s
           JOIN conversations c ON c.id = s.origin_conv
           WHERE s.id = ?",
    )
    .bind(id)
    .fetch_one(pool)
    .await?;

    let tools: Vec<String> = serde_json::from_str(&row.3)?;
    let agent = AgentId(Uuid::parse_str(&row.1)?);
    let instance = InstanceId(Uuid::parse_str(&row.13)?);
    let origin_conv = ConversationId::new(ChannelId::new(row.12.clone()), instance, row.14.clone());
    let schedule = match row.5.as_str() {
        "once" => {
            let at = row.6.as_deref().ok_or(StoreError::InvalidEnum {
                field: "scheduled_tasks.once_at",
                value: "null".into(),
            })?;
            ScheduleKind::Once(parse_ts(at)?)
        }
        "cron" => {
            let expr = row.7.clone().ok_or(StoreError::InvalidEnum {
                field: "scheduled_tasks.cron",
                value: "null".into(),
            })?;
            ScheduleKind::Cron(expr)
        }
        other => {
            return Err(StoreError::InvalidEnum {
                field: "scheduled_tasks.schedule_kind",
                value: other.to_string(),
            });
        }
    };

    Ok(Schedule {
        id: row.0,
        agent,
        instruction: row.2,
        tools,
        origin_conv,
        schedule,
        timezone: row.8,
        status: ScheduleStatus::parse(&row.9)?,
        created_at: parse_ts(&row.10)?,
        created_by_msg_id: row.11.map(MessageId),
    })
}

type HistorySqlRow = (
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn history_row(row: HistorySqlRow) -> StoreResult<HistoryRow> {
    let direction = match row.0.as_str() {
        "out" => Direction::Out,
        _ => Direction::In,
    };
    let sender = match (row.5.as_deref(), row.6) {
        (Some("user"), Some(external)) => Some(MessageSender::User(UserHandle {
            external,
            display: row.7,
        })),
        (Some("agent"), Some(key)) => Some(MessageSender::Agent(AgentId(Uuid::parse_str(&key)?))),
        (None, None) => None,
        (kind, key) => {
            return Err(StoreError::InvalidEnum {
                field: "messages.sender",
                value: format!("{kind:?}/{key:?}"),
            });
        }
    };
    Ok(HistoryRow {
        direction,
        sender,
        text: row.1,
        attachments: serde_json::from_str(&row.2)?,
        reply_to: row.3.map(MessageId),
        ts: parse_ts(&row.4)?,
    })
}

fn parse_ts(s: &str) -> StoreResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|e| StoreError::Timestamp(e.to_string()))
}

async fn load_goal(pool: &SqlitePool, id: i64) -> StoreResult<GoalRecord> {
    #[allow(clippy::type_complexity)]
    let row: (
        i64,
        String,
        String,
        Option<String>,
        String,
        i64,
        String,
        Option<String>,
        Option<String>,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = sqlx::query_as(
        r"SELECT g.id, g.agent_id, g.title, g.detail, g.status,
                  g.priority, g.origin, g.next_review_at,
                  g.last_reviewed_at, g.created_at, g.updated_at,
                  c.channel, c.instance, c.external
           FROM goals g
           LEFT JOIN conversations c ON c.id = g.origin_conv
           WHERE g.id = ?",
    )
    .bind(id)
    .fetch_one(pool)
    .await?;

    let agent = AgentId(Uuid::parse_str(&row.1)?);
    let origin_conv = match (&row.11, &row.12, &row.13) {
        (Some(channel), Some(instance), Some(external)) => Some(ConversationId::new(
            ChannelId::new(channel.clone()),
            InstanceId(Uuid::parse_str(instance)?),
            external.clone(),
        )),
        _ => None,
    };
    let next_review_at = row.7.as_deref().map(parse_ts).transpose()?;
    let last_reviewed_at = row.8.as_deref().map(parse_ts).transpose()?;

    Ok(GoalRecord {
        id: row.0,
        agent,
        title: row.2,
        detail: row.3,
        status: GoalStatus::parse(&row.4)?,
        priority: row.5,
        origin: GoalOrigin::parse(&row.6)?,
        origin_conv,
        next_review_at,
        last_reviewed_at,
        created_at: parse_ts(&row.9)?,
        updated_at: parse_ts(&row.10)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use goat_types::{AttachmentSource, ChannelId, InstanceId, Surface, UserHandle};

    async fn fresh() -> SqliteStore {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        std::mem::forget(dir);
        SqliteStore::open(&path).await.unwrap()
    }

    fn fixture_conv() -> ConversationId {
        ConversationId::new(ChannelId::new("discord"), InstanceId::new(), "chat:1")
    }

    async fn fixture_agent(store: &SqliteStore) -> AgentId {
        let p = AgentId::new();
        store.ensure_agent(p, "dev", "dev").await.unwrap();
        p
    }

    #[tokio::test]
    async fn activity_is_a_monotonic_feed_readable_by_cursor() {
        let s = fresh().await;
        let agent = fixture_agent(&s).await;

        assert_eq!(s.activity_watermark().await.unwrap(), 0);
        assert!(s.activity_since(&[], 0, 10).await.unwrap().is_empty());

        let first = s
            .record_activity(NewActivity {
                agent,
                kind: ActivityKind::TurnStarted,
                run_id: 41,
                detail: Some("discord".to_owned()),
                ok: None,
            })
            .await
            .unwrap();
        let second = s
            .record_activity(NewActivity {
                agent,
                kind: ActivityKind::TurnFinished,
                run_id: 41,
                detail: None,
                ok: Some(true),
            })
            .await
            .unwrap();
        assert!(second > first);
        assert_eq!(s.activity_watermark().await.unwrap(), second);

        let all = s.activity_since(&[], 0, 10).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].kind, ActivityKind::TurnStarted);
        assert_eq!(all[0].run_id, 41);
        assert_eq!(all[0].detail.as_deref(), Some("discord"));
        assert_eq!(all[1].kind, ActivityKind::TurnFinished);
        assert_eq!(all[1].ok, Some(true));

        let resumed = s.activity_since(&[], first, 10).await.unwrap();
        assert_eq!(resumed.len(), 1);
        assert_eq!(resumed[0].id, second);

        assert!(s.activity_since(&[], second, 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn activity_can_be_filtered_to_one_agent() {
        let s = fresh().await;
        let mine = fixture_agent(&s).await;
        let other = AgentId::new();
        s.ensure_agent(other, "other", "other").await.unwrap();

        for agent in [mine, other, mine] {
            s.record_activity(NewActivity {
                agent,
                kind: ActivityKind::ToolStarted,
                run_id: 1,
                detail: Some("shell".to_owned()),
                ok: None,
            })
            .await
            .unwrap();
        }

        let filtered = s.activity_since(&[mine], 0, 10).await.unwrap();
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|record| record.agent == mine));
        assert_eq!(s.activity_since(&[], 0, 10).await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn a_limit_bounds_the_page() {
        let s = fresh().await;
        let agent = fixture_agent(&s).await;
        for _ in 0..5 {
            s.record_activity(NewActivity {
                agent,
                kind: ActivityKind::ScheduleFired,
                run_id: 9,
                detail: None,
                ok: None,
            })
            .await
            .unwrap();
        }
        let page = s.activity_since(&[], 0, 2).await.unwrap();
        assert_eq!(page.len(), 2);
        let next = s.activity_since(&[], page[1].id, 2).await.unwrap();
        assert_eq!(next.len(), 2);
        assert!(next[0].id > page[1].id);
    }

    #[tokio::test]
    async fn the_feed_stays_bounded_per_agent() {
        let s = fresh().await;
        let agent = fixture_agent(&s).await;
        let overflow = ACTIVITY_RETAINED_PER_AGENT + 25;
        for _ in 0..overflow {
            s.record_activity(NewActivity {
                agent,
                kind: ActivityKind::TurnStarted,
                run_id: 1,
                detail: None,
                ok: None,
            })
            .await
            .unwrap();
        }
        let kept = s.activity_since(&[], 0, overflow * 2).await.unwrap().len();
        let retained = i64::try_from(kept).unwrap_or(i64::MAX);
        assert!(
            retained <= ACTIVITY_RETAINED_PER_AGENT + 1,
            "the activity feed must not grow without bound, kept {retained}"
        );
        assert!(retained > 0);
    }

    #[test]
    fn activity_kinds_round_trip_through_their_wire_names() {
        for kind in [
            ActivityKind::TurnStarted,
            ActivityKind::TurnFinished,
            ActivityKind::ToolStarted,
            ActivityKind::ScheduleFired,
        ] {
            assert_eq!(ActivityKind::parse(kind.as_str()), Some(kind.clone()));
        }
        assert_eq!(ActivityKind::parse("nonsense"), None);
    }

    #[tokio::test]
    async fn ensures_and_appends() {
        let s = fresh().await;
        let p = fixture_agent(&s).await;
        let conv = fixture_conv();
        let msg = IncomingMessage {
            id: MessageId("m1".into()),
            agent: p,
            conversation: conv.clone(),
            from: UserHandle {
                external: "u".into(),
                display: None,
            },
            text: "hello".into(),
            attachments: vec![],
            command: None,
            surface: Surface::Dm,
            addressed: true,
            parent: None,
            ts: Utc::now(),
            raw: serde_json::Value::Null,
        };
        s.append_incoming(&msg).await.unwrap();
        s.append_outgoing_text(p, &conv, "world", None)
            .await
            .unwrap();
        let hist = s.recent(p, &conv, 10).await.unwrap();
        assert_eq!(hist.len(), 2);
        assert_eq!(hist[0].text, "hello");
        assert_eq!(
            hist[0].sender,
            Some(MessageSender::User(UserHandle {
                external: "u".into(),
                display: None,
            }))
        );
        assert_eq!(hist[1].text, "world");
        assert_eq!(hist[1].sender, Some(MessageSender::Agent(p)));
    }

    #[tokio::test]
    async fn incoming_redelivery_preserves_one_complete_message() {
        let s = fresh().await;
        let p = fixture_agent(&s).await;
        let conv = fixture_conv();
        let attachments = vec![
            Attachment {
                mime: "image/png".into(),
                name: Some("diagram.png".into()),
                size: 42,
                source: AttachmentSource::Url("https://example.test/diagram.png".into()),
            },
            Attachment {
                mime: "application/octet-stream".into(),
                name: None,
                size: 7,
                source: AttachmentSource::ChannelRef {
                    channel: ChannelId::new("discord"),
                    kind: "attachment".into(),
                    value: "file-7".into(),
                    raw: serde_json::json!({"token": "opaque"}),
                },
            },
        ];
        let msg = IncomingMessage {
            id: MessageId("stable-external-id".into()),
            agent: p,
            conversation: conv.clone(),
            from: UserHandle {
                external: "user-42".into(),
                display: Some("Mutable Name".into()),
            },
            text: "with files".into(),
            attachments: attachments.clone(),
            command: None,
            surface: Surface::Thread,
            addressed: true,
            parent: Some("parent-message".into()),
            ts: Utc::now(),
            raw: serde_json::json!({"delivery": 1}),
        };

        s.append_incoming(&msg).await.unwrap();
        s.append_incoming(&msg).await.unwrap();

        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM messages WHERE id = ?")
            .bind(&msg.id.0)
            .fetch_one(&*s.pool)
            .await
            .unwrap();
        assert_eq!(count.0, 1);

        let history = s.recent(p, &conv, 10).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(
            history[0].sender,
            Some(MessageSender::User(msg.from.clone()))
        );
        assert_eq!(history[0].attachments, attachments);
        assert_eq!(
            history[0].reply_to,
            Some(MessageId("parent-message".into()))
        );
    }

    #[tokio::test]
    async fn has_agent_activity_flips_after_outgoing() {
        let s = fresh().await;
        let p = fixture_agent(&s).await;
        let conv = fixture_conv();

        s.append_incoming_text(p, &conv, "seed only").await.unwrap();
        assert!(
            !s.has_agent_activity(p, &conv).await.unwrap(),
            "incoming-only conversation has no agent activity"
        );

        s.append_outgoing_text(p, &conv, "reply", None)
            .await
            .unwrap();
        assert!(
            s.has_agent_activity(p, &conv).await.unwrap(),
            "outgoing message marks agent activity"
        );
    }

    #[tokio::test]
    async fn message_count_range_and_summary_round_trip() {
        let s = fresh().await;
        let p = fixture_agent(&s).await;
        let conv = fixture_conv();
        for i in 0..5 {
            let msg = IncomingMessage {
                id: MessageId(format!("m{i}")),
                agent: p,
                conversation: conv.clone(),
                from: UserHandle {
                    external: "u".into(),
                    display: None,
                },
                text: format!("msg {i}"),
                attachments: vec![],
                command: None,
                surface: Surface::Dm,
                addressed: true,
                parent: None,
                ts: Utc::now() + Duration::seconds(i),
                raw: serde_json::Value::Null,
            };
            s.append_incoming(&msg).await.unwrap();
        }

        assert_eq!(s.message_count(p, &conv).await.unwrap(), 5);

        let middle = s.messages_from(p, &conv, 1, 2).await.unwrap();
        assert_eq!(middle.len(), 2);
        assert_eq!(middle[0].text, "msg 1");
        assert_eq!(middle[1].text, "msg 2");

        assert!(
            s.get_conversation_summary(p, &conv)
                .await
                .unwrap()
                .is_none()
        );
        s.upsert_conversation_summary(p, &conv, "they discussed msgs 0-2", 3)
            .await
            .unwrap();
        let summary = s.get_conversation_summary(p, &conv).await.unwrap().unwrap();
        assert_eq!(summary.summarized_count, 3);
        assert_eq!(summary.summary, "they discussed msgs 0-2");

        s.upsert_conversation_summary(p, &conv, "updated", 4)
            .await
            .unwrap();
        let summary = s.get_conversation_summary(p, &conv).await.unwrap().unwrap();
        assert_eq!(summary.summarized_count, 4);
        assert_eq!(summary.summary, "updated");
    }

    #[tokio::test]
    async fn schedule_once_insert_and_list() {
        let s = fresh().await;
        let p = fixture_agent(&s).await;
        let conv = fixture_conv();
        s.ensure_conversation(&conv, p).await.unwrap();
        let due = Utc::now() + Duration::minutes(5);
        let schedule_id = s
            .insert_schedule(NewSchedule {
                agent: p,
                instruction: "ping example.com".into(),
                tools: vec!["shell".into()],
                origin_conv: conv.clone(),
                schedule: ScheduleKind::Once(due),
                timezone: Some("UTC".into()),
                created_by_msg_id: None,
            })
            .await
            .unwrap();
        s.insert_schedule_run(schedule_id, due, "ping example.com".into())
            .await
            .unwrap();

        let listed = s.list_active_schedules(p).await.unwrap();
        assert_eq!(listed.len(), 1);
        let (schedule, next_at) = &listed[0];
        assert_eq!(schedule.id, schedule_id);
        assert_eq!(schedule.instruction, "ping example.com");
        assert!(matches!(schedule.schedule, ScheduleKind::Once(_)));
        assert!(next_at.is_some());
    }

    #[tokio::test]
    async fn claim_due_run_is_atomic() {
        let s = fresh().await;
        let p = fixture_agent(&s).await;
        let conv = fixture_conv();
        s.ensure_conversation(&conv, p).await.unwrap();
        let past = Utc::now() - Duration::minutes(1);
        let schedule_id = s
            .insert_schedule(NewSchedule {
                agent: p,
                instruction: "old task".into(),
                tools: vec![],
                origin_conv: conv,
                schedule: ScheduleKind::Once(past),
                timezone: Some("UTC".into()),
                created_by_msg_id: None,
            })
            .await
            .unwrap();
        s.insert_schedule_run(schedule_id, past, "old task".into())
            .await
            .unwrap();

        let first = s.claim_due_run(Utc::now()).await.unwrap();
        assert!(first.is_some(), "first claim should succeed");
        let second = s.claim_due_run(Utc::now()).await.unwrap();
        assert!(
            second.is_none(),
            "second claim should find no pending run after first claimed"
        );

        let (run, schedule) = first.unwrap();
        assert_eq!(run.schedule_id, schedule_id);
        assert_eq!(run.status, ScheduleRunStatus::Running);
        assert_eq!(schedule.instruction, "old task");

        s.finish_run(run.id, ScheduleRunStatus::Done, Some("ok".into()))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn cancel_by_match_purges_pending() {
        let s = fresh().await;
        let p = fixture_agent(&s).await;
        let conv = fixture_conv();
        s.ensure_conversation(&conv, p).await.unwrap();
        let due = Utc::now() + Duration::minutes(1);
        let schedule_id = s
            .insert_schedule(NewSchedule {
                agent: p,
                instruction: "run loadtest in staging".into(),
                tools: vec![],
                origin_conv: conv,
                schedule: ScheduleKind::Once(due),
                timezone: Some("UTC".into()),
                created_by_msg_id: None,
            })
            .await
            .unwrap();
        s.insert_schedule_run(schedule_id, due, "run loadtest in staging".into())
            .await
            .unwrap();

        let cancelled = s.cancel_schedules_by_match(p, "loadtest").await.unwrap();
        assert_eq!(cancelled, vec![schedule_id]);

        let claim = s
            .claim_due_run(Utc::now() + Duration::minutes(2))
            .await
            .unwrap();
        assert!(claim.is_none(), "cancelled task's run must not be claimed");

        let active = s.list_active_schedules(p).await.unwrap();
        assert!(active.is_empty(), "cancelled task must drop out of list");
    }

    #[tokio::test]
    async fn reclaim_stale_runs_marks_failed_only_past_threshold() {
        let s = fresh().await;
        let p = fixture_agent(&s).await;
        let conv = fixture_conv();
        s.ensure_conversation(&conv, p).await.unwrap();
        let past = Utc::now() - Duration::minutes(30);
        let schedule_id = s
            .insert_schedule(NewSchedule {
                agent: p,
                instruction: "x".into(),
                tools: vec![],
                origin_conv: conv,
                schedule: ScheduleKind::Once(past),
                timezone: Some("UTC".into()),
                created_by_msg_id: None,
            })
            .await
            .unwrap();
        s.insert_schedule_run(schedule_id, past, "x".into())
            .await
            .unwrap();
        let _ = s.claim_due_run(Utc::now()).await.unwrap();

        let n = s
            .reclaim_stale_runs(Utc::now() - Duration::minutes(15))
            .await
            .unwrap();
        assert_eq!(n, 0, "fresh lease must not be reclaimed");

        let n = s
            .reclaim_stale_runs(Utc::now() + Duration::minutes(1))
            .await
            .unwrap();
        assert_eq!(n, 1, "lease past the threshold must be reclaimed");
    }

    #[tokio::test]
    async fn cron_tasks_missing_next_run_finds_them() {
        let s = fresh().await;
        let p = fixture_agent(&s).await;
        let conv = fixture_conv();
        s.ensure_conversation(&conv, p).await.unwrap();

        let schedule_id = s
            .insert_schedule(NewSchedule {
                agent: p,
                instruction: "weekly".into(),
                tools: vec![],
                origin_conv: conv,
                schedule: ScheduleKind::Cron("0 7 * * 1".into()),
                timezone: Some("UTC".into()),
                created_by_msg_id: None,
            })
            .await
            .unwrap();

        let missing = s.cron_schedules_missing_next_run().await.unwrap();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].id, schedule_id);

        s.insert_schedule_run(
            schedule_id,
            Utc::now() + Duration::minutes(1),
            "weekly".into(),
        )
        .await
        .unwrap();
        let missing = s.cron_schedules_missing_next_run().await.unwrap();
        assert!(missing.is_empty());
    }

    #[tokio::test]
    async fn cron_task_round_trip() {
        let s = fresh().await;
        let p = fixture_agent(&s).await;
        let conv = fixture_conv();
        s.ensure_conversation(&conv, p).await.unwrap();
        let schedule_id = s
            .insert_schedule(NewSchedule {
                agent: p,
                instruction: "weekly summary".into(),
                tools: vec!["read".into(), "grep".into()],
                origin_conv: conv,
                schedule: ScheduleKind::Cron("0 7 * * 1".into()),
                timezone: Some("UTC".into()),
                created_by_msg_id: None,
            })
            .await
            .unwrap();
        let active = s.list_active_schedules(p).await.unwrap();
        let (task, _) = &active[0];
        assert_eq!(task.id, schedule_id);
        match &task.schedule {
            ScheduleKind::Cron(expr) => assert_eq!(expr, "0 7 * * 1"),
            ScheduleKind::Once(_) => panic!("expected cron schedule"),
        }
        assert_eq!(task.tools, vec!["read".to_string(), "grep".to_string()]);
        assert_eq!(task.timezone.as_deref(), Some("UTC"));

        sqlx::query("UPDATE scheduled_tasks SET timezone = NULL WHERE id = ?")
            .bind(schedule_id)
            .execute(&*s.pool)
            .await
            .unwrap();
        let legacy = s.get_schedule(schedule_id).await.unwrap().unwrap();
        assert_eq!(legacy.timezone, None);
    }

    #[tokio::test]
    async fn latest_conversation_returns_most_recent() {
        let s = fresh().await;
        let p = fixture_agent(&s).await;

        assert!(s.latest_conversation(p).await.unwrap().is_none());

        let conv_old =
            ConversationId::new(ChannelId::new("discord"), InstanceId::new(), "chat:old");
        let conv_new =
            ConversationId::new(ChannelId::new("discord"), InstanceId::new(), "chat:new");

        let earlier = Utc::now() - Duration::seconds(60);
        let later = Utc::now();

        s.append_incoming(&IncomingMessage {
            id: MessageId("m-old".into()),
            agent: p,
            conversation: conv_old.clone(),
            from: UserHandle {
                external: "u".into(),
                display: None,
            },
            text: "older".into(),
            attachments: vec![],
            command: None,
            surface: Surface::Dm,
            addressed: true,
            parent: None,
            ts: earlier,
            raw: serde_json::Value::Null,
        })
        .await
        .unwrap();

        s.append_incoming(&IncomingMessage {
            id: MessageId("m-new".into()),
            agent: p,
            conversation: conv_new.clone(),
            from: UserHandle {
                external: "u".into(),
                display: None,
            },
            text: "newer".into(),
            attachments: vec![],
            command: None,
            surface: Surface::Dm,
            addressed: true,
            parent: None,
            ts: later,
            raw: serde_json::Value::Null,
        })
        .await
        .unwrap();

        let latest = s.latest_conversation(p).await.unwrap().unwrap();
        assert_eq!(latest.external, conv_new.external);
    }

    #[tokio::test]
    async fn sqlite_vec_extension_is_available() {
        let store = fresh().await;
        let v: (String,) = sqlx::query_as("SELECT vec_version()")
            .fetch_one(&*store.pool)
            .await
            .expect("vec_version");
        assert!(v.0.starts_with('v'), "unexpected vec_version: {}", v.0);
    }

    fn new_goal(p: AgentId) -> NewGoal {
        NewGoal {
            agent: p,
            title: "ship goals".into(),
            detail: Some("acceptance criteria".into()),
            priority: 3,
            origin: GoalOrigin::Owner,
            origin_conv: None,
            next_review_at: None,
        }
    }

    #[tokio::test]
    async fn create_goal_and_active_goals_round_trip() {
        let s = fresh().await;
        let p = fixture_agent(&s).await;
        let conv = fixture_conv();
        s.ensure_conversation(&conv, p).await.unwrap();

        let id = s
            .create_goal(NewGoal {
                origin: GoalOrigin::SelfFormed,
                origin_conv: Some(conv.clone()),
                ..new_goal(p)
            })
            .await
            .unwrap();

        let active = s.active_goals(p).await.unwrap();
        assert_eq!(active.len(), 1);
        let g = &active[0];
        assert_eq!(g.id, id);
        assert_eq!(g.title, "ship goals");
        assert_eq!(g.detail.as_deref(), Some("acceptance criteria"));
        assert_eq!(g.status, GoalStatus::Active);
        assert_eq!(g.priority, 3);
        assert_eq!(g.origin, GoalOrigin::SelfFormed);
        assert_eq!(
            g.origin_conv
                .as_ref()
                .map(goat_types::ConversationId::to_key),
            Some(conv.to_key())
        );

        let fetched = s.get_goal(id).await.unwrap().unwrap();
        assert_eq!(fetched.id, id);
        assert_eq!(fetched.origin, GoalOrigin::SelfFormed);
        assert!(s.get_goal(9999).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn update_goal_status_removes_from_active() {
        let s = fresh().await;
        let p = fixture_agent(&s).await;

        let id = s.create_goal(new_goal(p)).await.unwrap();
        assert_eq!(s.active_goals(p).await.unwrap().len(), 1);

        s.update_goal_status(id, GoalStatus::Done).await.unwrap();
        assert!(s.active_goals(p).await.unwrap().is_empty());

        let g = s.get_goal(id).await.unwrap().unwrap();
        assert_eq!(g.status, GoalStatus::Done);
    }

    #[tokio::test]
    async fn set_goal_review_updates_next_review_at() {
        let s = fresh().await;
        let p = fixture_agent(&s).await;

        let id = s.create_goal(new_goal(p)).await.unwrap();
        let before = s.get_goal(id).await.unwrap().unwrap();
        assert!(before.next_review_at.is_none());
        assert!(before.last_reviewed_at.is_none());

        let next = Utc::now() + Duration::hours(6);
        s.set_goal_review(id, Some(next)).await.unwrap();

        let after = s.get_goal(id).await.unwrap().unwrap();
        assert!(after.last_reviewed_at.is_some());
        let stored = after.next_review_at.expect("next_review_at set");
        assert_eq!(stored.to_rfc3339(), next.to_rfc3339());
    }

    #[tokio::test]
    async fn observation_round_trip() {
        let s = fresh().await;
        let p = fixture_agent(&s).await;
        let id = s
            .record_observation(NewObservation {
                agent: p,
                integration: "linear".into(),
                account: "default".into(),
                external_ref: "linear/default:issue:US-1".into(),
                kind: "assigned".into(),
                payload: serde_json::json!({ "id": "US-1", "title": "t" }),
            })
            .await
            .unwrap();

        let record = s.get_observation(id).await.unwrap().unwrap();
        assert_eq!(record.agent, p);
        assert_eq!(record.integration, "linear");
        assert_eq!(record.external_ref, "linear/default:issue:US-1");
        assert_eq!(record.payload["title"], "t");
        assert!(s.get_observation(9999).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn observations_by_ref_returns_the_history_newest_first() {
        let s = fresh().await;
        let p = fixture_agent(&s).await;
        let other = AgentId::from_slug("other");
        s.ensure_agent(other, "other", "other").await.unwrap();

        for n in 0..3 {
            s.record_observation(NewObservation {
                agent: p,
                integration: "sentry".into(),
                account: "default".into(),
                external_ref: "sentry/default:issue:E-1".into(),
                kind: "updated".into(),
                payload: serde_json::json!({ "seen": n }),
            })
            .await
            .unwrap();
        }
        s.record_observation(NewObservation {
            agent: p,
            integration: "sentry".into(),
            account: "default".into(),
            external_ref: "sentry/default:issue:E-2".into(),
            kind: "updated".into(),
            payload: serde_json::json!({}),
        })
        .await
        .unwrap();
        s.record_observation(NewObservation {
            agent: other,
            integration: "sentry".into(),
            account: "default".into(),
            external_ref: "sentry/default:issue:E-1".into(),
            kind: "updated".into(),
            payload: serde_json::json!({ "seen": "theirs" }),
        })
        .await
        .unwrap();

        let found = s
            .observations_by_ref(p, "sentry", "sentry/default:issue:E-1", 10)
            .await
            .unwrap();
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].payload["seen"], 2);
        assert_eq!(found[2].payload["seen"], 0);

        let capped = s
            .observations_by_ref(p, "sentry", "sentry/default:issue:E-1", 2)
            .await
            .unwrap();
        assert_eq!(capped.len(), 2);

        let none = s
            .observations_by_ref(p, "sentry", "sentry/default:issue:missing", 10)
            .await
            .unwrap();
        assert!(none.is_empty());
    }

    #[tokio::test]
    async fn integration_state_round_trip() {
        let s = fresh().await;
        let p = fixture_agent(&s).await;

        assert!(
            s.integration_state(p, "linear", "default", "assigned")
                .await
                .unwrap()
                .is_none()
        );

        s.set_integration_state(p, "linear", "default", "assigned", r#"{"watermark":"a"}"#)
            .await
            .unwrap();
        s.set_integration_state(p, "linear", "default", "assigned", r#"{"watermark":"b"}"#)
            .await
            .unwrap();
        s.set_integration_state(p, "linear", "work", "assigned", r#"{"watermark":"c"}"#)
            .await
            .unwrap();

        assert_eq!(
            s.integration_state(p, "linear", "default", "assigned")
                .await
                .unwrap()
                .as_deref(),
            Some(r#"{"watermark":"b"}"#)
        );
        assert_eq!(
            s.integration_state(p, "linear", "work", "assigned")
                .await
                .unwrap()
                .as_deref(),
            Some(r#"{"watermark":"c"}"#)
        );
    }

    #[tokio::test]
    async fn populated_watch_state_survives_identity_migration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("goat.db");
        goat_sqlite_vec::register();
        let options = format!("sqlite://{}", path.display())
            .parse::<sqlx::sqlite::SqliteConnectOptions>()
            .unwrap()
            .create_if_missing(true)
            .disable_statement_logging();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE _sqlx_migrations_agent (
                 version BIGINT PRIMARY KEY,
                 description TEXT NOT NULL,
                 installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
                 success BOOLEAN NOT NULL,
                 checksum BLOB NOT NULL,
                 execution_time BIGINT NOT NULL
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        let migrator = sqlx::migrate!("./migrations");
        for migration in migrator.iter().filter(|migration| migration.version < 25) {
            sqlx::raw_sql(migration.sql.clone())
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO _sqlx_migrations_agent
                 (version, description, success, checksum, execution_time)
                 VALUES (?, ?, TRUE, ?, 0)",
            )
            .bind(migration.version)
            .bind(migration.description.as_ref())
            .bind(migration.checksum.as_ref())
            .execute(&pool)
            .await
            .unwrap();
        }
        let agent = AgentId::from_slug("migration-proof");
        sqlx::query(
            "INSERT INTO agents (id, slug, display, created_at)
             VALUES (?, 'migration-proof', 'Migration Proof', '2026-08-08T00:00:00Z')",
        )
        .bind(agent.to_string())
        .execute(&pool)
        .await
        .unwrap();
        let state = r#"{"version":2,"seen":{"ISSUE-7":"2026-08-07"},"pending":{}}"#;
        sqlx::query(
            "INSERT INTO integration_state
             (agent_id, integration, account, stream, state, updated_at)
             VALUES (?, 'sentry', 'default', 'issues', ?, '2026-08-08T01:02:03Z')",
        )
        .bind(agent.to_string())
        .bind(state)
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;

        let store = SqliteStore::open(&path).await.unwrap();
        let migrated = store
            .migrate_integration_state(
                agent,
                "sentry",
                "default",
                "issues",
                "query:is:unresolved is:for_review",
            )
            .await
            .unwrap();
        assert_eq!(migrated.as_deref(), Some(state));
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT state_key, state, updated_at FROM integration_state
             WHERE agent_id = ? AND integration = 'sentry' AND account = 'default'",
        )
        .bind(agent.to_string())
        .fetch_all(&*store.pool)
        .await
        .unwrap();
        assert_eq!(
            rows,
            vec![(
                "query:is:unresolved is:for_review".to_owned(),
                state.to_owned(),
                "2026-08-08T01:02:03Z".to_owned(),
            )]
        );
    }

    #[tokio::test]
    async fn split_migrators_adopt_a_unified_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("goat.db");
        goat_sqlite_vec::register();
        let options = format!("sqlite://{}", path.display())
            .parse::<sqlx::sqlite::SqliteConnectOptions>()
            .unwrap()
            .create_if_missing(true)
            .disable_statement_logging();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE _sqlx_migrations (
                 version BIGINT PRIMARY KEY,
                 description TEXT NOT NULL,
                 installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
                 success BOOLEAN NOT NULL,
                 checksum BLOB NOT NULL,
                 execution_time BIGINT NOT NULL
             )",
        )
        .execute(&pool)
        .await
        .unwrap();

        let agent = sqlx::migrate!("./migrations");
        let code = sqlx::migrate!("../goat-code-store/migrations");
        let memory = sqlx::migrate!("../goat-memory/migrations");
        let proxy = sqlx::migrate!("../goat-proxy-store/migrations");
        let mut migrations = agent
            .iter()
            .chain(code.iter())
            .chain(memory.iter())
            .chain(proxy.iter())
            .filter(|migration| migration.version <= 23)
            .collect::<Vec<_>>();
        migrations.sort_by_key(|migration| migration.version);
        assert_eq!(migrations.len(), 23);
        for migration in migrations {
            sqlx::raw_sql(migration.sql.clone())
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO _sqlx_migrations
                 (version, description, success, checksum, execution_time)
                 VALUES (?, ?, TRUE, ?, 0)",
            )
            .bind(migration.version)
            .bind(migration.description.as_ref())
            .bind(migration.checksum.as_ref())
            .execute(&pool)
            .await
            .unwrap();
        }

        sqlx::raw_sql(
            "INSERT INTO agents (id, slug, display, created_at)
             VALUES ('agent-1', 'main', 'Main', '2026-01-01T00:00:00Z');
             INSERT INTO threads (id, agent_id, channel, instance, external, created_at)
             VALUES ('conversation-1', 'agent-1', 'discord', '00000000-0000-0000-0000-000000000001', 'dm:1', '2026-01-01T00:00:00Z');
             INSERT INTO messages
             (id, thread_id, agent_id, direction, body_kind, text, ts, sender_kind, sender_key, attachments)
             VALUES ('message-1', 'conversation-1', 'agent-1', 'in', 'text', 'hello', '2026-01-01T00:00:01Z', 'user', 'user-1', '[]');
             INSERT INTO tool_invocations
             (id, thread_id, agent_id, call_id, tool_name, args_json, status, started_at, finished_at)
             VALUES ('tool-1', 'conversation-1', 'agent-1', 'call-1', 'memory', '{}', 'ok', '2026-01-01T00:00:02Z', '2026-01-01T00:00:03Z');
             INSERT INTO thread_summary
             (thread_id, agent_id, summary, summarized_count, updated_at)
             VALUES ('conversation-1', 'agent-1', 'summary', 1, '2026-01-01T00:00:04Z');
             INSERT INTO code_threads
             (id, cwd, title, provider, model, account, created_at, updated_at, effort)
             VALUES (1, '/repo', 'title', 'openai', 'model', 'default', 1, 2, 'high');
             INSERT INTO code_turns
             (id, thread_id, task_id, provider, model, account, status, started_at, finished_at, effort)
             VALUES (1, 1, 1, 'openai', 'model', 'default', 'done', 3, 4, 'high');
             INSERT INTO code_messages
             (id, thread_id, turn_id, role, body, created_at, parent_message_id)
             VALUES (1, 1, 1, 'user', 'hello', 5, NULL);
             UPDATE code_threads SET head_message_id = 1 WHERE id = 1;
             INSERT INTO code_tool_calls
             (id, thread_id, turn_id, call_id, name, input, status, summary, started_at, finished_at)
             VALUES (1, 1, 1, 'call-1', 'Bash', '{}', 'done', 'ok', 6, 7);
             INSERT INTO code_compactions
             (id, thread_id, summary, after_message_id, preserved_message_ids, tokens_before, tokens_after, created_at)
             VALUES (1, 1, 'compact', 1, '[]', 100, 50, 8);
             INSERT INTO code_open_prompts
             (thread_id, call_id, kind, payload, task_id, created_at)
             VALUES (1, 'call-2', 'ask', '{}', 1, 9);
             INSERT INTO code_checkpoints
             (id, thread_id, prompt_message_id, draft, attachments, created_at)
             VALUES (1, 1, 1, 'draft', '[]', 10);",
        )
        .execute(&pool)
        .await
        .unwrap();

        let before = [
            "threads",
            "messages",
            "tool_invocations",
            "thread_summary",
            "code_threads",
            "code_turns",
            "code_messages",
            "code_tool_calls",
            "code_compactions",
            "code_open_prompts",
            "code_checkpoints",
        ]
        .map(|table| sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")));
        let mut before_counts = Vec::new();
        for sql in before {
            before_counts.push(
                sqlx::query_scalar::<_, i64>(sql)
                    .fetch_one(&pool)
                    .await
                    .unwrap(),
            );
        }

        SqliteStore::open(&path).await.unwrap();
        goat_memory::MemoryEngine::open(&path, dir.path(), None, 180.0)
            .await
            .unwrap();
        goat_code_store::CodeStore::open(&path).await.unwrap();
        goat_proxy_store::ProxyStore::open(&path).await.unwrap();

        let after = [
            "conversations",
            "messages",
            "tool_invocations",
            "conversation_summary",
            "code_conversations",
            "code_turns",
            "code_messages",
            "code_tool_calls",
            "code_compactions",
            "code_open_prompts",
            "code_checkpoints",
        ]
        .map(|table| sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")));
        let mut after_counts = Vec::new();
        for sql in after {
            after_counts.push(
                sqlx::query_scalar::<_, i64>(sql)
                    .fetch_one(&pool)
                    .await
                    .unwrap(),
            );
        }
        assert_eq!(before_counts, vec![1; 11]);
        assert_eq!(after_counts, before_counts);
        let message: String = sqlx::query_scalar(
            "SELECT m.text FROM messages m
             JOIN conversations c ON c.id = m.conversation_id
             WHERE c.id = 'conversation-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(message, "hello");
        let code_message: String = sqlx::query_scalar(
            "SELECT m.body FROM code_messages m
             JOIN code_conversations c ON c.id = m.conversation_id
             WHERE c.id = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(code_message, "hello");
        let foreign_key_errors: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM pragma_foreign_key_check")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(foreign_key_errors, 0);

        for (table, expected) in [
            ("_sqlx_migrations", 23_i64),
            ("_sqlx_migrations_agent", 22),
            ("_sqlx_migrations_memory", 2),
            ("_sqlx_migrations_code", 4),
            ("_sqlx_migrations_proxy", 1),
        ] {
            let sql = sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}"));
            let count: i64 = sqlx::query_scalar(sql).fetch_one(&pool).await.unwrap();
            assert_eq!(count, expected, "{table}");
        }
    }
}
