use chrono::{DateTime, Utc};
use sqlx::Row;
use sqlx::sqlite::SqlitePool;

use crate::MemoryResult;
use crate::scope::Scope;
use crate::vector;

#[derive(Clone, Debug)]
pub struct IndexChunk {
    pub scope: Scope,
    pub kind: String,
    pub source_ref: String,
    pub chunk_key: String,
    pub chunk_no: i64,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct Recall {
    pub index_id: i64,
    pub scope: Scope,
    pub kind: String,
    pub source_ref: String,
    pub chunk_key: String,
    pub text: String,
    pub score: f64,
}

pub async fn delete_source(pool: &SqlitePool, scope: &Scope, source_ref: &str) -> MemoryResult<()> {
    let ids: Vec<(i64,)> =
        sqlx::query_as("SELECT id FROM mem_index WHERE scope = ? AND source_ref = ?")
            .bind(scope.as_key())
            .bind(source_ref)
            .fetch_all(pool)
            .await?;
    for (id,) in &ids {
        sqlx::query("DELETE FROM mem_fts WHERE index_id = ?")
            .bind(id)
            .execute(pool)
            .await?;
        let _ = vector::delete_vector(pool, *id).await;
    }
    sqlx::query("DELETE FROM mem_index WHERE scope = ? AND source_ref = ?")
        .bind(scope.as_key())
        .bind(source_ref)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn insert_chunk(
    pool: &SqlitePool,
    chunk: &IndexChunk,
    embedding: Option<&[f32]>,
) -> MemoryResult<i64> {
    let now = Utc::now().to_rfc3339();
    let id = sqlx::query(
        "INSERT INTO mem_index (scope, kind, source_ref, chunk_key, chunk_no, text, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(chunk.scope.as_key())
    .bind(&chunk.kind)
    .bind(&chunk.source_ref)
    .bind(&chunk.chunk_key)
    .bind(chunk.chunk_no)
    .bind(&chunk.text)
    .bind(&now)
    .execute(pool)
    .await?
    .last_insert_rowid();

    sqlx::query("INSERT INTO mem_fts (rowid, text, scope, index_id) VALUES (?, ?, ?, ?)")
        .bind(id)
        .bind(&chunk.text)
        .bind(chunk.scope.as_key())
        .bind(id)
        .execute(pool)
        .await?;

    if let Some(emb) = embedding {
        vector::upsert_vector(pool, id, &chunk.scope.as_key(), emb).await?;
    }
    Ok(id)
}

async fn fts_search(
    pool: &SqlitePool,
    scope: &Scope,
    query: &str,
    limit: usize,
) -> MemoryResult<Vec<(i64, usize)>> {
    let escaped = format!("\"{}\"", query.replace('"', "\"\""));
    let rows = sqlx::query(
        "SELECT index_id FROM mem_fts \
         WHERE scope = ? AND mem_fts MATCH ? ORDER BY bm25(mem_fts) LIMIT ?",
    )
    .bind(scope.as_key())
    .bind(escaped)
    .bind(i64::try_from(limit).unwrap_or(i64::MAX))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .enumerate()
        .map(|(rank, r)| (r.get::<i64, _>(0), rank + 1))
        .collect())
}

pub async fn recall(
    pool: &SqlitePool,
    scopes: &[Scope],
    query_text: &str,
    query_embedding: Option<&[f32]>,
    k: usize,
    note_half_life_days: f64,
) -> MemoryResult<Vec<Recall>> {
    use std::collections::HashMap;
    const RRF_K: f64 = 60.0;
    let pool_limit = (k * 4).max(20);

    let mut id_scores: HashMap<i64, f64> = HashMap::new();

    for scope in scopes {
        for (id, rank) in fts_search(pool, scope, query_text, pool_limit).await? {
            *id_scores.entry(id).or_insert(0.0) +=
                1.0 / (RRF_K + f64::from(u32::try_from(rank).unwrap_or(u32::MAX)));
        }
        if let Some(emb) = query_embedding {
            let hits = vector::knn_in_scope(pool, &scope.as_key(), emb, pool_limit).await?;
            for (rank, hit) in hits.iter().enumerate() {
                *id_scores.entry(hit.index_id).or_insert(0.0) +=
                    1.0 / (RRF_K + f64::from(u32::try_from(rank + 1).unwrap_or(u32::MAX)));
            }
        }
    }

    if id_scores.is_empty() {
        return Ok(Vec::new());
    }

    let now = Utc::now();
    let mut out: Vec<Recall> = Vec::new();
    for (id, base) in id_scores {
        let row = sqlx::query(
            "SELECT scope, kind, source_ref, chunk_key, text, updated_at \
             FROM mem_index WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;
        let Some(r) = row else { continue };
        let scope_key: String = r.get(0);
        let kind: String = r.get(1);
        let updated_at: String = r.get(5);
        let mut score = base;
        if kind == "note" || kind == "journal" {
            let age_days = age_in_days(&updated_at, now);
            let decay = 0.5f64.powf(age_days / note_half_life_days).max(0.2);
            score *= decay;
        }
        out.push(Recall {
            index_id: id,
            scope: scope_key.parse().unwrap_or(Scope::Owner),
            kind,
            source_ref: r.get(2),
            chunk_key: r.get(3),
            text: r.get(4),
            score,
        });
    }

    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out.truncate(k);
    Ok(out)
}

pub async fn record_recall(pool: &SqlitePool, scope: &Scope, chunk_key: &str) -> MemoryResult<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO recall_stats (scope, chunk_key, recall_count, last_recalled_at) \
         VALUES (?, ?, 1, ?) \
         ON CONFLICT(scope, chunk_key) DO UPDATE SET \
           recall_count = recall_count + 1, last_recalled_at = excluded.last_recalled_at",
    )
    .bind(scope.as_key())
    .bind(chunk_key)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}

fn age_in_days(ts: &str, now: DateTime<Utc>) -> f64 {
    DateTime::parse_from_rfc3339(ts)
        .map_or(0.0, |d| {
            let secs = (now - d.with_timezone(&Utc)).num_seconds();
            #[allow(clippy::cast_precision_loss)]
            let days = secs as f64 / 86_400.0;
            days
        })
        .max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> std::sync::Arc<SqlitePool> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("goat.db");
        let engine = crate::MemoryEngine::open(&path, dir.path(), None, 180.0)
            .await
            .unwrap();
        std::mem::forget(dir);
        let pool = engine.pool();
        vector::ensure_vec_table(&pool, 3).await.unwrap();
        pool
    }

