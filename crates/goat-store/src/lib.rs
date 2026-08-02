use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use goat_types::{ChannelId, IncomingMessage, InstanceId, MessageId, ProfileId, ThreadId};
use sqlx::ConnectOptions;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use thiserror::Error;
use tracing::info;
use uuid::Uuid;

pub use goat_sqlite_vec::register as register_sqlite_vec;

mod code;
pub use code::{
    CheckpointFileVersion, CodeCheckpoint, CodeResult, CodeStore, CodeStoreError, Compaction,
    CreatedMessage, NewCheckpointFile, NewCodeCheckpoint, NewCompaction, NewMessage, NewProcess,
    NewThread, NewToolCall, NewTurn, OpenPrompt, OrphanProcess, StoredMessage, Thread,
};

mod proxy;
pub use proxy::{
    NewRequest, ProxyResult, ProxyStore, ProxyStoreError, RateLimitRow, RequestRow, Totals,
    UsageBucket,
};

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
    pub text: String,
    pub ts: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Debug)]
pub struct ThreadSummary {
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
    pub persona: ProfileId,
    pub thread: ThreadId,
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
pub enum ScheduledTaskStatus {
    Active,
    Cancelled,
    Done,
}

impl ScheduledTaskStatus {
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
pub enum TaskRunStatus {
    Pending,
    Running,
    Done,
    Failed,
    Skipped,
}

impl TaskRunStatus {
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
pub struct NewScheduledTask {
    pub persona: ProfileId,
    pub task: String,
    pub tools: Vec<String>,
    pub origin_conv: ThreadId,
    pub schedule: ScheduleKind,
    pub created_by_msg_id: Option<MessageId>,
}

#[derive(Clone, Debug)]
pub struct ScheduledTaskRecord {
    pub id: i64,
    pub persona: ProfileId,
    pub task: String,
    pub tools: Vec<String>,
    pub origin_conv: ThreadId,
    pub schedule: ScheduleKind,
    pub status: ScheduledTaskStatus,
    pub created_at: DateTime<Utc>,
    pub created_by_msg_id: Option<MessageId>,
}

#[derive(Clone, Debug)]
pub struct TaskRunRecord {
    pub id: i64,
    pub task_id: i64,
    pub task_snapshot: String,
    pub run_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status: TaskRunStatus,
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
    pub persona: ProfileId,
    pub title: String,
    pub detail: Option<String>,
    pub parent: Option<i64>,
    pub priority: i64,
    pub origin: GoalOrigin,
    pub origin_conv: Option<ThreadId>,
    pub next_review_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Clone, Debug)]
pub struct GoalRecord {
    pub id: i64,
    pub persona: ProfileId,
    pub title: String,
    pub detail: Option<String>,
    pub parent: Option<i64>,
    pub status: GoalStatus,
    pub priority: i64,
    pub origin: GoalOrigin,
    pub origin_conv: Option<ThreadId>,
    pub next_review_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_reviewed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[async_trait]
pub trait Store: Send + Sync + 'static {
    async fn ensure_persona(&self, id: ProfileId, slug: &str, display: &str) -> StoreResult<()>;

    async fn ensure_thread(&self, conv: &ThreadId, persona: ProfileId) -> StoreResult<()>;

    async fn append_incoming(&self, msg: &IncomingMessage) -> StoreResult<()>;

    async fn append_incoming_text(
        &self,
        agent: ProfileId,
        thread: &ThreadId,
        text: &str,
    ) -> StoreResult<()>;

    async fn has_agent_activity(&self, agent: ProfileId, thread: &ThreadId) -> StoreResult<bool>;

    async fn append_outgoing_text(
        &self,
        persona: ProfileId,
        conv: &ThreadId,
        text: &str,
        reply_to: Option<&MessageId>,
    ) -> StoreResult<()>;

    async fn append_tool_invocation(&self, record: ToolInvocationRecord) -> StoreResult<()>;

    async fn recent_tool_invocations(&self, limit: usize) -> StoreResult<Vec<ToolLogRow>>;

    async fn set_paused(&self, paused: bool) -> StoreResult<()>;

    async fn is_paused(&self) -> StoreResult<bool>;

    async fn recent(
        &self,
        persona: ProfileId,
        conv: &ThreadId,
        limit: usize,
    ) -> StoreResult<Vec<HistoryRow>>;

    async fn message_count(&self, persona: ProfileId, conv: &ThreadId) -> StoreResult<usize>;

    async fn messages_from(
        &self,
        persona: ProfileId,
        conv: &ThreadId,
        offset: usize,
        limit: usize,
    ) -> StoreResult<Vec<HistoryRow>>;

    async fn get_thread_summary(
        &self,
        persona: ProfileId,
        conv: &ThreadId,
    ) -> StoreResult<Option<ThreadSummary>>;

    async fn upsert_thread_summary(
        &self,
        persona: ProfileId,
        conv: &ThreadId,
        summary: &str,
        summarized_count: usize,
    ) -> StoreResult<()>;

    async fn insert_scheduled_task(&self, new: NewScheduledTask) -> StoreResult<i64>;

    async fn insert_task_run(
        &self,
        task_id: i64,
        run_at: DateTime<Utc>,
        task_snapshot: String,
    ) -> StoreResult<i64>;

    async fn claim_due_run(
        &self,
        now: DateTime<Utc>,
    ) -> StoreResult<Option<(TaskRunRecord, ScheduledTaskRecord)>>;

    async fn finish_run(
        &self,
        run_id: i64,
        status: TaskRunStatus,
        result_summary: Option<String>,
    ) -> StoreResult<()>;

