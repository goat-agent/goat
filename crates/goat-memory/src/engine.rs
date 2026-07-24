use std::path::Path;
use std::sync::Arc;

use chrono::Utc;
use sqlx::Row;
use sqlx::sqlite::SqlitePool;
use tracing::{info, warn};

use crate::MemoryResult;
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
        pool: Arc<SqlitePool>,
        goat_root: &Path,
        embedder: Option<Arc<dyn Embedder>>,
        note_half_life_days: f64,
    ) -> MemoryResult<Self> {
        let engine = Self {
            pool,
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
        scope: &Scope,
        subject: Option<&str>,
        limit: usize,
    ) -> MemoryResult<Vec<Fact>> {
        facts::current_facts(&self.pool, scope, subject, limit).await
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
        scopes: &[Scope],
        query_text: &str,
        k: usize,
    ) -> MemoryResult<Vec<Recall>> {
        let embedding = self.embed(query_text).await;
        let hits = search::recall(
            &self.pool,
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

        for scope in self.all_scopes().await? {
            for rel in self.files.list(&scope).await.unwrap_or_default() {
                self.reindex_file(&scope, &rel).await?;
            }
        }

        let rows = sqlx::query("SELECT id, scope, text FROM facts WHERE invalid_at IS NULL")
            .fetch_all(&*self.pool)
            .await?;
        for r in rows {
            let id: i64 = r.get(0);
            let scope: Scope = r.get::<String, _>(1).parse().unwrap_or(Scope::Owner);
            let text: String = r.get(2);
            let source_ref = format!("fact:{id}");
            let embedding = self.embed(&text).await;
            let ic = IndexChunk {
                scope,
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

    async fn all_scopes(&self) -> MemoryResult<Vec<Scope>> {
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
    use goat_store::SqliteStore;

    async fn engine() -> (tempfile::TempDir, MemoryEngine) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("goat.db");
        let pool = SqliteStore::open(&path).await.unwrap().pool();
        let eng = MemoryEngine::open(pool, dir.path(), None, 180.0)
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
        let pool = SqliteStore::open(&path).await.unwrap().pool();
        let eng = MemoryEngine::open(pool, dir.path(), Some(Arc::new(KeywordEmbedder)), 180.0)
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
        let hits = eng.recall(&[Scope::Owner], "budget", 5).await.unwrap();
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
        let hits = eng.recall(&[Scope::Owner], "sailing", 5).await.unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].kind, "core");
    }

    #[tokio::test]
    async fn fact_assert_recall_and_invalidate() {
        let (_d, eng) = engine().await;
        let nf = NewFact {
            scope: Scope::Owner,
            subject: Some("car".into()),
            text: "drives a blue truck".into(),
            origin: FactOrigin::OwnerStated,
            source_kind: "message".into(),
            source_ref: "m1".into(),
            importance: 0.6,
        };
        let id = eng.assert_fact(&nf).await.unwrap();
        let hits = eng.recall(&[Scope::Owner], "truck", 5).await.unwrap();
        assert!(hits.iter().any(|h| h.kind == "fact"));

        eng.invalidate_fact(id, None).await.unwrap();
        let after = eng.recall(&[Scope::Owner], "truck", 5).await.unwrap();
        assert!(
            after.iter().all(|h| h.kind != "fact"),
            "invalidated fact left the index"
        );
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
        let hits = eng.recall(&[Scope::Self_], "release", 5).await.unwrap();
        assert!(!hits.is_empty());
    }
}