    fn chunk(scope: Scope, kind: &str, key: &str, text: &str) -> IndexChunk {
        IndexChunk {
            scope,
            kind: kind.into(),
            source_ref: key.into(),
            chunk_key: key.into(),
            chunk_no: 0,
            text: text.into(),
        }
    }

    #[tokio::test]
    async fn fts_only_recall_when_no_embedding() {
        let p = pool().await;
        insert_chunk(
            &p,
            &chunk(Scope::Owner, "core", "a", "the owner likes cats"),
            None,
        )
        .await
        .unwrap();
        insert_chunk(
            &p,
            &chunk(Scope::Owner, "core", "b", "unrelated text about dogs"),
            None,
        )
        .await
        .unwrap();
        let hits = recall(&p, &[Scope::Owner], "cats", None, 5, 180.0)
            .await
            .unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].chunk_key, "a");
    }

    #[tokio::test]
    async fn hybrid_recall_fuses_fts_and_vec() {
        let p = pool().await;
        insert_chunk(
            &p,
            &chunk(Scope::Owner, "note", "n1", "meeting about the budget"),
            Some(&[0.1, 0.1, 0.9]),
        )
        .await
        .unwrap();
        insert_chunk(
            &p,
            &chunk(Scope::Owner, "note", "n2", "lunch plans"),
            Some(&[0.9, 0.1, 0.1]),
        )
        .await
        .unwrap();
        let hits = recall(
            &p,
            &[Scope::Owner],
            "budget",
            Some(&[0.1, 0.1, 0.9]),
            5,
            180.0,
        )
        .await
        .unwrap();
        assert_eq!(hits[0].chunk_key, "n1", "both signals favour n1");
    }

    #[tokio::test]
    async fn delete_source_clears_all_tables() {
        let p = pool().await;
        insert_chunk(
            &p,
            &chunk(Scope::Self_, "note", "s1", "hello world"),
            Some(&[1.0, 0.0, 0.0]),
        )
        .await
        .unwrap();
        delete_source(&p, &Scope::Self_, "s1").await.unwrap();
        let hits = recall(
            &p,
            &[Scope::Self_],
            "hello",
            Some(&[1.0, 0.0, 0.0]),
            5,
            180.0,
        )
        .await
        .unwrap();
        assert!(hits.is_empty());
        let n: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM mem_fts")
            .fetch_one(&*p)
            .await
            .unwrap();
        assert_eq!(n.0, 0);
    }

    #[tokio::test]
    async fn record_recall_accumulates() {
        let p = pool().await;
        record_recall(&p, &Scope::Owner, "k1").await.unwrap();
        record_recall(&p, &Scope::Owner, "k1").await.unwrap();
        let c: (i64,) = sqlx::query_as(
            "SELECT recall_count FROM recall_stats WHERE scope='owner' AND chunk_key='k1'",
        )
        .fetch_one(&*p)
        .await
        .unwrap();
        assert_eq!(c.0, 2);
    }
}