    async fn cancel_task_by_id(&self, task_id: i64) -> StoreResult<bool>;

    async fn cancel_tasks_by_match(
        &self,
        persona: ProfileId,
        match_text: &str,
    ) -> StoreResult<Vec<i64>>;

    async fn list_active_tasks(
        &self,
        persona: ProfileId,
    ) -> StoreResult<Vec<(ScheduledTaskRecord, Option<DateTime<Utc>>)>>;

    async fn get_scheduled_task(&self, id: i64) -> StoreResult<Option<ScheduledTaskRecord>>;

    async fn similar_active_tasks(
        &self,
        persona: ProfileId,
        needle: &str,
    ) -> StoreResult<Vec<ScheduledTaskRecord>>;

    async fn reclaim_stale_runs(&self, stale_before: DateTime<Utc>) -> StoreResult<usize>;

    async fn cron_tasks_missing_next_run(&self) -> StoreResult<Vec<ScheduledTaskRecord>>;

    async fn all_pending_runs(&self) -> StoreResult<Vec<(i64, i64, DateTime<Utc>)>>;

    async fn latest_thread(&self, persona: ProfileId) -> StoreResult<Option<ThreadId>>;

    async fn create_goal(&self, new: NewGoal) -> StoreResult<i64>;

    async fn update_goal_status(&self, id: i64, status: GoalStatus) -> StoreResult<()>;

    async fn set_goal_review(
        &self,
        id: i64,
        next_review_at: Option<DateTime<Utc>>,
    ) -> StoreResult<()>;

    async fn active_goals(&self, persona: ProfileId) -> StoreResult<Vec<GoalRecord>>;

    async fn goals_due_for_review(&self, now: DateTime<Utc>) -> StoreResult<Vec<GoalRecord>>;

    async fn get_goal(&self, id: i64) -> StoreResult<Option<GoalRecord>>;

    async fn integration_state(
        &self,
        persona: ProfileId,
        integration: &str,
        account: &str,
        stream: &str,
    ) -> StoreResult<Option<String>>;

    async fn set_integration_state(
        &self,
        persona: ProfileId,
        integration: &str,
        account: &str,
        stream: &str,
        state: &str,
    ) -> StoreResult<()>;

    async fn record_observation(&self, new: NewObservation) -> StoreResult<i64>;

    async fn get_observation(&self, id: i64) -> StoreResult<Option<ObservationRecord>>;

    async fn observations_by_ref(
        &self,
        persona: ProfileId,
        integration: &str,
        external_ref: &str,
        limit: i64,
    ) -> StoreResult<Vec<ObservationRecord>>;
}

#[derive(Clone, Debug)]
pub struct NewObservation {
    pub persona: ProfileId,
    pub integration: String,
    pub account: String,
    pub external_ref: String,
    pub kind: String,
    pub payload: serde_json::Value,
}

#[derive(Clone, Debug)]
pub struct ObservationRecord {
    pub id: i64,
    pub persona: ProfileId,
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
        register_sqlite_vec();
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
        sqlx::migrate!("./migrations").run(&pool).await?;
        restrict_db_permissions(path);
        info!(path = %path.display(), "opened sqlite store");
        Ok(Self {
            pool: Arc::new(pool),
        })
    }

