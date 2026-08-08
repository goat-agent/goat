use std::path::Path;
use std::sync::Arc;

use chrono::Utc;
use sqlx::ConnectOptions;
use sqlx::Row;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use tracing::{info, warn};

use crate::MemoryResult;
use crate::audience::Audience;
use crate::embed::Embedder;
use crate::facts::{self, Fact, NewFact};
use crate::files::{MemoryFiles, chunk_markdown};
use crate::scope::Scope;
use crate::search::{self, IndexChunk, Recall};
use crate::vector;

#[derive(Clone)]
pub struct MemoryEngine {
    pool: Arc<SqlitePool>,
    files: MemoryFiles,
    embedder: Option<Arc<dyn Embedder>>,
    note_half_life_days: f64,
}

impl MemoryEngine {
    pub async fn open(
        path: &Path,
        goat_root: &Path,
        embedder: Option<Arc<dyn Embedder>>,
        note_half_life_days: f64,
    ) -> MemoryResult<Self> {
        goat_sqlite_vec::register();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let options = format!("sqlite://{}", path.display())
            .parse::<sqlx::sqlite::SqliteConnectOptions>()?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .busy_timeout(std::time::Duration::from_secs(5))
            .disable_statement_logging();
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await?;
        crate::run_migrations(&pool).await?;
        let engine = Self {
            pool: Arc::new(pool),
            files: MemoryFiles::new(goat_root),
            embedder,
            note_half_life_days,
        };
        engine.ensure_vector_index().await?;
        Ok(engine)
    }

    pub fn files(&self) -> &MemoryFiles {
        &self.files
    }

    #[cfg(test)]
    pub(crate) fn pool(&self) -> Arc<SqlitePool> {
        self.pool.clone()
    }

    fn dim(&self) -> Option<usize> {
        self.embedder.as_ref().map(|e| e.dim())
    }

    async fn embed(&self, text: &str) -> Option<Vec<f32>> {
        match &self.embedder {
            None => None,
            Some(e) => match e.embed(text).await {
                Ok(v) => Some(v),
                Err(err) => {
                    warn!(error = ?err, "embedding failed; indexing without vector");
                    None
                }
            },
        }
    }

    async fn ensure_vector_index(&self) -> MemoryResult<()> {
        let Some(dim) = self.dim() else {
            return Ok(());
        };
        let model = self.embedder.as_ref().map_or("none", |_| "configured");

        let existing: Option<(String, i64)> =
            sqlx::query_as("SELECT embed_model, embed_dim FROM mem_index_meta WHERE id = 1")
                .fetch_optional(&*self.pool)
                .await?;

        let needs_rebuild = match &existing {
            Some((_m, d)) => *d as usize != dim,
            None => false,
        };

        if needs_rebuild {
            info!("embedding dimension changed; rebuilding vector index");
            vector::drop_vec_table(&self.pool).await?;
        }
        vector::ensure_vec_table(&self.pool, dim).await?;

        if existing.is_none() || needs_rebuild {
            let now = Utc::now().to_rfc3339();
            sqlx::query(
                "INSERT INTO mem_index_meta (id, embed_model, embed_dim, built_at) \
                 VALUES (1, ?, ?, ?) \
                 ON CONFLICT(id) DO UPDATE SET \
                   embed_model = excluded.embed_model, \
                   embed_dim = excluded.embed_dim, built_at = excluded.built_at",
            )
            .bind(model)
            .bind(i64::try_from(dim).unwrap_or(i64::MAX))
            .bind(&now)
            .execute(&*self.pool)
            .await?;
            if needs_rebuild {
                self.reindex().await?;
            }
        }
        Ok(())
    }

