use std::fmt::Write;
use std::path::Path;
use std::time::Duration;

use sha2::{Digest, Sha256};
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
    pub kind: Option<String>,
    pub body: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct StoredMessage {
    pub id: i64,
    pub parent_message_id: Option<i64>,
    pub turn_id: Option<i64>,
    pub role: String,
    pub kind: Option<String>,
    pub body: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedMessage {
    pub id: i64,
    pub parent_message_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCodeCheckpoint {
    pub thread_id: i64,
    pub prompt_message_id: i64,
    pub parent_message_id: Option<i64>,
    pub draft: String,
    pub attachments: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeCheckpoint {
    pub id: i64,
    pub prompt_message_id: i64,
    pub parent_message_id: Option<i64>,
    pub draft: String,
    pub attachments: String,
    pub files_available: bool,
    pub touched: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointFileVersion {
    pub checkpoint_id: i64,
    pub path: String,
    pub content: Option<Vec<u8>>,
    pub mode: Option<u32>,
    pub supported: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCheckpointFile {
    pub checkpoint_id: i64,
    pub path: String,
    pub content: Option<Vec<u8>>,
    pub mode: Option<u32>,
    pub supported: bool,
    pub touched: bool,
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
            "WITH RECURSIVE active(id, parent_message_id, turn_id, role, kind, body, created_at) AS (
                 SELECT m.id, m.parent_message_id, m.turn_id, m.role, m.kind, m.body, m.created_at
                 FROM code_messages m
                 JOIN code_threads t ON t.head_message_id = m.id
                 WHERE t.id = ?
                 UNION ALL
                 SELECT m.id, m.parent_message_id, m.turn_id, m.role, m.kind, m.body, m.created_at
                 FROM code_messages m
                 JOIN active a ON a.parent_message_id = m.id
             )
             SELECT id, parent_message_id, turn_id, role, kind, body, created_at
             FROM active ORDER BY id ASC",
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

    pub async fn create_message(&self, message: NewMessage) -> CodeResult<CreatedMessage> {
        let mut tx = self.writer.begin().await?;
        let parent_message_id: Option<i64> =
            sqlx::query_scalar("SELECT head_message_id FROM code_threads WHERE id = ?")
                .bind(message.thread_id)
                .fetch_one(&mut *tx)
                .await?;
        let id = sqlx::query(
            "INSERT INTO code_messages
             (thread_id, parent_message_id, turn_id, role, kind, body, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(message.thread_id)
        .bind(parent_message_id)
        .bind(message.turn_id)
        .bind(message.role)
        .bind(message.kind)
        .bind(message.body)
        .bind(message.created_at)
        .execute(&mut *tx)
        .await?
        .last_insert_rowid();
        sqlx::query("UPDATE code_threads SET head_message_id = ?, updated_at = ? WHERE id = ?")
            .bind(id)
            .bind(message.created_at)
            .bind(message.thread_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(CreatedMessage {
            id,
            parent_message_id,
        })
    }

    pub async fn create_code_checkpoint(&self, checkpoint: NewCodeCheckpoint) -> CodeResult<i64> {
        let mut tx = self.writer.begin().await?;
        let id = sqlx::query(
            "INSERT INTO code_checkpoints
             (thread_id, prompt_message_id, parent_message_id, draft, attachments, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(checkpoint.thread_id)
        .bind(checkpoint.prompt_message_id)
        .bind(checkpoint.parent_message_id)
        .bind(checkpoint.draft)
        .bind(checkpoint.attachments)
        .bind(checkpoint.created_at)
        .execute(&mut *tx)
        .await?
        .last_insert_rowid();
        sqlx::query(
            "UPDATE code_checkpoints SET files_available = 0
             WHERE thread_id = ? AND id NOT IN (
                 SELECT id FROM code_checkpoints
                 WHERE thread_id = ? ORDER BY id DESC LIMIT 100
             )",
        )
        .bind(checkpoint.thread_id)
        .bind(checkpoint.thread_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "DELETE FROM code_checkpoint_files
             WHERE checkpoint_id IN (
                 SELECT id FROM code_checkpoints
                 WHERE thread_id = ? AND files_available = 0
             )",
        )
        .bind(checkpoint.thread_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "DELETE FROM code_checkpoint_blobs
             WHERE NOT EXISTS (
                 SELECT 1 FROM code_checkpoint_files
                 WHERE blob_hash = code_checkpoint_blobs.hash
             )",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(id)
    }

    pub async fn active_code_checkpoints(&self, thread_id: i64) -> CodeResult<Vec<CodeCheckpoint>> {
        let rows = sqlx::query(
            "WITH RECURSIVE active_messages(id, parent_message_id) AS (
                 SELECT m.id, m.parent_message_id
                 FROM code_messages m
                 JOIN code_threads t ON t.head_message_id = m.id
                 WHERE t.id = ?
                 UNION ALL
                 SELECT m.id, m.parent_message_id
                 FROM code_messages m
                 JOIN active_messages a ON a.parent_message_id = m.id
             )
             SELECT c.id, c.prompt_message_id, c.parent_message_id, c.draft,
                    c.attachments, c.files_available, c.created_at,
                    EXISTS(
                        SELECT 1 FROM code_checkpoint_files f
                        WHERE f.checkpoint_id = c.id AND f.touched = 1
                    ) AS touched
             FROM code_checkpoints c
             JOIN active_messages a ON a.id = c.prompt_message_id
             WHERE c.thread_id = ?
             ORDER BY c.id DESC",
        )
        .bind(thread_id)
        .bind(thread_id)
        .fetch_all(&self.readers)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| CodeCheckpoint {
                id: row.get("id"),
                prompt_message_id: row.get("prompt_message_id"),
                parent_message_id: row.get("parent_message_id"),
                draft: row.get("draft"),
                attachments: row.get("attachments"),
                files_available: row.get::<i64, _>("files_available") != 0,
                touched: row.get::<i64, _>("touched") != 0,
                created_at: row.get("created_at"),
            })
            .collect())
    }

    pub async fn tracked_checkpoint_paths(&self, thread_id: i64) -> CodeResult<Vec<String>> {
        let paths = sqlx::query_scalar(
            "SELECT DISTINCT f.path
             FROM code_checkpoint_files f
             JOIN code_checkpoints c ON c.id = f.checkpoint_id
             WHERE c.thread_id = ? AND c.files_available = 1
             ORDER BY f.path",
        )
        .bind(thread_id)
        .fetch_all(&self.readers)
        .await?;
        Ok(paths)
    }

    pub async fn record_checkpoint_file(&self, file: NewCheckpointFile) -> CodeResult<()> {
        let mut tx = self.writer.begin().await?;
        let blob_hash = file.content.as_ref().map(|bytes| {
            let mut digest = Sha256::new();
            digest.update(bytes);
            digest
                .finalize()
                .iter()
                .fold(String::with_capacity(64), |mut hash, byte| {
                    write!(&mut hash, "{byte:02x}").expect("writing to a string cannot fail");
                    hash
                })
        });
        if let (Some(hash), Some(bytes)) = (&blob_hash, file.content) {
            sqlx::query(
                "INSERT OR IGNORE INTO code_checkpoint_blobs (hash, content) VALUES (?, ?)",
            )
            .bind(hash)
            .bind(bytes)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "INSERT INTO code_checkpoint_files
             (checkpoint_id, path, present, blob_hash, mode, supported, touched)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(checkpoint_id, path) DO UPDATE SET
                 touched = MAX(touched, excluded.touched)",
        )
        .bind(file.checkpoint_id)
        .bind(file.path)
        .bind(i32::from(blob_hash.is_some()))
        .bind(blob_hash)
        .bind(file.mode.map(i64::from))
        .bind(i32::from(file.supported))
        .bind(i32::from(file.touched))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn active_checkpoint_file_versions(
        &self,
        thread_id: i64,
    ) -> CodeResult<Vec<CheckpointFileVersion>> {
        let rows = sqlx::query(
            "WITH RECURSIVE active_messages(id, parent_message_id) AS (
                 SELECT m.id, m.parent_message_id
                 FROM code_messages m
                 JOIN code_threads t ON t.head_message_id = m.id
                 WHERE t.id = ?
                 UNION ALL
                 SELECT m.id, m.parent_message_id
                 FROM code_messages m
                 JOIN active_messages a ON a.parent_message_id = m.id
             )
             SELECT f.checkpoint_id, f.path, f.present, f.mode, f.supported, b.content
             FROM code_checkpoint_files f
             JOIN code_checkpoints c ON c.id = f.checkpoint_id
             JOIN active_messages a ON a.id = c.prompt_message_id
             LEFT JOIN code_checkpoint_blobs b ON b.hash = f.blob_hash
             WHERE c.thread_id = ? AND c.files_available = 1
             ORDER BY f.checkpoint_id ASC, f.path ASC",
        )
        .bind(thread_id)
        .bind(thread_id)
        .fetch_all(&self.readers)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| CheckpointFileVersion {
                checkpoint_id: row.get("checkpoint_id"),
                path: row.get("path"),
                content: if row.get::<i64, _>("present") == 0 {
                    None
                } else {
                    Some(row.get::<Vec<u8>, _>("content"))
                },
                mode: row
                    .get::<Option<i64>, _>("mode")
                    .and_then(|mode| u32::try_from(mode).ok()),
                supported: row.get::<i64, _>("supported") != 0,
            })
            .collect())
    }

    pub async fn set_thread_head(
        &self,
        thread_id: i64,
        head_message_id: Option<i64>,
        updated_at: i64,
    ) -> CodeResult<()> {
        sqlx::query("UPDATE code_threads SET head_message_id = ?, updated_at = ? WHERE id = ?")
            .bind(head_message_id)
            .bind(updated_at)
            .bind(thread_id)
            .execute(&self.writer)
            .await?;
        Ok(())
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
                kind: None,
                body: "hello".into(),
                created_at: 111,
            })
            .await
            .unwrap();
        assert_eq!(m1.id, 1, "message ids start at 1 and are stable");
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

    #[tokio::test]
    async fn message_head_selects_one_conversation_branch() {
        let store = CodeStore::open_in_memory().await.unwrap();
        let thread_id = store.create_thread(sample_thread()).await.unwrap();
        let first = store
            .create_message(NewMessage {
                thread_id,
                turn_id: None,
                role: "user".into(),
                kind: None,
                body: "first".into(),
                created_at: 101,
            })
            .await
            .unwrap();
        let abandoned = store
            .create_message(NewMessage {
                thread_id,
                turn_id: None,
                role: "assistant".into(),
                kind: None,
                body: "old branch".into(),
                created_at: 102,
            })
            .await
            .unwrap();
        store
            .set_thread_head(thread_id, Some(first.id), 103)
            .await
            .unwrap();
        let replacement = store
            .create_message(NewMessage {
                thread_id,
                turn_id: None,
                role: "assistant".into(),
                kind: None,
                body: "new branch".into(),
                created_at: 104,
            })
            .await
            .unwrap();

        let messages = store.get_messages(thread_id).await.unwrap();
        let ids: Vec<_> = messages.iter().map(|message| message.id).collect();
        assert_eq!(ids, vec![first.id, replacement.id]);
        assert_eq!(replacement.parent_message_id, Some(first.id));
        assert!(!ids.contains(&abandoned.id));
    }

    #[tokio::test]
    async fn checkpoints_follow_the_active_conversation_branch() {
        let store = CodeStore::open_in_memory().await.unwrap();
        let thread_id = store.create_thread(sample_thread()).await.unwrap();
        let first = store
            .create_message(NewMessage {
                thread_id,
                turn_id: None,
                role: "user".into(),
                kind: None,
                body: "first".into(),
                created_at: 101,
            })
            .await
            .unwrap();
        let first_checkpoint = store
            .create_code_checkpoint(NewCodeCheckpoint {
                thread_id,
                prompt_message_id: first.id,
                parent_message_id: first.parent_message_id,
                draft: "first".into(),
                attachments: "[]".into(),
                created_at: 101,
            })
            .await
            .unwrap();
        let second = store
            .create_message(NewMessage {
                thread_id,
                turn_id: None,
                role: "user".into(),
                kind: None,
                body: "second".into(),
                created_at: 102,
            })
            .await
            .unwrap();
        store
            .create_code_checkpoint(NewCodeCheckpoint {
                thread_id,
                prompt_message_id: second.id,
                parent_message_id: second.parent_message_id,
                draft: "second".into(),
                attachments: "[]".into(),
                created_at: 102,
            })
            .await
            .unwrap();
        store
            .set_thread_head(thread_id, Some(first.id), 103)
            .await
            .unwrap();

        let checkpoints = store.active_code_checkpoints(thread_id).await.unwrap();
        assert_eq!(checkpoints.len(), 1);
        assert_eq!(checkpoints[0].id, first_checkpoint);
    }

    #[tokio::test]
    async fn migration_0022_backfills_kind_and_strips_language_reminder() {
        let reminder = "[Reminder: write your prose to the user in the language they used in their request. Keep code, identifiers, file paths, shell commands, tool arguments, and quoted file or output excerpts exactly as they are. Text stored in the repository stays in the project's prevailing language.]";
        crate::register_sqlite_vec();
        let opts = "sqlite::memory:"
            .parse::<SqliteConnectOptions>()
            .unwrap()
            .disable_statement_logging();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE code_threads (
                id INTEGER PRIMARY KEY, cwd TEXT NOT NULL, title TEXT,
                provider TEXT NOT NULL, model TEXT NOT NULL, account TEXT NOT NULL,
                created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, effort TEXT
            );
            CREATE TABLE code_messages (
                id INTEGER PRIMARY KEY, thread_id INTEGER NOT NULL, turn_id INTEGER,
                role TEXT NOT NULL, body TEXT NOT NULL, created_at INTEGER NOT NULL
            );
            INSERT INTO code_threads (id, cwd, provider, model, account, created_at, updated_at)
            VALUES (1, '/tmp', 'p', 'm', 'a', 1, 1);
            INSERT INTO code_messages (id, thread_id, role, body, created_at) VALUES
                (1, 1, 'user', '[{\"type\":\"text\",\"text\":\"hello\"}]', 1),
                (2, 1, 'user', '[{\"type\":\"text\",\"text\":\"<environment-notice>\\nstuff\"}]', 2),
                (3, 1, 'user', '[{\"type\":\"text\",\"text\":\"The plan at /p is approved. Implement it now.\"}]', 3),
                (4, 1, 'user', '[{\"type\":\"text\",\"text\":\"The user did not approve the plan.\"}]', 4),
                (5, 1, 'user', '[{\"type\":\"tool_result\",\"tool_use_id\":\"x\",\"content\":\"out\"},{\"type\":\"text\",\"text\":\"' || ? || '\"}]', 5);",
        )
        .bind(reminder)
        .execute(&pool)
        .await
        .unwrap();
        for statement in include_str!("../migrations/0022_message_kind.sql").split(';') {
            let statement = statement.trim();
            if statement.is_empty() {
                continue;
            }
            sqlx::query(statement).execute(&pool).await.unwrap();
        }
        let rows = sqlx::query("SELECT id, kind, body FROM code_messages ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        let kind_of = |id: i64| {
            rows.iter()
                .find(|row| row.get::<i64, _>("id") == id)
                .map(|row| row.get::<Option<String>, _>("kind"))
        };
        assert_eq!(kind_of(1).unwrap().as_deref(), Some("user"));
        assert_eq!(kind_of(2).unwrap().as_deref(), Some("wake"));
        assert_eq!(kind_of(3).unwrap().as_deref(), Some("plan_decision"));
        assert_eq!(kind_of(4).unwrap().as_deref(), Some("plan_decision"));
        assert_eq!(kind_of(5).unwrap().as_deref(), Some("user"));
        let body5: String = rows
            .iter()
            .find(|row| row.get::<i64, _>("id") == 5)
            .map(|row| row.get("body"))
            .unwrap();
        assert!(
            !body5.contains("[Reminder"),
            "reminder block stripped: {body5}"
        );
        assert!(body5.contains("tool_result"), "tool_result kept: {body5}");
        let body2: String = rows
            .iter()
            .find(|row| row.get::<i64, _>("id") == 2)
            .map(|row| row.get("body"))
            .unwrap();
        assert!(
            body2.contains("<environment-notice>"),
            "wake body kept: {body2}"
        );
    }
}
