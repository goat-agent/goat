use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use goat_types::{ConversationId, PersonaId};
use sqlx::sqlite::SqlitePool;
use thiserror::Error;
use uuid::Uuid;

pub mod embed;

pub use embed::{DummyEmbedder, Embedder};

#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("sqlx: {0}")]
    Sqlx(#[from] sqlx::Error),
}

pub type MemoryResult<T> = Result<T, MemoryError>;

#[derive(Clone, Debug)]
pub struct CoreBlock {
    pub slug: String,
    pub text: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct EpisodicEntry {
    pub id: String,
    pub kind: EpisodicKind,
    pub text: String,
    pub ts: DateTime<Utc>,
    pub score: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EpisodicKind {
    User,
    Assistant,
    Observation,
}

impl EpisodicKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Observation => "observation",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "user" => Self::User,
            "assistant" => Self::Assistant,
            _ => Self::Observation,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SemanticEntry {
    pub topic: String,
    pub text: String,
    pub source_path: String,
    pub updated_at: DateTime<Utc>,
    pub score: Option<f32>,
}

#[async_trait]
pub trait MemoryStore: Send + Sync + 'static {
    async fn core_blocks(&self, persona: PersonaId) -> MemoryResult<Vec<CoreBlock>>;
    async fn upsert_core(&self, persona: PersonaId, slug: &str, text: &str) -> MemoryResult<()>;

    async fn append_episodic(
        &self,
        persona: PersonaId,
        conv: &ConversationId,
        kind: EpisodicKind,
        text: &str,
        embedding: Option<&[f32]>,
    ) -> MemoryResult<()>;

    async fn search_episodic(
        &self,
        persona: PersonaId,
        query: &[f32],
        k: usize,
    ) -> MemoryResult<Vec<EpisodicEntry>>;

    async fn search_semantic(
        &self,
        persona: PersonaId,
        query: &[f32],
        k: usize,
    ) -> MemoryResult<Vec<SemanticEntry>>;

    async fn upsert_semantic(
        &self,
        persona: PersonaId,
        topic: &str,
        text: &str,
        source_path: &str,
        embedding: &[f32],
    ) -> MemoryResult<()>;
}

pub struct SqliteMemory {
    pool: Arc<SqlitePool>,
}

impl SqliteMemory {
    /// Build a memory store over an already-open pool. The memory tables
    /// (`core_memory`, `episodic_memory`, `semantic_memory`) carry foreign
    /// keys into `personas`/`conversations`, so they must live in the same
    /// database as [`goat_store::SqliteStore`]. The runtime passes that
    /// store's pool here rather than opening a second database file.
    pub fn from_pool(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    pub async fn open(path: &Path) -> MemoryResult<Self> {
        use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
        use sqlx::ConnectOptions;
        let url = format!("sqlite://{}", path.display());
        let opts: SqliteConnectOptions = url
            .parse::<SqliteConnectOptions>()?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .disable_statement_logging();
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(opts)
            .await?;
        Ok(Self {
            pool: Arc::new(pool),
        })
    }
}

#[async_trait]
impl MemoryStore for SqliteMemory {
    async fn core_blocks(&self, persona: PersonaId) -> MemoryResult<Vec<CoreBlock>> {
        let rows = sqlx::query_as::<_, (String, String, String)>(
            r#"SELECT slug, text, updated_at FROM core_memory WHERE persona_id = ?"#,
        )
        .bind(persona.to_string())
        .fetch_all(&*self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(slug, text, ts)| CoreBlock {
                slug,
                text,
                updated_at: parse_ts(&ts),
            })
            .collect())
    }

    async fn upsert_core(&self, persona: PersonaId, slug: &str, text: &str) -> MemoryResult<()> {
        sqlx::query(
            r#"INSERT INTO core_memory (persona_id, slug, text, updated_at)
               VALUES (?, ?, ?, ?)
               ON CONFLICT(persona_id, slug) DO UPDATE SET
                 text = excluded.text,
                 updated_at = excluded.updated_at"#,
        )
        .bind(persona.to_string())
        .bind(slug)
        .bind(text)
        .bind(Utc::now().to_rfc3339())
        .execute(&*self.pool)
        .await?;
        Ok(())
    }