    pub async fn reindex_file(&self, scope: &Scope, rel: &str) -> MemoryResult<()> {
        search::delete_source(&self.pool, scope, rel).await?;
        let Ok(content) = self.files.view(scope, rel, None).await else {
            return Ok(());
        };
        let kind = kind_for(rel);
        for chunk in chunk_markdown(&content) {
            let chunk_key = format!("{rel}#{}", chunk.heading);
            let embedding = self.embed(&chunk.text).await;
            let ic = IndexChunk {
                scope: scope.clone(),
                audience: Audience::global(),
                kind: kind.to_string(),
                source_ref: rel.to_string(),
                chunk_key,
                chunk_no: i64::try_from(chunk.chunk_no).unwrap_or(i64::MAX),
                text: chunk.text,
            };
            search::insert_chunk(&self.pool, &ic, embedding.as_deref()).await?;
        }
        Ok(())
    }

    pub async fn assert_fact(&self, new: &NewFact) -> MemoryResult<i64> {
        let id = facts::assert_fact(&self.pool, new).await?;
        let source_ref = format!("fact:{id}");
        let embedding = self.embed(&new.text).await;
        let ic = IndexChunk {
            scope: new.scope.clone(),
            audience: new.audience.clone(),
            kind: "fact".into(),
            source_ref: source_ref.clone(),
            chunk_key: source_ref,
            chunk_no: 0,
            text: new.text.clone(),
        };
        search::insert_chunk(&self.pool, &ic, embedding.as_deref()).await?;
        Ok(id)
    }

    pub async fn invalidate_fact(
        &self,
        fact_id: i64,
        superseded_by: Option<i64>,
    ) -> MemoryResult<()> {
        facts::invalidate(&self.pool, fact_id, superseded_by).await?;
        if let Some(scope_key) = self.fact_scope(fact_id).await? {
            let scope: Scope = scope_key.parse().unwrap_or(Scope::Owner);
            search::delete_source(&self.pool, &scope, &format!("fact:{fact_id}")).await?;
        }
        Ok(())
    }

