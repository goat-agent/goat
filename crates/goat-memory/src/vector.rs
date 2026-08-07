use sqlx::Row;
use sqlx::sqlite::SqlitePool;
use zerocopy::AsBytes;

use crate::MemoryResult;

pub fn embedding_bytes(embedding: &[f32]) -> Vec<u8> {
    embedding.as_bytes().to_vec()
}

pub async fn ensure_vec_table(pool: &SqlitePool, dim: usize) -> MemoryResult<()> {
    let sql = format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS mem_vec USING vec0(\
         index_id INTEGER PRIMARY KEY, scope TEXT PARTITION KEY, embedding float[{dim}])"
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn drop_vec_table(pool: &SqlitePool) -> MemoryResult<()> {
    sqlx::query("DROP TABLE IF EXISTS mem_vec")
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn upsert_vector(
    pool: &SqlitePool,
    index_id: i64,
    scope_key: &str,
    embedding: &[f32],
) -> MemoryResult<()> {
    sqlx::query("DELETE FROM mem_vec WHERE index_id = ?")
        .bind(index_id)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO mem_vec(index_id, scope, embedding) VALUES (?, ?, ?)")
        .bind(index_id)
        .bind(scope_key)
        .bind(embedding_bytes(embedding))
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_vector(pool: &SqlitePool, index_id: i64) -> MemoryResult<()> {
    sqlx::query("DELETE FROM mem_vec WHERE index_id = ?")
        .bind(index_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[derive(Clone, Debug)]
pub struct VecHit {
    pub index_id: i64,
    pub distance: f64,
}

pub async fn knn_in_scope(
    pool: &SqlitePool,
    scope_key: &str,
    query: &[f32],
    k: usize,
) -> MemoryResult<Vec<VecHit>> {
    let rows = sqlx::query(
        "SELECT index_id, distance FROM mem_vec \
         WHERE scope = ? AND embedding MATCH ? AND k = ? ORDER BY distance",
    )
    .bind(scope_key)
    .bind(embedding_bytes(query))
    .bind(i64::try_from(k).unwrap_or(i64::MAX))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| VecHit {
            index_id: r.get::<i64, _>(0),
            distance: r.get::<f64, _>(1),
        })
        .collect())
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
        engine.pool()
    }

    #[tokio::test]
    async fn vec_table_knn_partitioned_by_scope() {
        let p = pool().await;
        ensure_vec_table(&p, 3).await.unwrap();

        upsert_vector(&p, 1, "owner", &[0.1, 0.1, 0.1])
            .await
            .unwrap();
        upsert_vector(&p, 2, "owner", &[0.9, 0.1, 0.1])
            .await
            .unwrap();
        upsert_vector(&p, 3, "domain:dev", &[0.1, 0.9, 0.1])
            .await
            .unwrap();

        let hits = knn_in_scope(&p, "owner", &[0.12, 0.1, 0.1], 2)
            .await
            .unwrap();
        assert_eq!(hits.len(), 2, "only owner-partition rows");
        assert_eq!(hits[0].index_id, 1, "nearest is id=1");
        assert!(hits[0].distance < hits[1].distance);

        let dev = knn_in_scope(&p, "domain:dev", &[0.1, 0.9, 0.1], 5)
            .await
            .unwrap();
        assert_eq!(dev.len(), 1);
        assert_eq!(dev[0].index_id, 3);
    }

    #[tokio::test]
    async fn upsert_replaces_and_delete_removes() {
        let p = pool().await;
        ensure_vec_table(&p, 2).await.unwrap();
        upsert_vector(&p, 7, "self", &[1.0, 0.0]).await.unwrap();
        upsert_vector(&p, 7, "self", &[0.0, 1.0]).await.unwrap();
        let hits = knn_in_scope(&p, "self", &[0.0, 1.0], 5).await.unwrap();
        assert_eq!(hits.len(), 1, "upsert must not duplicate index_id");
        delete_vector(&p, 7).await.unwrap();
        let after = knn_in_scope(&p, "self", &[0.0, 1.0], 5).await.unwrap();
        assert!(after.is_empty());
    }
}