    pub fn pool(&self) -> Arc<SqlitePool> {
        self.pool.clone()
    }
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
    async fn ensure_persona(&self, id: ProfileId, slug: &str, display: &str) -> StoreResult<()> {
        sqlx::query(
            r"INSERT INTO personas (id, slug, display, created_at)
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

    async fn ensure_thread(&self, conv: &ThreadId, persona: ProfileId) -> StoreResult<()> {
        sqlx::query(
            r"INSERT INTO threads (id, persona_id, channel, instance, external, created_at)
               VALUES (?, ?, ?, ?, ?, ?)
               ON CONFLICT(id) DO NOTHING",
        )
        .bind(conv.to_key())
        .bind(persona.to_string())
        .bind(conv.channel.as_str())
        .bind(conv.instance.to_string())
        .bind(&conv.external)
        .bind(Utc::now().to_rfc3339())
        .execute(&*self.pool)
        .await?;
        Ok(())
    }

    async fn latest_thread(&self, persona: ProfileId) -> StoreResult<Option<ThreadId>> {
        let row: Option<(String, String, String)> = sqlx::query_as(
            r"SELECT c.channel, c.instance, c.external
               FROM threads c
               JOIN messages m ON m.thread_id = c.id
               WHERE c.persona_id = ?
               GROUP BY c.id
               ORDER BY MAX(m.ts) DESC
               LIMIT 1",
        )
        .bind(persona.to_string())
        .fetch_optional(&*self.pool)
        .await?;

        match row {
            None => Ok(None),
            Some((channel, instance, external)) => {
                let instance = InstanceId(Uuid::parse_str(&instance).map_err(StoreError::Uuid)?);
                Ok(Some(ThreadId::new(
                    ChannelId::new(channel),
                    instance,
                    external,
                )))
            }
        }
    }

    async fn append_incoming(&self, msg: &IncomingMessage) -> StoreResult<()> {
        self.ensure_thread(&msg.thread, msg.profile).await?;
        sqlx::query(
            r"INSERT INTO messages
               (id, thread_id, persona_id, direction, body_kind, text, attachment_ref, reply_to, ts, raw)
               VALUES (?, ?, ?, 'in', 'text', ?, NULL, NULL, ?, ?)
               ON CONFLICT(id) DO NOTHING",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(msg.thread.to_key())
        .bind(msg.profile.to_string())
        .bind(&msg.text)
        .bind(msg.ts.to_rfc3339())
        .bind(msg.raw.to_string())
        .execute(&*self.pool)
        .await?;
        Ok(())
    }

    async fn append_incoming_text(
        &self,
        agent: ProfileId,
        thread: &ThreadId,
        text: &str,
    ) -> StoreResult<()> {
        self.ensure_thread(thread, agent).await?;
        sqlx::query(
            r"INSERT INTO messages
               (id, thread_id, persona_id, direction, body_kind, text, attachment_ref, reply_to, ts, raw)
               VALUES (?, ?, ?, 'in', 'text', ?, NULL, NULL, ?, NULL)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(thread.to_key())
        .bind(agent.to_string())
        .bind(text)
        .bind(Utc::now().to_rfc3339())
        .execute(&*self.pool)
        .await?;
        Ok(())
    }

    async fn has_agent_activity(&self, agent: ProfileId, thread: &ThreadId) -> StoreResult<bool> {
        let row: (bool,) = sqlx::query_as(
            r"SELECT EXISTS(
                SELECT 1 FROM messages
                WHERE persona_id = ? AND thread_id = ? AND direction = 'out'
                LIMIT 1
            )",
        )
        .bind(agent.to_string())
        .bind(thread.to_key())
        .fetch_one(&*self.pool)
        .await?;
        Ok(row.0)
    }

    async fn append_outgoing_text(
        &self,
        persona: ProfileId,
        conv: &ThreadId,
        text: &str,
        reply_to: Option<&MessageId>,
    ) -> StoreResult<()> {
        self.ensure_thread(conv, persona).await?;
        sqlx::query(
            r"INSERT INTO messages
               (id, thread_id, persona_id, direction, body_kind, text, attachment_ref, reply_to, ts, raw)
               VALUES (?, ?, ?, 'out', 'text', ?, NULL, ?, ?, NULL)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(conv.to_key())
        .bind(persona.to_string())
        .bind(text)
        .bind(reply_to.map(|m| m.0.clone()))
        .bind(Utc::now().to_rfc3339())
        .execute(&*self.pool)
        .await?;
        Ok(())
    }

    async fn append_tool_invocation(&self, record: ToolInvocationRecord) -> StoreResult<()> {
        self.ensure_thread(&record.thread, record.persona).await?;
        sqlx::query(
            r"INSERT INTO tool_invocations
               (id, thread_id, persona_id, call_id, tool_name, args_json, status,
                output_preview, error, started_at, finished_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(record.thread.to_key())
        .bind(record.persona.to_string())
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

    async fn set_paused(&self, paused: bool) -> StoreResult<()> {
        sqlx::query(
            r"INSERT INTO runtime_flags (key, value, updated_at)
               VALUES ('paused', ?, ?)
               ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(if paused { "1" } else { "0" })
        .bind(Utc::now().to_rfc3339())
        .execute(&*self.pool)
        .await?;
        Ok(())
    }

    async fn is_paused(&self) -> StoreResult<bool> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM runtime_flags WHERE key = 'paused'")
                .fetch_optional(&*self.pool)
                .await?;
        Ok(row.is_some_and(|(v,)| v == "1"))
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
        persona: ProfileId,
        conv: &ThreadId,
        limit: usize,
    ) -> StoreResult<Vec<HistoryRow>> {
        let limit = i64::try_from(limit.max(1)).unwrap_or(i64::MAX);
        let rows = sqlx::query_as::<_, (String, String, String)>(
            r"SELECT direction, text, ts
               FROM messages
               WHERE persona_id = ? AND thread_id = ? AND text IS NOT NULL
               ORDER BY ts DESC
               LIMIT ?",
        )
        .bind(persona.to_string())
        .bind(conv.to_key())
        .bind(limit)
        .fetch_all(&*self.pool)
        .await?;

        let mut history: Vec<HistoryRow> = rows
            .into_iter()
            .map(|(dir, text, ts)| HistoryRow {
                direction: match dir.as_str() {
                    "out" => Direction::Out,
                    _ => Direction::In,
                },
                text,
                ts: chrono::DateTime::parse_from_rfc3339(&ts)
                    .map_or_else(|_| Utc::now(), |d| d.with_timezone(&Utc)),
            })
            .collect();
        history.reverse();
        Ok(history)
    }

    async fn message_count(&self, persona: ProfileId, conv: &ThreadId) -> StoreResult<usize> {
        let row: (i64,) = sqlx::query_as(
            r"SELECT COUNT(*) FROM messages
               WHERE persona_id = ? AND thread_id = ? AND text IS NOT NULL",
        )
        .bind(persona.to_string())
        .bind(conv.to_key())
        .fetch_one(&*self.pool)
        .await?;
        Ok(row.0.max(0) as usize)
    }

    async fn messages_from(
        &self,
        persona: ProfileId,
        conv: &ThreadId,
        offset: usize,
        limit: usize,
    ) -> StoreResult<Vec<HistoryRow>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let rows = sqlx::query_as::<_, (String, String, String)>(
            r"SELECT direction, text, ts
               FROM messages
               WHERE persona_id = ? AND thread_id = ? AND text IS NOT NULL
               ORDER BY ts ASC, id ASC
               LIMIT ? OFFSET ?",
        )
        .bind(persona.to_string())
        .bind(conv.to_key())
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .bind(i64::try_from(offset).unwrap_or(i64::MAX))
        .fetch_all(&*self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(dir, text, ts)| HistoryRow {
                direction: match dir.as_str() {
                    "out" => Direction::Out,
                    _ => Direction::In,
                },
                text,
                ts: chrono::DateTime::parse_from_rfc3339(&ts)
                    .map_or_else(|_| Utc::now(), |d| d.with_timezone(&Utc)),
            })
            .collect())
    }

    async fn get_thread_summary(
        &self,
        persona: ProfileId,
        conv: &ThreadId,
    ) -> StoreResult<Option<ThreadSummary>> {
        let row: Option<(String, i64)> = sqlx::query_as(
            r"SELECT summary, summarized_count FROM thread_summary
               WHERE persona_id = ? AND thread_id = ?",
        )
        .bind(persona.to_string())
        .bind(conv.to_key())
        .fetch_optional(&*self.pool)
        .await?;
        Ok(row.map(|(summary, count)| ThreadSummary {
            summary,
            summarized_count: count.max(0) as usize,
        }))
    }

    async fn upsert_thread_summary(
        &self,
        persona: ProfileId,
        conv: &ThreadId,
        summary: &str,
        summarized_count: usize,
    ) -> StoreResult<()> {
        self.ensure_thread(conv, persona).await?;
        sqlx::query(
            r"INSERT INTO thread_summary
               (thread_id, persona_id, summary, summarized_count, updated_at)
               VALUES (?, ?, ?, ?, ?)
               ON CONFLICT(thread_id) DO UPDATE SET
                 summary = excluded.summary,
                 summarized_count = excluded.summarized_count,
                 updated_at = excluded.updated_at",
        )
        .bind(conv.to_key())
        .bind(persona.to_string())
        .bind(summary)
        .bind(i64::try_from(summarized_count).unwrap_or(i64::MAX))
        .bind(Utc::now().to_rfc3339())
        .execute(&*self.pool)
        .await?;
        Ok(())
    }

    async fn insert_scheduled_task(&self, new: NewScheduledTask) -> StoreResult<i64> {
        self.ensure_thread(&new.origin_conv, new.persona).await?;
        let (kind_str, once_at, cron) = match &new.schedule {
            ScheduleKind::Once(at) => ("once", Some(at.to_rfc3339()), None),
            ScheduleKind::Cron(expr) => ("cron", None, Some(expr.clone())),
        };
        let tools_json = serde_json::to_string(&new.tools)?;
        let row: (i64,) = sqlx::query_as(
            r"INSERT INTO scheduled_tasks
               (persona_id, task, tools, origin_conv, schedule_kind, once_at,
                cron, status, created_at, created_by_msg_id)
               VALUES (?, ?, ?, ?, ?, ?, ?, 'active', ?, ?)
               RETURNING id",
        )
        .bind(new.persona.to_string())
        .bind(&new.task)
        .bind(tools_json)
        .bind(new.origin_conv.to_key())
        .bind(kind_str)
        .bind(once_at)
        .bind(cron)
        .bind(Utc::now().to_rfc3339())
        .bind(new.created_by_msg_id.as_ref().map(|m| m.0.clone()))
        .fetch_one(&*self.pool)
        .await?;
        Ok(row.0)
    }

    async fn insert_task_run(
        &self,
        task_id: i64,
        run_at: DateTime<Utc>,
        task_snapshot: String,
    ) -> StoreResult<i64> {
        let row: (i64,) = sqlx::query_as(
            r"INSERT INTO task_runs
               (task_id, task_snapshot, run_at, status, attempts)
               VALUES (?, ?, ?, 'pending', 0)
               RETURNING id",
        )
        .bind(task_id)
        .bind(&task_snapshot)
        .bind(run_at.to_rfc3339())
        .fetch_one(&*self.pool)
        .await?;
        Ok(row.0)
    }

    async fn claim_due_run(
        &self,
        now: DateTime<Utc>,
    ) -> StoreResult<Option<(TaskRunRecord, ScheduledTaskRecord)>> {
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

        let run = TaskRunRecord {
            id: row.0,
            task_id: row.1,
            task_snapshot: row.2,
            run_at: parse_ts(&row.3)?,
            started_at: row.4.as_deref().map(parse_ts).transpose()?,
            finished_at: row.5.as_deref().map(parse_ts).transpose()?,
            status: TaskRunStatus::parse(&row.6)?,
            running_since: row.7.as_deref().map(parse_ts).transpose()?,
            attempts: row.8,
            result_summary: row.9,
        };

        let task = load_scheduled_task(&self.pool, run.task_id).await?;
        Ok(Some((run, task)))
    }

    async fn finish_run(
        &self,
        run_id: i64,
        status: TaskRunStatus,
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

    async fn cancel_task_by_id(&self, task_id: i64) -> StoreResult<bool> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            r"UPDATE scheduled_tasks
               SET status = 'cancelled'
               WHERE id = ? AND status = 'active'",
        )
        .bind(task_id)
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
            .bind(task_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(changed)
    }

    async fn cancel_tasks_by_match(
        &self,
        persona: ProfileId,
        match_text: &str,
    ) -> StoreResult<Vec<i64>> {
        let pattern = format!("%{match_text}%");
        let ids: Vec<(i64,)> = sqlx::query_as(
            r"SELECT id FROM scheduled_tasks
               WHERE persona_id = ? AND status = 'active' AND task LIKE ?",
        )
        .bind(persona.to_string())
        .bind(pattern)
        .fetch_all(&*self.pool)
        .await?;
        let mut cancelled = Vec::new();
        for (id,) in ids {
            if self.cancel_task_by_id(id).await? {
                cancelled.push(id);
            }
        }
        Ok(cancelled)
    }

    async fn get_scheduled_task(&self, id: i64) -> StoreResult<Option<ScheduledTaskRecord>> {
        let exists: Option<(i64,)> = sqlx::query_as(r"SELECT id FROM scheduled_tasks WHERE id = ?")
            .bind(id)
            .fetch_optional(&*self.pool)
            .await?;
        match exists {
            Some(_) => Ok(Some(load_scheduled_task(&self.pool, id).await?)),
            None => Ok(None),
        }
    }

    async fn similar_active_tasks(
        &self,
        persona: ProfileId,
        needle: &str,
    ) -> StoreResult<Vec<ScheduledTaskRecord>> {
        let trimmed = needle.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }
        let pattern = format!("%{}%", trimmed.to_lowercase());
        let ids: Vec<(i64,)> = sqlx::query_as(
            r"SELECT id FROM scheduled_tasks
               WHERE persona_id = ? AND status = 'active'
                 AND LOWER(task) LIKE ?
               ORDER BY created_at",
        )
        .bind(persona.to_string())
        .bind(pattern)
        .fetch_all(&*self.pool)
        .await?;
        let mut out = Vec::with_capacity(ids.len());
        for (id,) in ids {
            out.push(load_scheduled_task(&self.pool, id).await?);
        }
        Ok(out)
    }

    async fn reclaim_stale_runs(&self, stale_before: DateTime<Utc>) -> StoreResult<usize> {
        let now = Utc::now().to_rfc3339();
        let stale_str = stale_before.to_rfc3339();
        let result = sqlx::query(
            r"UPDATE task_runs
               SET status = 'failed',
                   finished_at = ?,
                   running_since = NULL,
                   result_summary = COALESCE(result_summary,
                                             'lease stale: handler did not finish in time')
               WHERE status = 'running'
                 AND running_since IS NOT NULL
                 AND running_since < ?",
        )
        .bind(now)
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
        for (id, task_id, ts) in rows {
            out.push((id, task_id, parse_ts(&ts)?));
        }
        Ok(out)
    }

    async fn cron_tasks_missing_next_run(&self) -> StoreResult<Vec<ScheduledTaskRecord>> {
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
            out.push(load_scheduled_task(&self.pool, id).await?);
        }
        Ok(out)
    }

    async fn list_active_tasks(
        &self,
        persona: ProfileId,
    ) -> StoreResult<Vec<(ScheduledTaskRecord, Option<DateTime<Utc>>)>> {
        let rows: Vec<(i64,)> = sqlx::query_as(
            r"SELECT id FROM scheduled_tasks
               WHERE persona_id = ? AND status = 'active'
               ORDER BY created_at",
        )
        .bind(persona.to_string())
        .fetch_all(&*self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for (id,) in rows {
            let task = load_scheduled_task(&self.pool, id).await?;
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
            self.ensure_thread(conv, new.persona).await?;
        }
        let now = Utc::now().to_rfc3339();
        let row: (i64,) = sqlx::query_as(
            r"INSERT INTO goals
               (persona_id, title, detail, parent, status, priority, origin,
                origin_conv, next_review_at, created_at, updated_at)
               VALUES (?, ?, ?, ?, 'active', ?, ?, ?, ?, ?, ?)
               RETURNING id",
        )
        .bind(new.persona.to_string())
        .bind(&new.title)
        .bind(new.detail)
        .bind(new.parent)
        .bind(new.priority)
        .bind(new.origin.as_str())
        .bind(new.origin_conv.as_ref().map(goat_types::ThreadId::to_key))
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

    async fn active_goals(&self, persona: ProfileId) -> StoreResult<Vec<GoalRecord>> {
        let ids: Vec<(i64,)> = sqlx::query_as(
            r"SELECT id FROM goals
               WHERE persona_id = ? AND status = 'active'
               ORDER BY priority ASC, id ASC",
        )
        .bind(persona.to_string())
        .fetch_all(&*self.pool)
        .await?;
        let mut out = Vec::with_capacity(ids.len());
        for (id,) in ids {
            out.push(load_goal(&self.pool, id).await?);
        }
        Ok(out)
    }

    async fn goals_due_for_review(&self, now: DateTime<Utc>) -> StoreResult<Vec<GoalRecord>> {
        let ids: Vec<(i64,)> = sqlx::query_as(
            r"SELECT id FROM goals
               WHERE status = 'active'
                 AND next_review_at IS NOT NULL
                 AND next_review_at <= ?
               ORDER BY next_review_at",
        )
        .bind(now.to_rfc3339())
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
        persona: ProfileId,
        integration: &str,
        account: &str,
        stream: &str,
    ) -> StoreResult<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as(
            r"SELECT state FROM integration_state
               WHERE persona_id = ? AND integration = ? AND account = ? AND stream = ?",
        )
        .bind(persona.to_string())
        .bind(integration)
        .bind(account)
        .bind(stream)
        .fetch_optional(&*self.pool)
        .await?;
        Ok(row.map(|(state,)| state))
    }

    async fn set_integration_state(
        &self,
        persona: ProfileId,
        integration: &str,
        account: &str,
        stream: &str,
        state: &str,
    ) -> StoreResult<()> {
        sqlx::query(
            r"INSERT INTO integration_state
               (persona_id, integration, account, stream, state, updated_at)
               VALUES (?, ?, ?, ?, ?, ?)
               ON CONFLICT(persona_id, integration, account, stream)
               DO UPDATE SET state = excluded.state, updated_at = excluded.updated_at",
        )
        .bind(persona.to_string())
        .bind(integration)
        .bind(account)
        .bind(stream)
        .bind(state)
        .bind(Utc::now().to_rfc3339())
        .execute(&*self.pool)
        .await?;
        Ok(())
    }

    async fn record_observation(&self, new: NewObservation) -> StoreResult<i64> {
        let row: (i64,) = sqlx::query_as(
            r"INSERT INTO integration_observations
               (persona_id, integration, account, external_ref, kind, payload, observed_at)
               VALUES (?, ?, ?, ?, ?, ?, ?)
               RETURNING id",
        )
        .bind(new.persona.to_string())
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
                r"SELECT id, persona_id, integration, account, external_ref, kind,
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
            persona: ProfileId(Uuid::parse_str(&row.1)?),
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
        persona: ProfileId,
        integration: &str,
        external_ref: &str,
        limit: i64,
    ) -> StoreResult<Vec<ObservationRecord>> {
        let rows: Vec<(i64, String, String, String, String, String, String, String)> =
            sqlx::query_as(
                r"SELECT id, persona_id, integration, account, external_ref, kind,
                          payload, observed_at
                   FROM integration_observations
                   WHERE persona_id = ? AND integration = ? AND external_ref = ?
                   ORDER BY id DESC
                   LIMIT ?",
            )
            .bind(persona.to_string())
            .bind(integration)
            .bind(external_ref)
            .bind(limit.max(1))
            .fetch_all(&*self.pool)
            .await?;
        rows.into_iter()
            .map(|row| {
                Ok(ObservationRecord {
                    id: row.0,
                    persona: ProfileId(Uuid::parse_str(&row.1)?),
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
}

async fn load_scheduled_task(pool: &SqlitePool, id: i64) -> StoreResult<ScheduledTaskRecord> {
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
        String,
        String,
        Option<String>,
        String,
        String,
        String,
    ) = sqlx::query_as(
        r"SELECT s.id, s.persona_id, s.task, s.tools, s.origin_conv,
                  s.schedule_kind, s.once_at, s.cron, s.status, s.created_at,
                  s.created_by_msg_id, c.channel, c.instance, c.external
           FROM scheduled_tasks s
           JOIN threads c ON c.id = s.origin_conv
           WHERE s.id = ?",
    )
    .bind(id)
    .fetch_one(pool)
    .await?;

    let tools: Vec<String> = serde_json::from_str(&row.3)?;
    let persona = ProfileId(Uuid::parse_str(&row.1)?);
    let instance = InstanceId(Uuid::parse_str(&row.12)?);
    let origin_conv = ThreadId::new(ChannelId::new(row.11.clone()), instance, row.13.clone());
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

    Ok(ScheduledTaskRecord {
        id: row.0,
        persona,
        task: row.2,
        tools,
        origin_conv,
        schedule,
        status: ScheduledTaskStatus::parse(&row.8)?,
        created_at: parse_ts(&row.9)?,
        created_by_msg_id: row.10.map(MessageId),
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
        Option<i64>,
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
        r"SELECT g.id, g.persona_id, g.title, g.detail, g.parent, g.status,
                  g.priority, g.origin, g.next_review_at,
                  g.last_reviewed_at, g.created_at, g.updated_at,
                  c.channel, c.instance, c.external
           FROM goals g
           LEFT JOIN threads c ON c.id = g.origin_conv
           WHERE g.id = ?",
    )
    .bind(id)
    .fetch_one(pool)
    .await?;

    let persona = ProfileId(Uuid::parse_str(&row.1)?);
    let origin_conv = match (&row.12, &row.13, &row.14) {
        (Some(channel), Some(instance), Some(external)) => Some(ThreadId::new(
            ChannelId::new(channel.clone()),
            InstanceId(Uuid::parse_str(instance)?),
            external.clone(),
        )),
        _ => None,
    };
    let next_review_at = row.8.as_deref().map(parse_ts).transpose()?;
    let last_reviewed_at = row.9.as_deref().map(parse_ts).transpose()?;

    Ok(GoalRecord {
        id: row.0,
        persona,
        title: row.2,
        detail: row.3,
        parent: row.4,
        status: GoalStatus::parse(&row.5)?,
        priority: row.6,
        origin: GoalOrigin::parse(&row.7)?,
        origin_conv,
        next_review_at,
        last_reviewed_at,
        created_at: parse_ts(&row.10)?,
        updated_at: parse_ts(&row.11)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use goat_types::{ChannelId, InstanceId, Surface, UserHandle};

    async fn fresh() -> SqliteStore {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        std::mem::forget(dir);
        SqliteStore::open(&path).await.unwrap()
    }

    fn fixture_conv() -> ThreadId {
        ThreadId::new(ChannelId::new("discord"), InstanceId::new(), "chat:1")
    }

    async fn fixture_persona(store: &SqliteStore) -> ProfileId {
        let p = ProfileId::new();
        store.ensure_persona(p, "dev", "dev").await.unwrap();
        p
    }

    #[tokio::test]
    async fn ensures_and_appends() {
        let s = fresh().await;
        let p = fixture_persona(&s).await;
        let conv = fixture_conv();
        let msg = IncomingMessage {
            id: MessageId("m1".into()),
            profile: p,
            thread: conv.clone(),
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
        assert_eq!(hist[1].text, "world");
    }

    #[tokio::test]
    async fn has_agent_activity_flips_after_outgoing() {
        let s = fresh().await;
        let p = fixture_persona(&s).await;
        let conv = fixture_conv();

        s.append_incoming_text(p, &conv, "seed only").await.unwrap();
        assert!(
            !s.has_agent_activity(p, &conv).await.unwrap(),
            "incoming-only thread has no agent activity"
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
        let p = fixture_persona(&s).await;
        let conv = fixture_conv();
        for i in 0..5 {
            let msg = IncomingMessage {
                id: MessageId(format!("m{i}")),
                profile: p,
                thread: conv.clone(),
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

        assert!(s.get_thread_summary(p, &conv).await.unwrap().is_none());
        s.upsert_thread_summary(p, &conv, "they discussed msgs 0-2", 3)
            .await
            .unwrap();
        let summary = s.get_thread_summary(p, &conv).await.unwrap().unwrap();
        assert_eq!(summary.summarized_count, 3);
        assert_eq!(summary.summary, "they discussed msgs 0-2");

        s.upsert_thread_summary(p, &conv, "updated", 4)
            .await
            .unwrap();
        let summary = s.get_thread_summary(p, &conv).await.unwrap().unwrap();
        assert_eq!(summary.summarized_count, 4);
        assert_eq!(summary.summary, "updated");
    }

    #[tokio::test]
    async fn schedule_once_insert_and_list() {
        let s = fresh().await;
        let p = fixture_persona(&s).await;
        let conv = fixture_conv();
        s.ensure_thread(&conv, p).await.unwrap();
        let due = Utc::now() + Duration::minutes(5);
        let task_id = s
            .insert_scheduled_task(NewScheduledTask {
                persona: p,
                task: "ping example.com".into(),
                tools: vec!["shell".into()],
                origin_conv: conv.clone(),
                schedule: ScheduleKind::Once(due),
                created_by_msg_id: None,
            })
            .await
            .unwrap();
        s.insert_task_run(task_id, due, "ping example.com".into())
            .await
            .unwrap();

        let listed = s.list_active_tasks(p).await.unwrap();
        assert_eq!(listed.len(), 1);
        let (task, next_at) = &listed[0];
        assert_eq!(task.id, task_id);
        assert_eq!(task.task, "ping example.com");
        assert!(matches!(task.schedule, ScheduleKind::Once(_)));
        assert!(next_at.is_some());
    }

    #[tokio::test]
    async fn claim_due_run_is_atomic() {
        let s = fresh().await;
        let p = fixture_persona(&s).await;
        let conv = fixture_conv();
        s.ensure_thread(&conv, p).await.unwrap();
        let past = Utc::now() - Duration::minutes(1);
        let task_id = s
            .insert_scheduled_task(NewScheduledTask {
                persona: p,
                task: "old task".into(),
                tools: vec![],
                origin_conv: conv,
                schedule: ScheduleKind::Once(past),
                created_by_msg_id: None,
            })
            .await
            .unwrap();
        s.insert_task_run(task_id, past, "old task".into())
            .await
            .unwrap();

        let first = s.claim_due_run(Utc::now()).await.unwrap();
        assert!(first.is_some(), "first claim should succeed");
        let second = s.claim_due_run(Utc::now()).await.unwrap();
        assert!(
            second.is_none(),
            "second claim should find no pending run after first claimed"
        );

        let (run, task) = first.unwrap();
        assert_eq!(run.task_id, task_id);
        assert_eq!(run.status, TaskRunStatus::Running);
        assert_eq!(task.task, "old task");

        s.finish_run(run.id, TaskRunStatus::Done, Some("ok".into()))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn cancel_by_match_purges_pending() {
        let s = fresh().await;
        let p = fixture_persona(&s).await;
        let conv = fixture_conv();
        s.ensure_thread(&conv, p).await.unwrap();
        let due = Utc::now() + Duration::minutes(1);
        let task_id = s
            .insert_scheduled_task(NewScheduledTask {
                persona: p,
                task: "run loadtest in staging".into(),
                tools: vec![],
                origin_conv: conv,
                schedule: ScheduleKind::Once(due),
                created_by_msg_id: None,
            })
            .await
            .unwrap();
        s.insert_task_run(task_id, due, "run loadtest in staging".into())
            .await
            .unwrap();

        let cancelled = s.cancel_tasks_by_match(p, "loadtest").await.unwrap();
        assert_eq!(cancelled, vec![task_id]);

        let claim = s
            .claim_due_run(Utc::now() + Duration::minutes(2))
            .await
            .unwrap();
        assert!(claim.is_none(), "cancelled task's run must not be claimed");

        let active = s.list_active_tasks(p).await.unwrap();
        assert!(active.is_empty(), "cancelled task must drop out of list");
    }

    #[tokio::test]
    async fn reclaim_stale_runs_marks_failed_only_past_threshold() {
        let s = fresh().await;
        let p = fixture_persona(&s).await;
        let conv = fixture_conv();
        s.ensure_thread(&conv, p).await.unwrap();
        let past = Utc::now() - Duration::minutes(30);
        let task_id = s
            .insert_scheduled_task(NewScheduledTask {
                persona: p,
                task: "x".into(),
                tools: vec![],
                origin_conv: conv,
                schedule: ScheduleKind::Once(past),
                created_by_msg_id: None,
            })
            .await
            .unwrap();
        s.insert_task_run(task_id, past, "x".into()).await.unwrap();
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
        let p = fixture_persona(&s).await;
        let conv = fixture_conv();
        s.ensure_thread(&conv, p).await.unwrap();

        let task_id = s
            .insert_scheduled_task(NewScheduledTask {
                persona: p,
                task: "weekly".into(),
                tools: vec![],
                origin_conv: conv,
                schedule: ScheduleKind::Cron("0 7 * * 1".into()),
                created_by_msg_id: None,
            })
            .await
            .unwrap();

        let missing = s.cron_tasks_missing_next_run().await.unwrap();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].id, task_id);

        s.insert_task_run(task_id, Utc::now() + Duration::minutes(1), "weekly".into())
            .await
            .unwrap();
        let missing = s.cron_tasks_missing_next_run().await.unwrap();
        assert!(missing.is_empty());
    }

    #[tokio::test]
    async fn cron_task_round_trip() {
        let s = fresh().await;
        let p = fixture_persona(&s).await;
        let conv = fixture_conv();
        s.ensure_thread(&conv, p).await.unwrap();
        let task_id = s
            .insert_scheduled_task(NewScheduledTask {
                persona: p,
                task: "weekly summary".into(),
                tools: vec!["read".into(), "grep".into()],
                origin_conv: conv,
                schedule: ScheduleKind::Cron("0 7 * * 1".into()),
                created_by_msg_id: None,
            })
            .await
            .unwrap();
        let active = s.list_active_tasks(p).await.unwrap();
        let (task, _) = &active[0];
        assert_eq!(task.id, task_id);
        match &task.schedule {
            ScheduleKind::Cron(expr) => assert_eq!(expr, "0 7 * * 1"),
            ScheduleKind::Once(_) => panic!("expected cron schedule"),
        }
        assert_eq!(task.tools, vec!["read".to_string(), "grep".to_string()]);
    }

    #[tokio::test]
    async fn latest_thread_returns_most_recent() {
        let s = fresh().await;
        let p = fixture_persona(&s).await;

        assert!(s.latest_thread(p).await.unwrap().is_none());

        let conv_old = ThreadId::new(ChannelId::new("discord"), InstanceId::new(), "chat:old");
        let conv_new = ThreadId::new(ChannelId::new("discord"), InstanceId::new(), "chat:new");

        let earlier = Utc::now() - Duration::seconds(60);
        let later = Utc::now();

        s.append_incoming(&IncomingMessage {
            id: MessageId("m-old".into()),
            profile: p,
            thread: conv_old.clone(),
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
            profile: p,
            thread: conv_new.clone(),
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

        let latest = s.latest_thread(p).await.unwrap().unwrap();
        assert_eq!(latest.external, conv_new.external);
    }

    #[tokio::test]
    async fn sqlite_vec_extension_is_available() {
        let store = fresh().await;
        let pool = store.pool();
        let v: (String,) = sqlx::query_as("SELECT vec_version()")
            .fetch_one(&*pool)
            .await
            .expect("vec_version");
        assert!(v.0.starts_with('v'), "unexpected vec_version: {}", v.0);
    }

    fn new_goal(p: ProfileId) -> NewGoal {
        NewGoal {
            persona: p,
            title: "ship goals".into(),
            detail: Some("acceptance criteria".into()),
            parent: None,
            priority: 3,
            origin: GoalOrigin::Owner,
            origin_conv: None,
            next_review_at: None,
        }
    }

    #[tokio::test]
    async fn create_goal_and_active_goals_round_trip() {
        let s = fresh().await;
        let p = fixture_persona(&s).await;
        let conv = fixture_conv();
        s.ensure_thread(&conv, p).await.unwrap();

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
            g.origin_conv.as_ref().map(goat_types::ThreadId::to_key),
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
        let p = fixture_persona(&s).await;

        let id = s.create_goal(new_goal(p)).await.unwrap();
        assert_eq!(s.active_goals(p).await.unwrap().len(), 1);

        s.update_goal_status(id, GoalStatus::Done).await.unwrap();
        assert!(s.active_goals(p).await.unwrap().is_empty());

        let g = s.get_goal(id).await.unwrap().unwrap();
        assert_eq!(g.status, GoalStatus::Done);
    }

    #[tokio::test]
    async fn goals_due_for_review_returns_only_due_active() {
        let s = fresh().await;
        let p = fixture_persona(&s).await;
        let now = Utc::now();

        let due_past = s
            .create_goal(NewGoal {
                title: "past".into(),
                next_review_at: Some(now - Duration::hours(1)),
                ..new_goal(p)
            })
            .await
            .unwrap();

        s.create_goal(NewGoal {
            title: "future".into(),
            next_review_at: Some(now + Duration::hours(1)),
            ..new_goal(p)
        })
        .await
        .unwrap();

        let done = s
            .create_goal(NewGoal {
                title: "done".into(),
                next_review_at: Some(now - Duration::hours(2)),
                ..new_goal(p)
            })
            .await
            .unwrap();
        s.update_goal_status(done, GoalStatus::Done).await.unwrap();

        let due = s.goals_due_for_review(now).await.unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, due_past);
        assert_eq!(due[0].title, "past");
    }

    #[tokio::test]
    async fn set_goal_review_updates_next_review_at() {
        let s = fresh().await;
        let p = fixture_persona(&s).await;

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
        let p = fixture_persona(&s).await;
        let id = s
            .record_observation(NewObservation {
                persona: p,
                integration: "linear".into(),
                account: "default".into(),
                external_ref: "linear/default:issue:US-1".into(),
                kind: "assigned".into(),
                payload: serde_json::json!({ "id": "US-1", "title": "t" }),
            })
            .await
            .unwrap();

        let record = s.get_observation(id).await.unwrap().unwrap();
        assert_eq!(record.persona, p);
        assert_eq!(record.integration, "linear");
        assert_eq!(record.external_ref, "linear/default:issue:US-1");
        assert_eq!(record.payload["title"], "t");
        assert!(s.get_observation(9999).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn observations_by_ref_returns_the_history_newest_first() {
        let s = fresh().await;
        let p = fixture_persona(&s).await;
        let other = ProfileId::from_slug("other");
        s.ensure_persona(other, "other", "other").await.unwrap();

        for n in 0..3 {
            s.record_observation(NewObservation {
                persona: p,
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
            persona: p,
            integration: "sentry".into(),
            account: "default".into(),
            external_ref: "sentry/default:issue:E-2".into(),
            kind: "updated".into(),
            payload: serde_json::json!({}),
        })
        .await
        .unwrap();
        s.record_observation(NewObservation {
            persona: other,
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
        let p = fixture_persona(&s).await;

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
}
