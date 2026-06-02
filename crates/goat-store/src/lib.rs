use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use goat_types::{ChannelId, ConversationId, IncomingMessage, InstanceId, MessageId, PersonaId};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use sqlx::ConnectOptions;
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
    pub text: String,
    pub ts: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Debug)]
pub struct ConversationSummary {
    pub summary: String,
    /// Number of text messages (ts-ascending) already folded into `summary`.
    pub summarized_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    In,
    Out,
}

#[derive(Clone, Debug)]
pub struct ToolInvocationRecord {
    pub persona: PersonaId,
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
    pub persona: PersonaId,
    pub task: String,
    pub tools: Vec<String>,
    pub origin_conv: ConversationId,
    pub schedule: ScheduleKind,
    pub created_by_msg_id: Option<MessageId>,
}

#[derive(Clone, Debug)]
pub struct ScheduledTaskRecord {
    pub id: i64,
    pub persona: PersonaId,
    pub task: String,
    pub tools: Vec<String>,
    pub origin_conv: ConversationId,
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

#[async_trait]
pub trait Store: Send + Sync + 'static {
    async fn ensure_persona(&self, id: PersonaId, slug: &str, display: &str) -> StoreResult<()>;

    async fn ensure_conversation(
        &self,
        conv: &ConversationId,
        persona: PersonaId,
    ) -> StoreResult<()>;

    async fn append_incoming(&self, msg: &IncomingMessage) -> StoreResult<()>;

    async fn append_outgoing_text(
        &self,
        persona: PersonaId,
        conv: &ConversationId,
        text: &str,
        reply_to: Option<&MessageId>,
    ) -> StoreResult<()>;

    async fn append_tool_invocation(&self, record: ToolInvocationRecord) -> StoreResult<()>;

    async fn recent(
        &self,
        persona: PersonaId,
        conv: &ConversationId,
        limit: usize,
    ) -> StoreResult<Vec<HistoryRow>>;

    /// Count of text messages in a conversation. Used as the stable upper
    /// bound for summary-watermark math.
    async fn message_count(&self, persona: PersonaId, conv: &ConversationId) -> StoreResult<usize>;

    /// Text messages in ts-ascending order, `limit` rows starting at `offset`.
    /// Offsets are stable because messages are append-only.
    async fn messages_from(
        &self,
        persona: PersonaId,
        conv: &ConversationId,
        offset: usize,
        limit: usize,
    ) -> StoreResult<Vec<HistoryRow>>;

    async fn get_conversation_summary(
        &self,
        persona: PersonaId,
        conv: &ConversationId,
    ) -> StoreResult<Option<ConversationSummary>>;

    async fn upsert_conversation_summary(
        &self,
        persona: PersonaId,
        conv: &ConversationId,
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
        persona: PersonaId,
        match_text: &str,
    ) -> StoreResult<Vec<i64>>;

    async fn list_active_tasks(
        &self,
        persona: PersonaId,
    ) -> StoreResult<Vec<(ScheduledTaskRecord, Option<DateTime<Utc>>)>>;

    /// Fetches a single scheduled task by id, regardless of status. Returns
    /// `None` if the task doesn't exist.
    async fn get_scheduled_task(&self, id: i64) -> StoreResult<Option<ScheduledTaskRecord>>;

    /// Active tasks for `persona` whose `task` contains the given
    /// substring (case-insensitive). Used by tools to surface a soft
    /// "looks similar" warning before allowing a duplicate registration.
    async fn similar_active_tasks(
        &self,
        persona: PersonaId,
        needle: &str,
    ) -> StoreResult<Vec<ScheduledTaskRecord>>;

    /// Marks `running` runs whose `running_since` is older than
    /// `stale_before` as `failed`, freeing the lease. Returns the number
    /// of affected rows.
    async fn reclaim_stale_runs(&self, stale_before: DateTime<Utc>) -> StoreResult<usize>;

    /// Returns all active `cron` tasks that currently have no `pending`
    /// run row. The caller is expected to compute the next occurrence and
    /// insert a fresh run.
    async fn cron_tasks_missing_next_run(&self) -> StoreResult<Vec<ScheduledTaskRecord>>;

    /// Returns every `pending` `task_runs` row as `(run_id, task_id, run_at)`,
    /// ordered by `run_at`. Used by the scheduler at boot to repopulate the
    /// in-process timer queue from durable state.
    async fn all_pending_runs(&self) -> StoreResult<Vec<(i64, i64, DateTime<Utc>)>>;
}