    async fn append_episodic(
        &self,
        persona: PersonaId,
        conv: &ConversationId,
        kind: EpisodicKind,
        text: &str,
        embedding: Option<&[f32]>,
    ) -> MemoryResult<()> {
        let blob = embedding.map(encode_vec);
        sqlx::query(
            r#"INSERT INTO episodic_memory
               (id, persona_id, conversation_id, kind, text, embedding, ts)
               VALUES (?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(persona.to_string())
        .bind(conv.to_key())
        .bind(kind.as_str())
        .bind(text)
        .bind(blob)
        .bind(Utc::now().to_rfc3339())
        .execute(&*self.pool)
        .await?;
        Ok(())
    }

    async fn search_episodic(
        &self,
        persona: PersonaId,
        query: &[f32],
        k: usize,
    ) -> MemoryResult<Vec<EpisodicEntry>> {
        let rows = sqlx::query_as::<_, (String, String, String, String, Option<Vec<u8>>)>(
            r#"SELECT id, kind, text, ts, embedding
               FROM episodic_memory
               WHERE persona_id = ? AND embedding IS NOT NULL"#,
        )
        .bind(persona.to_string())
        .fetch_all(&*self.pool)
        .await?;
        let mut scored: Vec<EpisodicEntry> = rows
            .into_iter()
            .filter_map(|(id, kind, text, ts, embedding)| {
                let vec = decode_vec(&embedding?);
                let score = cosine(query, &vec);
                Some(EpisodicEntry {
                    id,
                    kind: EpisodicKind::parse(&kind),
                    text,
                    ts: parse_ts(&ts),
                    score: Some(score),
                })
            })
            .collect();
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(k);
        Ok(scored)
    }

    async fn search_semantic(
        &self,
        persona: PersonaId,
        query: &[f32],
        k: usize,
    ) -> MemoryResult<Vec<SemanticEntry>> {
        let rows = sqlx::query_as::<_, (String, String, String, String, Vec<u8>)>(
            r#"SELECT topic, text, source_path, updated_at, embedding
               FROM semantic_memory
               WHERE persona_id = ?"#,
        )
        .bind(persona.to_string())
        .fetch_all(&*self.pool)
        .await?;
        let mut scored: Vec<SemanticEntry> = rows
            .into_iter()
            .map(|(topic, text, source_path, ts, embedding)| {
                let vec = decode_vec(&embedding);
                let score = cosine(query, &vec);
                SemanticEntry {
                    topic,
                    text,
                    source_path,
                    updated_at: parse_ts(&ts),
                    score: Some(score),
                }
            })
            .collect();
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(k);
        Ok(scored)
    }

    async fn upsert_semantic(
        &self,
        persona: PersonaId,
        topic: &str,
        text: &str,
        source_path: &str,
        embedding: &[f32],
    ) -> MemoryResult<()> {
        sqlx::query(
            r#"INSERT INTO semantic_memory
               (id, persona_id, topic, text, source_path, updated_at, embedding)
               VALUES (?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(persona_id, topic) DO UPDATE SET
                 text = excluded.text,
                 source_path = excluded.source_path,
                 updated_at = excluded.updated_at,
                 embedding = excluded.embedding"#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(persona.to_string())
        .bind(topic)
        .bind(text)
        .bind(source_path)
        .bind(Utc::now().to_rfc3339())
        .bind(encode_vec(embedding))
        .execute(&*self.pool)
        .await?;
        Ok(())
    }
}

pub(crate) fn parse_ts_pub(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

// Keep the private alias so existing callers in this file are unchanged.
fn parse_ts(s: &str) -> DateTime<Utc> {
    parse_ts_pub(s)
}

fn encode_vec(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

pub(crate) fn decode_vec(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

pub(crate) fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0_f32;
    let mut na = 0.0_f32;
    let mut nb = 0.0_f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom == 0.0 {
        0.0
    } else {
        dot / denom
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_identity() {
        let v = vec![0.5, 0.5, 0.5, 0.5];
        let s = cosine(&v, &v);
        assert!((s - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let s = cosine(&a, &b);
        assert!(s.abs() < 1e-6);
    }

    #[test]
    fn vec_codec_round_trip() {
        let v = vec![0.1_f32, -0.2, 0.3, 1.5];
        let b = encode_vec(&v);
        let back = decode_vec(&b);
        for (a, b) in v.iter().zip(back.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    use goat_store::{SqliteStore, Store};
    use goat_types::{ChannelId, ConversationId, InstanceId};

    async fn fresh_memory() -> (SqliteMemory, SqliteStore) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("goat.db");
        std::mem::forget(dir);
        let store = SqliteStore::open(&path).await.unwrap();
        let memory = SqliteMemory::from_pool(store.pool());
        (memory, store)
    }

    fn conv() -> ConversationId {
        ConversationId::new(ChannelId::new("telegram"), InstanceId::new(), "chat:1")
    }

    #[tokio::test]
    async fn episodic_round_trip_ranks_by_similarity() {
        let (mem, store) = fresh_memory().await;
        let persona = PersonaId::new();
        store.ensure_persona(persona, "dev", "dev").await.unwrap();
        let c = conv();
        store.ensure_conversation(&c, persona).await.unwrap();

        mem.append_episodic(
            persona,
            &c,
            EpisodicKind::User,
            "likes cats",
            Some(&[1.0, 0.0]),
        )
        .await
        .unwrap();
        mem.append_episodic(
            persona,
            &c,
            EpisodicKind::User,
            "likes dogs",
            Some(&[0.0, 1.0]),
        )
        .await
        .unwrap();

        let hits = mem.search_episodic(persona, &[1.0, 0.0], 1).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "likes cats");
    }

    #[tokio::test]
    async fn episodic_search_is_persona_scoped() {
        let (mem, store) = fresh_memory().await;
        let alice = PersonaId::new();
        let bob = PersonaId::new();
        store.ensure_persona(alice, "alice", "alice").await.unwrap();
        store.ensure_persona(bob, "bob", "bob").await.unwrap();
        let c = conv();
        store.ensure_conversation(&c, alice).await.unwrap();

        mem.append_episodic(
            alice,
            &c,
            EpisodicKind::User,
            "alice secret",
            Some(&[1.0, 0.0]),
        )
        .await
        .unwrap();

        let bob_hits = mem.search_episodic(bob, &[1.0, 0.0], 5).await.unwrap();
        assert!(bob_hits.is_empty(), "bob must not see alice's memory");
        let alice_hits = mem.search_episodic(alice, &[1.0, 0.0], 5).await.unwrap();
        assert_eq!(alice_hits.len(), 1);
    }

    #[tokio::test]
    async fn core_upsert_overwrites_by_slug() {
        let (mem, store) = fresh_memory().await;
        let persona = PersonaId::new();
        store.ensure_persona(persona, "dev", "dev").await.unwrap();

        mem.upsert_core(persona, "tz", "UTC").await.unwrap();
        mem.upsert_core(persona, "tz", "KST").await.unwrap();
        let blocks = mem.core_blocks(persona).await.unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].text, "KST");
    }
}