    async fn fact_scope(&self, fact_id: i64) -> MemoryResult<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as("SELECT scope FROM facts WHERE id = ?")
            .bind(fact_id)
            .fetch_optional(&*self.pool)
            .await?;
        Ok(row.map(|(s,)| s))
    }

    pub async fn current_facts(
        &self,
        audience: &Audience,
        scope: &Scope,
        subject: Option<&str>,
        limit: usize,
    ) -> MemoryResult<Vec<Fact>> {
        facts::current_facts(&self.pool, audience, scope, subject, limit).await
    }

    pub async fn decay_scope(&self, scope: &Scope, factor: f32) -> MemoryResult<u64> {
        facts::decay_strength(&self.pool, scope, factor).await
    }

    pub async fn append_file(&self, scope: &Scope, rel: &str, text: &str) -> MemoryResult<()> {
        let existing = self.files.view(scope, rel, None).await.unwrap_or_default();
        let joined = if existing.trim().is_empty() {
            text.to_string()
        } else {
            format!("{}\n{}", existing.trim_end(), text)
        };
        self.files
            .write(scope, rel, &joined)
            .await
            .map_err(|e| crate::MemoryError::File(e.to_string()))?;
        self.reindex_file(scope, rel).await
    }

    pub async fn recall(
        &self,
        audience: &Audience,
        scopes: &[Scope],
        query_text: &str,
        k: usize,
    ) -> MemoryResult<Vec<Recall>> {
        let embedding = self.embed(query_text).await;
        let hits = search::recall(
            &self.pool,
            audience,
            scopes,
            query_text,
            embedding.as_deref(),
            k,
            self.note_half_life_days,
        )
        .await?;
        for h in &hits {
            let _ = search::record_recall(&self.pool, &h.scope, &h.chunk_key).await;
        }
        Ok(hits)
    }

    pub async fn reindex(&self) -> MemoryResult<()> {
        sqlx::query("DELETE FROM mem_index")
            .execute(&*self.pool)
            .await?;
        sqlx::query("DELETE FROM mem_fts")
            .execute(&*self.pool)
            .await?;
        if self.dim().is_some() {
            vector::drop_vec_table(&self.pool).await?;
            vector::ensure_vec_table(&self.pool, self.dim().unwrap()).await?;
        }

        for scope in self.scopes().await? {
            for rel in self.files.list(&scope).await.unwrap_or_default() {
                self.reindex_file(&scope, &rel).await?;
            }
        }

        let rows = sqlx::query(
            "SELECT id, scope, audience_kind, audience_ref, text \
             FROM facts WHERE invalid_at IS NULL",
        )
        .fetch_all(&*self.pool)
        .await?;
        for r in rows {
            let id: i64 = r.get(0);
            let scope: Scope = r.get::<String, _>(1).parse().unwrap_or(Scope::Owner);
            let audience_kind: String = r.get(2);
            let audience_ref: Option<String> = r.get(3);
            let audience =
                Audience::from_parts(&audience_kind, audience_ref.clone()).ok_or_else(|| {
                    crate::MemoryError::InvalidAudience {
                        kind: audience_kind,
                        reference: audience_ref,
                    }
                })?;
            let text: String = r.get(4);
            let source_ref = format!("fact:{id}");
            let embedding = self.embed(&text).await;
            let ic = IndexChunk {
                scope,
                audience,
                kind: "fact".into(),
                source_ref: source_ref.clone(),
                chunk_key: source_ref,
                chunk_no: 0,
                text,
            };
            search::insert_chunk(&self.pool, &ic, embedding.as_deref()).await?;
        }
        info!("memory index rebuilt");
        Ok(())
    }

    pub async fn scopes(&self) -> MemoryResult<Vec<Scope>> {
        let mut scopes = vec![Scope::Owner, Scope::Self_];
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT DISTINCT scope FROM facts WHERE scope LIKE 'domain:%'")
                .fetch_all(&*self.pool)
                .await?;
        for (key,) in rows {
            if let Ok(s) = key.parse::<Scope>()
                && !scopes.contains(&s)
            {
                scopes.push(s);
            }
        }
        if let Ok(entries) = std::fs::read_dir(
            self.files
                .scope_dir(&Scope::Owner)
                .parent()
                .unwrap()
                .join("domain"),
        ) {
            for e in entries.flatten() {
                if e.path().is_dir()
                    && let Some(name) = e.file_name().to_str()
                    && let Ok(s) = Scope::domain(name)
                    && !scopes.contains(&s)
                {
                    scopes.push(s);
                }
            }
        }
        Ok(scopes)
    }
}

