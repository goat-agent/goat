use std::path::Path;
use std::time::Duration;

use sqlx::ConnectOptions;
use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

const READER_POOL_MAX: u32 = 4;

#[derive(Debug, thiserror::Error)]
pub enum CodeStoreError {
    #[error("sqlx: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("migrate: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type CodeResult<T> = Result<T, CodeStoreError>;

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Thread {
    pub id: i64,
    pub cwd: String,
    pub title: Option<String>,
    pub provider: String,
    pub model: String,
    pub account: String,
    pub effort: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewThread {
    pub cwd: String,
    pub title: Option<String>,
    pub provider: String,
    pub model: String,
    pub account: String,
    pub effort: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTurn {
    pub thread_id: i64,
    pub task_id: i64,
    pub provider: String,
    pub model: String,
    pub account: String,
    pub effort: Option<String>,
    pub status: String,
    pub started_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMessage {
    pub thread_id: i64,
    pub turn_id: Option<i64>,
    pub role: String,
    pub body: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct StoredMessage {
    pub id: i64,
    pub turn_id: Option<i64>,
    pub role: String,
    pub body: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCompaction {
    pub thread_id: i64,
    pub summary: String,
    pub after_message_id: i64,
    pub tail_from_message_id: Option<i64>,
    pub preserved_message_ids: Vec<i64>,
    pub tokens_before: i64,
    pub tokens_after: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compaction {
    pub id: i64,
    pub thread_id: i64,
    pub summary: String,
    pub after_message_id: i64,
    pub tail_from_message_id: Option<i64>,
    pub preserved_message_ids: Vec<i64>,
    pub tokens_before: i64,
    pub tokens_after: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenPrompt {
    pub call_id: String,
    pub kind: String,
    pub payload: String,
    pub task_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewToolCall {
    pub thread_id: i64,
    pub turn_id: i64,
    pub call_id: String,
    pub name: String,
    pub input: String,
    pub status: String,
    pub started_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewProcess {
    pub pgid: i64,
    pub command: String,
    pub cwd: String,
    pub started_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrphanProcess {
    pub id: i64,
    pub pgid: i64,
    pub command: String,
}

#[derive(Clone)]
pub struct CodeStore {
    writer: SqlitePool,
    readers: SqlitePool,
}

fn connect_opts(path: &Path) -> CodeResult<SqliteConnectOptions> {
    let opts = format!("sqlite://{}", path.display())
        .parse::<SqliteConnectOptions>()?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5))
        .disable_statement_logging();
    Ok(opts)
}

impl CodeStore {
    pub async fn open(path: &Path) -> CodeResult<Self> {
        crate::register_sqlite_vec();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let writer = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(connect_opts(path)?)
            .await?;
        sqlx::migrate!("./migrations").run(&writer).await?;
        let readers = SqlitePoolOptions::new()
            .max_connections(READER_POOL_MAX)
            .connect_with(connect_opts(path)?.read_only(true))
            .await?;
        Ok(Self { writer, readers })
    }

    pub async fn open_in_memory() -> CodeResult<Self> {
        crate::register_sqlite_vec();
        let opts = "sqlite::memory:"
            .parse::<SqliteConnectOptions>()?
            .disable_statement_logging();
        let writer = SqlitePoolOptions::new()
            .max_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .acquire_timeout(Duration::from_hours(1))
            .test_before_acquire(false)
            .connect_with(opts)
            .await?;
        sqlx::migrate!("./migrations").run(&writer).await?;
        let readers = writer.clone();
        Ok(Self { writer, readers })
    }

    pub async fn create_thread(&self, thread: NewThread) -> CodeResult<i64> {
        let id = sqlx::query(
            "INSERT INTO code_threads (cwd, title, provider, model, account, effort, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(thread.cwd)
        .bind(thread.title)
        .bind(thread.provider)
        .bind(thread.model)
        .bind(thread.account)
        .bind(thread.effort)
        .bind(thread.created_at)
        .bind(thread.updated_at)
        .execute(&self.writer)
        .await?
        .last_insert_rowid();
        Ok(id)
    }

    pub async fn get_thread(&self, id: i64) -> CodeResult<Option<Thread>> {
        let thread = sqlx::query_as::<_, Thread>(
            "SELECT id, cwd, title, provider, model, account, effort, created_at, updated_at
             FROM code_threads WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.readers)
        .await?;
        Ok(thread)
    }

    pub async fn latest_thread_in(&self, cwd: String) -> CodeResult<Option<Thread>> {
        let thread = sqlx::query_as::<_, Thread>(
            "SELECT id, cwd, title, provider, model, account, effort, created_at, updated_at
             FROM code_threads WHERE cwd = ? ORDER BY updated_at DESC, id DESC LIMIT 1",
        )
        .bind(cwd)
        .fetch_optional(&self.readers)
        .await?;
        Ok(thread)
    }

    pub async fn list_threads_in(&self, cwd: String, limit: i64) -> CodeResult<Vec<Thread>> {
        let threads = sqlx::query_as::<_, Thread>(
            "SELECT id, cwd, title, provider, model, account, effort, created_at, updated_at
             FROM code_threads WHERE cwd = ? ORDER BY updated_at DESC, id DESC LIMIT ?",
        )
        .bind(cwd)
        .bind(limit)
        .fetch_all(&self.readers)
        .await?;
        Ok(threads)
    }

    pub async fn last_turn_interrupted(&self, thread_id: i64) -> CodeResult<bool> {
        let status: Option<String> = sqlx::query_scalar(
            "SELECT status FROM code_turns WHERE thread_id = ? ORDER BY id DESC LIMIT 1",
        )
        .bind(thread_id)
        .fetch_optional(&self.readers)
        .await?;
        Ok(matches!(status.as_deref(), Some("interrupted")))
    }

    pub async fn record_open_prompt(
        &self,
        thread_id: i64,
        call_id: String,
        kind: String,
        payload: String,
        task_id: u64,
        created_at: i64,
    ) -> CodeResult<()> {
        let task_id = i64::try_from(task_id).unwrap_or(i64::MAX);
        sqlx::query(
            "INSERT OR REPLACE INTO code_open_prompts
             (thread_id, call_id, kind, payload, task_id, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(thread_id)
        .bind(call_id)
        .bind(kind)
        .bind(payload)
        .bind(task_id)
        .bind(created_at)
        .execute(&self.writer)
        .await?;
        Ok(())
    }

    pub async fn clear_open_prompt(&self, thread_id: i64, call_id: String) -> CodeResult<()> {
        sqlx::query("DELETE FROM code_open_prompts WHERE thread_id = ? AND call_id = ?")
            .bind(thread_id)
            .bind(call_id)
            .execute(&self.writer)
            .await?;
        Ok(())
    }

    pub async fn open_prompts(&self, thread_id: i64) -> CodeResult<Vec<OpenPrompt>> {
        let rows = sqlx::query(
            "SELECT call_id, kind, payload, task_id FROM code_open_prompts
             WHERE thread_id = ? ORDER BY created_at ASC",
        )
        .bind(thread_id)
        .fetch_all(&self.readers)
        .await?;
        let out = rows
            .into_iter()
            .map(|row| {
                let task_id: i64 = row.get("task_id");
                OpenPrompt {
                    call_id: row.get("call_id"),
                    kind: row.get("kind"),
                    payload: row.get("payload"),
                    task_id: u64::try_from(task_id).unwrap_or(0),
                }
            })
            .collect();
        Ok(out)
    }

    pub async fn get_messages(&self, thread_id: i64) -> CodeResult<Vec<StoredMessage>> {
        let messages = sqlx::query_as::<_, StoredMessage>(
            "SELECT id, turn_id, role, body, created_at
             FROM code_messages WHERE thread_id = ? ORDER BY id ASC",
        )
        .bind(thread_id)
        .fetch_all(&self.readers)
        .await?;
        Ok(messages)
    }

    pub async fn update_thread_model(
        &self,
        id: i64,
        provider: String,
        model: String,
        account: String,
        effort: Option<String>,
        updated_at: i64,
    ) -> CodeResult<()> {
        sqlx::query(
            "UPDATE code_threads SET provider = ?, model = ?, account = ?, effort = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(provider)
        .bind(model)
        .bind(account)
        .bind(effort)
        .bind(updated_at)
        .bind(id)
        .execute(&self.writer)
        .await?;
        Ok(())
    }

    pub async fn update_thread_title(&self, id: i64, title: String) -> CodeResult<()> {
        sqlx::query("UPDATE code_threads SET title = ? WHERE id = ?")
            .bind(title)
            .bind(id)
            .execute(&self.writer)
            .await?;
        Ok(())
    }

    pub async fn create_turn(&self, turn: NewTurn) -> CodeResult<i64> {
        let id = sqlx::query(
            "INSERT INTO code_turns (thread_id, task_id, provider, model, account, effort, status, started_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(turn.thread_id)
        .bind(turn.task_id)
        .bind(turn.provider)
        .bind(turn.model)
        .bind(turn.account)
        .bind(turn.effort)
        .bind(turn.status)
        .bind(turn.started_at)
        .execute(&self.writer)
        .await?
        .last_insert_rowid();
        Ok(id)
    }

    pub async fn finish_turn(&self, id: i64, status: String, finished_at: i64) -> CodeResult<()> {
        sqlx::query("UPDATE code_turns SET status = ?, finished_at = ? WHERE id = ?")
            .bind(status)
            .bind(finished_at)
            .bind(id)
            .execute(&self.writer)
            .await?;
        Ok(())
    }

    pub async fn mark_running_turns_interrupted(&self, finished_at: i64) -> CodeResult<u64> {
        let changed = sqlx::query(
            "UPDATE code_turns SET status = 'interrupted', finished_at = ? WHERE status = 'running'",
        )
        .bind(finished_at)
        .execute(&self.writer)
        .await?
        .rows_affected();
        Ok(changed)
    }

    pub async fn create_process(&self, process: NewProcess) -> CodeResult<i64> {
        let id = sqlx::query(
            "INSERT INTO code_processes (pgid, command, cwd, status, started_at)
             VALUES (?, ?, ?, 'running', ?)",
        )
        .bind(process.pgid)
        .bind(process.command)
        .bind(process.cwd)
        .bind(process.started_at)
        .execute(&self.writer)
        .await?
        .last_insert_rowid();
        Ok(id)
    }

    pub async fn finish_process(&self, id: i64, finished_at: i64) -> CodeResult<()> {
        sqlx::query("UPDATE code_processes SET status = 'dead', finished_at = ? WHERE id = ?")
            .bind(finished_at)
            .bind(id)
            .execute(&self.writer)
            .await?;
        Ok(())
    }

    pub async fn take_orphan_processes(&self, finished_at: i64) -> CodeResult<Vec<OrphanProcess>> {
        let mut tx = self.writer.begin().await?;
        let rows =
            sqlx::query("SELECT id, pgid, command FROM code_processes WHERE status = 'running'")
                .fetch_all(&mut *tx)
                .await?;
        let orphans: Vec<OrphanProcess> = rows
            .into_iter()
            .map(|row| OrphanProcess {
                id: row.get("id"),
                pgid: row.get("pgid"),
                command: row.get("command"),
            })
            .collect();
        sqlx::query(
            "UPDATE code_processes SET status = 'dead', finished_at = ? WHERE status = 'running'",
        )
        .bind(finished_at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(orphans)
    }

    pub async fn create_message(&self, message: NewMessage) -> CodeResult<i64> {
        let id = sqlx::query(
            "INSERT INTO code_messages (thread_id, turn_id, role, body, created_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(message.thread_id)
        .bind(message.turn_id)
        .bind(message.role)
        .bind(message.body)
        .bind(message.created_at)
        .execute(&self.writer)
        .await?
        .last_insert_rowid();
        Ok(id)
    }

    pub async fn create_tool_call(&self, call: NewToolCall) -> CodeResult<i64> {
        let id = sqlx::query(
            "INSERT INTO code_tool_calls (thread_id, turn_id, call_id, name, input, status, started_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(call.thread_id)
        .bind(call.turn_id)
        .bind(call.call_id)
        .bind(call.name)
        .bind(call.input)
        .bind(call.status)
        .bind(call.started_at)
        .execute(&self.writer)
        .await?
        .last_insert_rowid();
        Ok(id)
    }

    pub async fn finish_tool_call(
        &self,
        id: i64,
        status: String,
        summary: Option<String>,
        finished_at: i64,
    ) -> CodeResult<()> {
        sqlx::query(
            "UPDATE code_tool_calls SET status = ?, summary = ?, finished_at = ? WHERE id = ?",
        )
        .bind(status)
        .bind(summary)
        .bind(finished_at)
        .bind(id)
        .execute(&self.writer)
        .await?;
        Ok(())
    }

    pub async fn create_compaction(&self, compaction: NewCompaction) -> CodeResult<i64> {
        let preserved = serde_json::to_string(&compaction.preserved_message_ids)
            .unwrap_or_else(|_| "[]".to_owned());
        let id = sqlx::query(
            "INSERT INTO code_compactions (thread_id, summary, after_message_id, tail_from_message_id, preserved_message_ids, tokens_before, tokens_after, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(compaction.thread_id)
        .bind(compaction.summary)
        .bind(compaction.after_message_id)
        .bind(compaction.tail_from_message_id)
        .bind(preserved)
        .bind(compaction.tokens_before)
        .bind(compaction.tokens_after)
        .bind(compaction.created_at)
        .execute(&self.writer)
        .await?
        .last_insert_rowid();
        Ok(id)
    }

    pub async fn compactions_for_thread(&self, thread_id: i64) -> CodeResult<Vec<Compaction>> {
        let rows = sqlx::query(
            "SELECT id, thread_id, summary, after_message_id, tail_from_message_id, preserved_message_ids, tokens_before, tokens_after, created_at
             FROM code_compactions WHERE thread_id = ? ORDER BY id ASC",
        )
        .bind(thread_id)
        .fetch_all(&self.readers)
        .await?;
        let out = rows
            .into_iter()
            .map(|row| {
                let preserved_raw: String = row.get("preserved_message_ids");
                Compaction {
                    id: row.get("id"),
                    thread_id: row.get("thread_id"),
                    summary: row.get("summary"),
                    after_message_id: row.get("after_message_id"),
                    tail_from_message_id: row.get("tail_from_message_id"),
                    preserved_message_ids: serde_json::from_str(&preserved_raw).unwrap_or_default(),
                    tokens_before: row.get("tokens_before"),
                    tokens_after: row.get("tokens_after"),
                    created_at: row.get("created_at"),
                }
            })
            .collect();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_thread() -> NewThread {
        NewThread {
            cwd: "/tmp/project".into(),
            title: Some("first".into()),
            provider: "openai".into(),
            model: "gpt-x".into(),
            account: "default".into(),
            effort: None,
            created_at: 100,
            updated_at: 100,
        }
    }

    #[tokio::test]
    async fn migrates_and_roundtrips_thread() {
        let store = CodeStore::open_in_memory().await.unwrap();
        let id = store.create_thread(sample_thread()).await.unwrap();
        let thread = store.get_thread(id).await.unwrap().unwrap();
        assert_eq!(thread.provider, "openai");
        assert_eq!(thread.title.as_deref(), Some("first"));
    }

    #[tokio::test]
    async fn latest_thread_in_is_scoped_to_cwd_and_recency() {
        let store = CodeStore::open_in_memory().await.unwrap();
        let make = |cwd: &str, model: &str, updated: i64| NewThread {
            cwd: cwd.into(),
            title: None,
            provider: "openai".into(),
            model: model.into(),
            account: "default".into(),
            effort: None,
            created_at: updated,
            updated_at: updated,
        };
        store.create_thread(make("/a", "old", 100)).await.unwrap();
        store.create_thread(make("/a", "new", 200)).await.unwrap();
        store.create_thread(make("/b", "other", 300)).await.unwrap();
        let latest = store.latest_thread_in("/a".into()).await.unwrap().unwrap();
        assert_eq!(latest.model, "new");
        assert!(
            store
                .latest_thread_in("/missing".into())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn persists_turn_message_tool_call_and_compaction() {
        let store = CodeStore::open_in_memory().await.unwrap();
        let thread_id = store.create_thread(sample_thread()).await.unwrap();
        let turn_id = store
            .create_turn(NewTurn {
                thread_id,
                task_id: 1,
                provider: "openai".into(),
                model: "gpt-x".into(),
                account: "default".into(),
                effort: None,
                status: "running".into(),
                started_at: 110,
            })
            .await
            .unwrap();
        let m1 = store
            .create_message(NewMessage {
                thread_id,
                turn_id: None,
                role: "user".into(),
                body: "hello".into(),
                created_at: 111,
            })
            .await
            .unwrap();
        assert_eq!(m1, 1, "message ids start at 1 and are stable");
        store
            .create_tool_call(NewToolCall {
                thread_id,
                turn_id,
                call_id: "call-1".into(),
                name: "Read".into(),
                input: "file.rs".into(),
                status: "running".into(),
                started_at: 112,
            })
            .await
            .unwrap();
        store
            .finish_turn(turn_id, "done".into(), 120)
            .await
            .unwrap();
        let comp = store
            .create_compaction(NewCompaction {
                thread_id,
                summary: "s".into(),
                after_message_id: 0,
                tail_from_message_id: None,
                preserved_message_ids: vec![1, 2, 3],
                tokens_before: 10,
                tokens_after: 5,
                created_at: 130,
            })
            .await
            .unwrap();
        assert!(comp > 0);
        let comps = store.compactions_for_thread(thread_id).await.unwrap();
        assert_eq!(comps[0].preserved_message_ids, vec![1, 2, 3]);
        assert_eq!(comps[0].after_message_id, 0);
    }
}