#[derive(Clone)]
pub struct SqliteStore {
    pool: Arc<SqlitePool>,
}

impl SqliteStore {
    pub async fn open(path: &Path) -> StoreResult<Self> {
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
                .map(|s| s.to_os_string())
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
    async fn ensure_persona(&self, id: PersonaId, slug: &str, display: &str) -> StoreResult<()> {
        sqlx::query(
            r#"INSERT INTO personas (id, slug, display, created_at)
               VALUES (?, ?, ?, ?)
               ON CONFLICT(id) DO UPDATE SET slug = excluded.slug, display = excluded.display"#,
        )
        .bind(id.to_string())
        .bind(slug)
        .bind(display)
        .bind(Utc::now().to_rfc3339())
        .execute(&*self.pool)
        .await?;
        Ok(())
    }

    async fn ensure_conversation(
        &self,
        conv: &ConversationId,
        persona: PersonaId,
    ) -> StoreResult<()> {
        sqlx::query(
            r#"INSERT INTO conversations (id, persona_id, channel, instance, external, created_at)
               VALUES (?, ?, ?, ?, ?, ?)
               ON CONFLICT(id) DO NOTHING"#,
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

    async fn append_incoming(&self, msg: &IncomingMessage) -> StoreResult<()> {
        self.ensure_conversation(&msg.conversation, msg.persona)
            .await?;
        sqlx::query(
            r#"INSERT INTO messages
               (id, conversation_id, persona_id, direction, body_kind, text, attachment_ref, reply_to, ts, raw)
               VALUES (?, ?, ?, 'in', 'text', ?, NULL, NULL, ?, ?)
               ON CONFLICT(id) DO NOTHING"#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(msg.conversation.to_key())
        .bind(msg.persona.to_string())
        .bind(&msg.text)
        .bind(msg.ts.to_rfc3339())
        .bind(msg.raw.to_string())
        .execute(&*self.pool)
        .await?;
        Ok(())
    }

    async fn append_outgoing_text(
        &self,
        persona: PersonaId,
        conv: &ConversationId,
        text: &str,
        reply_to: Option<&MessageId>,
    ) -> StoreResult<()> {
        self.ensure_conversation(conv, persona).await?;
        sqlx::query(
            r#"INSERT INTO messages
               (id, conversation_id, persona_id, direction, body_kind, text, attachment_ref, reply_to, ts, raw)
               VALUES (?, ?, ?, 'out', 'text', ?, NULL, ?, ?, NULL)"#,
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
        self.ensure_conversation(&record.conversation, record.persona)
            .await?;
        sqlx::query(
            r#"INSERT INTO tool_invocations
               (id, conversation_id, persona_id, call_id, tool_name, args_json, status,
                output_preview, error, started_at, finished_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(record.conversation.to_key())
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

    async fn recent(
        &self,
        persona: PersonaId,
        conv: &ConversationId,
        limit: usize,
    ) -> StoreResult<Vec<HistoryRow>> {
        let limit = limit.max(1) as i64;
        let rows = sqlx::query_as::<_, (String, String, String)>(
            r#"SELECT direction, text, ts
               FROM messages
               WHERE persona_id = ? AND conversation_id = ? AND text IS NOT NULL
               ORDER BY ts DESC
               LIMIT ?"#,
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
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            })
            .collect();
        history.reverse();
        Ok(history)
    }

    async fn message_count(&self, persona: PersonaId, conv: &ConversationId) -> StoreResult<usize> {
        let row: (i64,) = sqlx::query_as(
            r#"SELECT COUNT(*) FROM messages
               WHERE persona_id = ? AND conversation_id = ? AND text IS NOT NULL"#,
        )
        .bind(persona.to_string())
        .bind(conv.to_key())
        .fetch_one(&*self.pool)
        .await?;
        Ok(row.0.max(0) as usize)
    }

    async fn messages_from(
        &self,
        persona: PersonaId,
        conv: &ConversationId,
        offset: usize,
        limit: usize,
    ) -> StoreResult<Vec<HistoryRow>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let rows = sqlx::query_as::<_, (String, String, String)>(
            r#"SELECT direction, text, ts
               FROM messages
               WHERE persona_id = ? AND conversation_id = ? AND text IS NOT NULL
               ORDER BY ts ASC, id ASC
               LIMIT ? OFFSET ?"#,
        )
        .bind(persona.to_string())
        .bind(conv.to_key())
        .bind(limit as i64)
        .bind(offset as i64)
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
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            })
            .collect())
    }

    async fn get_conversation_summary(
        &self,
        persona: PersonaId,
        conv: &ConversationId,
    ) -> StoreResult<Option<ConversationSummary>> {
        let row: Option<(String, i64)> = sqlx::query_as(
            r#"SELECT summary, summarized_count FROM conversation_summary
               WHERE persona_id = ? AND conversation_id = ?"#,
        )
        .bind(persona.to_string())
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
        persona: PersonaId,
        conv: &ConversationId,
        summary: &str,
        summarized_count: usize,
    ) -> StoreResult<()> {
        self.ensure_conversation(conv, persona).await?;
        sqlx::query(
            r#"INSERT INTO conversation_summary
               (conversation_id, persona_id, summary, summarized_count, updated_at)
               VALUES (?, ?, ?, ?, ?)
               ON CONFLICT(conversation_id) DO UPDATE SET
                 summary = excluded.summary,
                 summarized_count = excluded.summarized_count,
                 updated_at = excluded.updated_at"#,
        )
        .bind(conv.to_key())
        .bind(persona.to_string())
        .bind(summary)
        .bind(summarized_count as i64)
        .bind(Utc::now().to_rfc3339())
        .execute(&*self.pool)
        .await?;
        Ok(())
    }

    async fn insert_scheduled_task(&self, new: NewScheduledTask) -> StoreResult<i64> {
        self.ensure_conversation(&new.origin_conv, new.persona)
            .await?;
        let (kind_str, once_at, cron) = match &new.schedule {
            ScheduleKind::Once(at) => ("once", Some(at.to_rfc3339()), None),
            ScheduleKind::Cron(expr) => ("cron", None, Some(expr.clone())),
        };
        let tools_json = serde_json::to_string(&new.tools)?;
        let row: (i64,) = sqlx::query_as(
            r#"INSERT INTO scheduled_tasks
               (persona_id, task, tools, origin_conv, schedule_kind, once_at,
                cron, status, created_at, created_by_msg_id)
               VALUES (?, ?, ?, ?, ?, ?, ?, 'active', ?, ?)
               RETURNING id"#,
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
            r#"INSERT INTO task_runs
               (task_id, task_snapshot, run_at, status, attempts)
               VALUES (?, ?, ?, 'pending', 0)
               RETURNING id"#,
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
            r#"UPDATE task_runs
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
                         finished_at, status, running_since, attempts, result_summary"#,
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
            r#"UPDATE task_runs
               SET status = ?, finished_at = ?, running_since = NULL, result_summary = ?
               WHERE id = ?"#,
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
            r#"UPDATE scheduled_tasks
               SET status = 'cancelled'
               WHERE id = ? AND status = 'active'"#,
        )
        .bind(task_id)
        .execute(&mut *tx)
        .await?;
        let changed = result.rows_affected() > 0;
        if changed {
            sqlx::query(
                r#"UPDATE task_runs
                   SET status = 'skipped', finished_at = ?
                   WHERE task_id = ? AND status = 'pending'"#,
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
        persona: PersonaId,
        match_text: &str,
    ) -> StoreResult<Vec<i64>> {
        let pattern = format!("%{}%", match_text);
        let ids: Vec<(i64,)> = sqlx::query_as(
            r#"SELECT id FROM scheduled_tasks
               WHERE persona_id = ? AND status = 'active' AND task LIKE ?"#,
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
        let exists: Option<(i64,)> =
            sqlx::query_as(r#"SELECT id FROM scheduled_tasks WHERE id = ?"#)
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
        persona: PersonaId,
        needle: &str,
    ) -> StoreResult<Vec<ScheduledTaskRecord>> {
        let trimmed = needle.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }
        let pattern = format!("%{}%", trimmed.to_lowercase());
        let ids: Vec<(i64,)> = sqlx::query_as(
            r#"SELECT id FROM scheduled_tasks
               WHERE persona_id = ? AND status = 'active'
                 AND LOWER(task) LIKE ?
               ORDER BY created_at"#,
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
            r#"UPDATE task_runs
               SET status = 'failed',
                   finished_at = ?,
                   running_since = NULL,
                   result_summary = COALESCE(result_summary,
                                             'lease stale: handler did not finish in time')
               WHERE status = 'running'
                 AND running_since IS NOT NULL
                 AND running_since < ?"#,
        )
        .bind(now)
        .bind(stale_str)
        .execute(&*self.pool)
        .await?;
        Ok(result.rows_affected() as usize)
    }

    async fn all_pending_runs(&self) -> StoreResult<Vec<(i64, i64, DateTime<Utc>)>> {
        let rows: Vec<(i64, i64, String)> = sqlx::query_as(
            r#"SELECT id, task_id, run_at FROM task_runs
               WHERE status = 'pending'
               ORDER BY run_at"#,
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
            r#"SELECT s.id FROM scheduled_tasks s
               WHERE s.status = 'active' AND s.schedule_kind = 'cron'
                 AND NOT EXISTS (
                     SELECT 1 FROM task_runs r
                     WHERE r.task_id = s.id AND r.status = 'pending'
                 )
               ORDER BY s.id"#,
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
        persona: PersonaId,
    ) -> StoreResult<Vec<(ScheduledTaskRecord, Option<DateTime<Utc>>)>> {
        let rows: Vec<(i64,)> = sqlx::query_as(
            r#"SELECT id FROM scheduled_tasks
               WHERE persona_id = ? AND status = 'active'
               ORDER BY created_at"#,
        )
        .bind(persona.to_string())
        .fetch_all(&*self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for (id,) in rows {
            let task = load_scheduled_task(&self.pool, id).await?;
            let next: Option<(String,)> = sqlx::query_as(
                r#"SELECT run_at FROM task_runs
                   WHERE task_id = ? AND status = 'pending'
                   ORDER BY run_at
                   LIMIT 1"#,
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
        r#"SELECT s.id, s.persona_id, s.task, s.tools, s.origin_conv,
                  s.schedule_kind, s.once_at, s.cron, s.status, s.created_at,
                  s.created_by_msg_id, c.channel, c.instance, c.external
           FROM scheduled_tasks s
           JOIN conversations c ON c.id = s.origin_conv
           WHERE s.id = ?"#,
    )
    .bind(id)
    .fetch_one(pool)
    .await?;

    let tools: Vec<String> = serde_json::from_str(&row.3)?;
    let persona = PersonaId(Uuid::parse_str(&row.1)?);
    let instance = InstanceId(Uuid::parse_str(&row.12)?);
    let origin_conv = ConversationId::new(ChannelId::new(row.11.clone()), instance, row.13.clone());
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
            })
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use goat_types::{ChannelId, InstanceId, UserHandle};

    async fn fresh() -> SqliteStore {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        std::mem::forget(dir);
        SqliteStore::open(&path).await.unwrap()
    }

    fn fixture_conv() -> ConversationId {
        ConversationId::new(ChannelId::new("telegram"), InstanceId::new(), "chat:1")
    }

    async fn fixture_persona(store: &SqliteStore) -> PersonaId {
        let p = PersonaId::new();
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
            persona: p,
            conversation: conv.clone(),
            from: UserHandle {
                external: "u".into(),
                display: None,
            },
            text: "hello".into(),
            attachments: vec![],
            command: None,
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
    async fn message_count_range_and_summary_round_trip() {
        let s = fresh().await;
        let p = fixture_persona(&s).await;
        let conv = fixture_conv();
        for i in 0..5 {
            let msg = IncomingMessage {
                id: MessageId(format!("m{i}")),
                persona: p,
                conversation: conv.clone(),
                from: UserHandle {
                    external: "u".into(),
                    display: None,
                },
                text: format!("msg {i}"),
                attachments: vec![],
                command: None,
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

        assert!(s
            .get_conversation_summary(p, &conv)
            .await
            .unwrap()
            .is_none());
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
        let p = fixture_persona(&s).await;
        let conv = fixture_conv();
        s.ensure_conversation(&conv, p).await.unwrap();
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
        s.ensure_conversation(&conv, p).await.unwrap();
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
        s.ensure_conversation(&conv, p).await.unwrap();
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
        s.ensure_conversation(&conv, p).await.unwrap();
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
        s.ensure_conversation(&conv, p).await.unwrap();

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
        s.ensure_conversation(&conv, p).await.unwrap();
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
            _ => panic!("expected cron schedule"),
        }
        assert_eq!(task.tools, vec!["read".to_string(), "grep".to_string()]);
    }
}
