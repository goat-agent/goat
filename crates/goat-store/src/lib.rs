use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use goat_types::{ConversationId, IncomingMessage, MessageId, PersonaId};
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
}

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Clone, Debug)]
pub struct HistoryRow {
    pub direction: Direction,
    pub text: String,
    pub ts: chrono::DateTime<chrono::Utc>,
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
            .disable_statement_logging();
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(opts)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        info!(path = %path.display(), "opened sqlite store");
        Ok(Self {
            pool: Arc::new(pool),
        })
    }
}

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use goat_types::{ChannelId, InstanceId, UserHandle};

    async fn fresh() -> SqliteStore {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        std::mem::forget(dir);
        SqliteStore::open(&path).await.unwrap()
    }

    #[tokio::test]
    async fn ensures_and_appends() {
        let s = fresh().await;
        let p = PersonaId::new();
        s.ensure_persona(p, "dev", "dev").await.unwrap();
        let conv = ConversationId::new(ChannelId::new("telegram"), InstanceId::new(), "x");
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
}
