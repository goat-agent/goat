use chrono::{DateTime, Utc};
use sqlx::Row;
use sqlx::sqlite::SqlitePool;

use crate::MemoryResult;
use crate::scope::Scope;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FactOrigin {
    OwnerStated,
    Inferred,
    Consolidated,
}

impl FactOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OwnerStated => "owner_stated",
            Self::Inferred => "inferred",
            Self::Consolidated => "consolidated",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "owner_stated" => Self::OwnerStated,
            "consolidated" => Self::Consolidated,
            _ => Self::Inferred,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Fact {
    pub id: i64,
    pub scope: Scope,
    pub subject: Option<String>,
    pub text: String,
    pub origin: FactOrigin,
    pub source_kind: String,
    pub source_ref: String,
    pub stated_at: DateTime<Utc>,
    pub valid_from: Option<DateTime<Utc>>,
    pub invalid_at: Option<DateTime<Utc>>,
    pub superseded_by: Option<i64>,
    pub importance: f32,
    pub strength: f32,
}

#[derive(Clone, Debug)]
pub struct NewFact {
    pub scope: Scope,
    pub subject: Option<String>,
    pub text: String,
    pub origin: FactOrigin,
    pub source_kind: String,
    pub source_ref: String,
    pub importance: f32,
}

pub async fn assert_fact(pool: &SqlitePool, new: &NewFact) -> MemoryResult<i64> {
    let now = Utc::now().to_rfc3339();
    let id = sqlx::query(
        "INSERT INTO facts \
         (scope, subject, text, origin, source_kind, source_ref, stated_at, valid_from, importance, strength) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1.0)",
    )
    .bind(new.scope.as_key())
    .bind(new.subject.as_deref())
    .bind(&new.text)
    .bind(new.origin.as_str())
    .bind(&new.source_kind)
    .bind(&new.source_ref)
    .bind(&now)
    .bind(&now)
    .bind(new.importance as f64)
    .execute(pool)
    .await?
    .last_insert_rowid();
    Ok(id)
}

pub async fn invalidate(
    pool: &SqlitePool,
    fact_id: i64,
    superseded_by: Option<i64>,
) -> MemoryResult<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE facts SET invalid_at = ?, superseded_by = ? \
         WHERE id = ? AND invalid_at IS NULL",
    )
    .bind(&now)
    .bind(superseded_by)
    .bind(fact_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn current_facts(
    pool: &SqlitePool,
    scope: &Scope,
    subject: Option<&str>,
    limit: usize,
) -> MemoryResult<Vec<Fact>> {
    let rows = if let Some(subj) = subject {
        sqlx::query(
            "SELECT id, scope, subject, text, origin, source_kind, source_ref, stated_at, \
             valid_from, invalid_at, superseded_by, importance, strength \
             FROM facts WHERE scope = ? AND subject = ? AND invalid_at IS NULL \
             ORDER BY importance DESC, id DESC LIMIT ?",
        )
        .bind(scope.as_key())
        .bind(subj)
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query(
            "SELECT id, scope, subject, text, origin, source_kind, source_ref, stated_at, \
             valid_from, invalid_at, superseded_by, importance, strength \
             FROM facts WHERE scope = ? AND invalid_at IS NULL \
             ORDER BY importance DESC, id DESC LIMIT ?",
        )
        .bind(scope.as_key())
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(pool)
        .await?
    };
    Ok(rows.into_iter().map(|r| row_to_fact(&r)).collect())
}

pub async fn decay_strength(pool: &SqlitePool, scope: &Scope, factor: f32) -> MemoryResult<u64> {
    let res = sqlx::query(
        "UPDATE facts SET strength = strength * ? WHERE scope = ? AND invalid_at IS NULL",
    )
    .bind(factor as f64)
    .bind(scope.as_key())
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

fn parse_ts(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).map_or_else(|_| Utc::now(), |d| d.with_timezone(&Utc))
}

fn opt_ts(s: Option<&str>) -> Option<DateTime<Utc>> {
    s.map(parse_ts)
}

fn row_to_fact(r: &sqlx::sqlite::SqliteRow) -> Fact {
    let scope_key: String = r.get(1);
    let scope = scope_key.parse::<Scope>().unwrap_or(Scope::Owner);
    Fact {
        id: r.get(0),
        scope,
        subject: r.get(2),
        text: r.get(3),
        origin: FactOrigin::parse(&r.get::<String, _>(4)),
        source_kind: r.get(5),
        source_ref: r.get(6),
        stated_at: parse_ts(&r.get::<String, _>(7)),
        valid_from: opt_ts(r.get::<Option<String>, _>(8).as_deref()),
        invalid_at: opt_ts(r.get::<Option<String>, _>(9).as_deref()),
        superseded_by: r.get(10),
        importance: r.get::<f64, _>(11) as f32,
        strength: r.get::<f64, _>(12) as f32,
    }
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

    fn nf(text: &str, origin: FactOrigin) -> NewFact {
        NewFact {
            scope: Scope::Owner,
            subject: Some("wife".into()),
            text: text.into(),
            origin,
            source_kind: "message".into(),
            source_ref: "m1".into(),
            importance: 0.7,
        }
    }

    #[tokio::test]
    async fn assert_then_current_lists_it() {
        let p = pool().await;
        let id = assert_fact(&p, &nf("birthday is in March", FactOrigin::OwnerStated))
            .await
            .unwrap();
        assert!(id > 0);
        let cur = current_facts(&p, &Scope::Owner, None, 10).await.unwrap();
        assert_eq!(cur.len(), 1);
        assert_eq!(cur[0].text, "birthday is in March");
        assert!(cur[0].invalid_at.is_none());
    }

    #[tokio::test]
    async fn contradiction_invalidates_old_keeps_history() {
        let p = pool().await;
        let old = assert_fact(&p, &nf("birthday is in March", FactOrigin::Inferred))
            .await
            .unwrap();
        let new = assert_fact(&p, &nf("birthday is in April", FactOrigin::OwnerStated))
            .await
            .unwrap();
        invalidate(&p, old, Some(new)).await.unwrap();

        let cur = current_facts(&p, &Scope::Owner, None, 10).await.unwrap();
        assert_eq!(cur.len(), 1);
        assert_eq!(cur[0].id, new);
        assert_eq!(cur[0].text, "birthday is in April");

        let all: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM facts")
            .fetch_one(&*p)
            .await
            .unwrap();
        assert_eq!(all.0, 2);
    }

    #[tokio::test]
    async fn scope_and_subject_filtering() {
        let p = pool().await;
        assert_fact(&p, &nf("owner wife fact", FactOrigin::Inferred))
            .await
            .unwrap();
        let mut other = nf("dev fact", FactOrigin::Inferred);
        other.scope = Scope::domain("dev").unwrap();
        other.subject = None;
        assert_fact(&p, &other).await.unwrap();

        let owner = current_facts(&p, &Scope::Owner, None, 10).await.unwrap();
        assert_eq!(owner.len(), 1);
        let dev = current_facts(&p, &Scope::domain("dev").unwrap(), None, 10)
            .await
            .unwrap();
        assert_eq!(dev.len(), 1);
        let by_subj = current_facts(&p, &Scope::Owner, Some("wife"), 10)
            .await
            .unwrap();
        assert_eq!(by_subj.len(), 1);
        let no_subj = current_facts(&p, &Scope::Owner, Some("nobody"), 10)
            .await
            .unwrap();
        assert!(no_subj.is_empty());
    }
}