fn kind_for(rel: &str) -> &'static str {
    if rel.starts_with("core/") || rel == "core" {
        "core"
    } else if rel.starts_with("notes/") {
        "note"
    } else if rel.starts_with("journal") {
        "journal"
    } else {
        "note"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::FactOrigin;
    use async_trait::async_trait;

    async fn engine() -> (tempfile::TempDir, MemoryEngine) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("goat.db");
        let eng = MemoryEngine::open(&path, dir.path(), None, 180.0)
            .await
            .unwrap();
        (dir, eng)
    }

    struct KeywordEmbedder;

    #[async_trait]
    impl Embedder for KeywordEmbedder {
        fn dim(&self) -> usize {
            3
        }
        async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
            let t = text.to_lowercase();
            Ok(vec![
                if t.contains("budget") { 1.0 } else { 0.0 },
                if t.contains("lunch") { 1.0 } else { 0.0 },
                0.1,
            ])
        }
    }

    async fn engine_with_embedder() -> (tempfile::TempDir, MemoryEngine) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("goat.db");
        let eng = MemoryEngine::open(&path, dir.path(), Some(Arc::new(KeywordEmbedder)), 180.0)
            .await
            .unwrap();
        (dir, eng)
    }

    #[tokio::test]
    async fn hybrid_engine_recall_with_embedder() {
        let (_d, eng) = engine_with_embedder().await;
        eng.files()
            .write(
                &Scope::Owner,
                "notes/2026/2026-07-07.md",
                "## A\nthe quarterly budget review",
            )
            .await
            .unwrap();
        eng.files()
            .write(
                &Scope::Owner,
                "notes/2026/2026-07-08.md",
                "## B\ncasual lunch chat",
            )
            .await
            .unwrap();
        eng.reindex().await.unwrap();
        let hits = eng
            .recall(&Audience::global(), &[Scope::Owner], "budget", 5)
            .await
            .unwrap();
        assert!(!hits.is_empty());
        assert!(
            hits[0].text.contains("budget"),
            "vector+fts should rank budget note first"
        );
    }

    #[tokio::test]
    async fn file_write_then_recall() {
        let (_d, eng) = engine().await;
        eng.files()
            .write(
                &Scope::Owner,
                "core/profile.md",
                "## Profile\nThe owner loves sailing",
            )
            .await
            .unwrap();
        eng.reindex_file(&Scope::Owner, "core/profile.md")
            .await
            .unwrap();
        let hits = eng
            .recall(&Audience::global(), &[Scope::Owner], "sailing", 5)
            .await
            .unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].kind, "core");
    }

    #[tokio::test]
    async fn fact_assert_recall_and_invalidate() {
        let (_d, eng) = engine().await;
        let nf = NewFact {
            scope: Scope::Owner,
            audience: Audience::global(),
            subject: Some("car".into()),
            text: "drives a blue truck".into(),
            origin: FactOrigin::OwnerStated,
            source_kind: "message".into(),
            source_ref: "m1".into(),
            importance: 0.6,
        };
        let id = eng.assert_fact(&nf).await.unwrap();
        let hits = eng
            .recall(&Audience::global(), &[Scope::Owner], "truck", 5)
            .await
            .unwrap();
        assert!(hits.iter().any(|h| h.kind == "fact"));

        eng.invalidate_fact(id, None).await.unwrap();
        let after = eng
            .recall(&Audience::global(), &[Scope::Owner], "truck", 5)
            .await
            .unwrap();
        assert!(
            after.iter().all(|h| h.kind != "fact"),
            "invalidated fact left the index"
        );
    }

    #[tokio::test]
    async fn private_fact_is_recalled_only_for_its_principal() {
        let (_d, eng) = engine().await;
        let person_a = Audience::principal("person-a").unwrap();
        let person_b = Audience::principal("person-b").unwrap();
        let fact = NewFact {
            scope: Scope::Owner,
            audience: person_a.clone(),
            subject: None,
            text: "the launch phrase is cedar".into(),
            origin: FactOrigin::OwnerStated,
            source_kind: "message".into(),
            source_ref: "message-a".into(),
            importance: 0.8,
        };
        eng.assert_fact(&fact).await.unwrap();

        let visible = eng
            .recall(&person_a, &[Scope::Owner], "cedar", 5)
            .await
            .unwrap();
        let hidden = eng
            .recall(&person_b, &[Scope::Owner], "cedar", 5)
            .await
            .unwrap();

        assert_eq!(visible.len(), 1);
        assert!(hidden.is_empty());
    }

    #[tokio::test]
    async fn shared_fact_is_recalled_only_in_its_context() {
        let (_d, eng) = engine().await;
        let room_a = Audience::shared("room-a").unwrap();
        let room_b = Audience::shared("room-b").unwrap();
        let fact = NewFact {
            scope: Scope::Owner,
            audience: room_a.clone(),
            subject: None,
            text: "the team launch phrase is maple".into(),
            origin: FactOrigin::OwnerStated,
            source_kind: "message".into(),
            source_ref: "message-shared".into(),
            importance: 0.8,
        };
        eng.assert_fact(&fact).await.unwrap();

        let visible = eng
            .recall(&room_a, &[Scope::Owner], "maple", 5)
            .await
            .unwrap();
        let hidden = eng
            .recall(&room_b, &[Scope::Owner], "maple", 5)
            .await
            .unwrap();

        assert_eq!(visible.len(), 1);
        assert!(hidden.is_empty());
    }

    #[tokio::test]
    async fn populated_database_migrates_existing_rows_as_global() {
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
            "CREATE TABLE _sqlx_migrations_memory (
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
        let initial = migrator
            .iter()
            .find(|migration| migration.version == 11)
            .unwrap();
        sqlx::raw_sql(initial.sql.clone())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO _sqlx_migrations_memory
             (version, description, success, checksum, execution_time)
             VALUES (?, ?, TRUE, ?, 0)",
        )
        .bind(initial.version)
        .bind(initial.description.as_ref())
        .bind(initial.checksum.as_ref())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO facts
             (id, scope, subject, text, origin, source_kind, source_ref, stated_at,
              valid_from, importance, strength)
             VALUES (7, 'owner', 'profile', 'legacy fact', 'owner_stated', 'message',
                     'old-message', '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z', 0.7, 1.0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO mem_index
             (id, scope, kind, source_ref, chunk_key, chunk_no, text, updated_at)
             VALUES (9, 'owner', 'fact', 'fact:7', 'fact:7', 0, 'legacy fact',
                     '2026-08-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO mem_fts (rowid, text, scope, index_id)
             VALUES (9, 'legacy fact', 'owner', 9)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;

        let eng = MemoryEngine::open(&path, dir.path(), None, 180.0)
            .await
            .unwrap();
        let fact_rows: Vec<(i64, String, String, Option<String>)> =
            sqlx::query_as("SELECT id, text, audience_kind, audience_ref FROM facts ORDER BY id")
                .fetch_all(&*eng.pool)
                .await
                .unwrap();
        let index_rows: Vec<(i64, String, String, Option<String>)> = sqlx::query_as(
            "SELECT id, text, audience_kind, audience_ref FROM mem_index ORDER BY id",
        )
        .fetch_all(&*eng.pool)
        .await
        .unwrap();

        assert_eq!(
            fact_rows,
            vec![(7, "legacy fact".into(), "global".into(), None)]
        );
        assert_eq!(
            index_rows,
            vec![(9, "legacy fact".into(), "global".into(), None)]
        );
        let visible = eng
            .current_facts(
                &Audience::principal("new-person").unwrap(),
                &Scope::Owner,
                None,
                10,
            )
            .await
            .unwrap();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, 7);
        for audience in [
            Audience::principal("new-person").unwrap(),
            Audience::shared("new-room").unwrap(),
        ] {
            let recalled = eng
                .recall(&audience, &[Scope::Owner], "legacy fact", 5)
                .await
                .unwrap();
            assert_eq!(recalled.len(), 1);
            assert_eq!(recalled[0].index_id, 9);
        }
    }

    #[tokio::test]
    async fn scopes_include_domains_with_files() {
        let (_d, eng) = engine().await;
        let scope = Scope::domain("linear").unwrap();
        eng.files()
            .write(&scope, "notes/context.md", "## ctx\nx")
            .await
            .unwrap();
        let scopes = eng.scopes().await.unwrap();
        assert!(scopes.contains(&Scope::Owner));
        assert!(scopes.contains(&Scope::Self_));
        assert!(scopes.contains(&scope));
    }

    #[tokio::test]
    async fn reindex_rebuilds_from_sources() {
        let (_d, eng) = engine().await;
        eng.files()
            .write(
                &Scope::Self_,
                "notes/2026/2026-07-07.md",
                "## Log\nshipped the release",
            )
            .await
            .unwrap();
        eng.reindex().await.unwrap();
        let hits = eng
            .recall(&Audience::global(), &[Scope::Self_], "release", 5)
            .await
            .unwrap();
        assert!(!hits.is_empty());
    }
}
