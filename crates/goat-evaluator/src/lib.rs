use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use goat_llm::{LlmRequest, LlmResponse, Model, ProviderId};
use goat_types::PersonaId;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqlitePool;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EvalScore {
    pub helpful: f32,
    pub on_persona: f32,
    pub safe: f32,
    pub notes: String,
}

#[async_trait]
pub trait Evaluator: Send + Sync + 'static {
    async fn score(&self, req: &LlmRequest, resp: &LlmResponse) -> EvalScore;
}

pub struct NoopEvaluator;

#[async_trait]
impl Evaluator for NoopEvaluator {
    async fn score(&self, _req: &LlmRequest, _resp: &LlmResponse) -> EvalScore {
        EvalScore {
            helpful: 1.0,
            on_persona: 1.0,
            safe: 1.0,
            notes: String::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ModelScore {
    pub provider: ProviderId,
    pub model_id: String,
    pub n_calls: i64,
    pub score_sum: f64,
    pub latency_ms_sum: i64,
}

impl ModelScore {
    pub fn average(&self) -> f64 {
        if self.n_calls == 0 {
            0.0
        } else {
            self.score_sum / (self.n_calls as f64)
        }
    }
}

pub struct ModelScoreStore {
    pool: Arc<SqlitePool>,
}

impl ModelScoreStore {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    pub async fn record(
        &self,
        persona: PersonaId,
        model: &Model,
        score: &EvalScore,
        latency_ms: i64,
    ) -> anyhow::Result<()> {
        let combined = (score.helpful + score.on_persona + score.safe) / 3.0;
        sqlx::query(
            r#"INSERT INTO model_scores
               (persona_id, provider, model_id, n_calls, score_sum, latency_ms_sum, last_seen)
               VALUES (?, ?, ?, 1, ?, ?, ?)
               ON CONFLICT(persona_id, provider, model_id) DO UPDATE SET
                 n_calls = n_calls + 1,
                 score_sum = score_sum + excluded.score_sum,
                 latency_ms_sum = latency_ms_sum + excluded.latency_ms_sum,
                 last_seen = excluded.last_seen"#,
        )
        .bind(persona.to_string())
        .bind(model.provider.as_str())
        .bind(&model.id)
        .bind(combined as f64)
        .bind(latency_ms)
        .bind(Utc::now().to_rfc3339())
        .execute(&*self.pool)
        .await?;
        Ok(())
    }

    pub async fn for_persona(&self, persona: PersonaId) -> anyhow::Result<Vec<ModelScore>> {
        let rows = sqlx::query_as::<_, (String, String, i64, f64, i64)>(
            r#"SELECT provider, model_id, n_calls, score_sum, latency_ms_sum
               FROM model_scores
               WHERE persona_id = ?"#,
        )
        .bind(persona.to_string())
        .fetch_all(&*self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(provider, model_id, n_calls, score_sum, latency_ms_sum)| ModelScore {
                    provider: ProviderId::new(provider),
                    model_id,
                    n_calls,
                    score_sum,
                    latency_ms_sum,
                },
            )
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use goat_llm::{Model, ProviderId};
    use goat_types::PersonaId;
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;

    /// Creates an in-memory SQLite pool with the model_scores schema.
    async fn fresh_pool() -> Arc<sqlx::sqlite::SqlitePool> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite");
        sqlx::query(
            r#"CREATE TABLE model_scores (
               id              INTEGER PRIMARY KEY AUTOINCREMENT,
               persona_id      TEXT    NOT NULL,
               provider        TEXT    NOT NULL,
               model_id        TEXT    NOT NULL,
               n_calls         INTEGER NOT NULL DEFAULT 0,
               score_sum       REAL    NOT NULL DEFAULT 0.0,
               latency_ms_sum  INTEGER NOT NULL DEFAULT 0,
               last_seen       TEXT    NOT NULL,
               UNIQUE(persona_id, provider, model_id)
            )"#,
        )
        .execute(&pool)
        .await
        .expect("create model_scores");
        Arc::new(pool)
    }

    fn fixture_model() -> Model {
        Model::new(ProviderId::new("test-provider"), "test-model-v1")
    }

    fn fixture_score(helpful: f32, on_persona: f32, safe: f32) -> EvalScore {
        EvalScore {
            helpful,
            on_persona,
            safe,
            notes: String::new(),
        }
    }

    // ── ModelScore::average ────────────────────────────────────────────────────

    #[test]
    fn average_zero_calls_returns_zero() {
        let ms = ModelScore {
            provider: ProviderId::new("p"),
            model_id: "m".into(),
            n_calls: 0,
            score_sum: 0.0,
            latency_ms_sum: 0,
        };
        assert_eq!(ms.average(), 0.0, "n_calls=0 guard must return 0.0");
    }

    #[test]
    fn average_computes_correctly() {
        let ms = ModelScore {
            provider: ProviderId::new("p"),
            model_id: "m".into(),
            n_calls: 4,
            score_sum: 3.0,
            latency_ms_sum: 0,
        };
        let avg = ms.average();
        assert!(
            (avg - 0.75).abs() < 1e-9,
            "expected 0.75, got {avg}"
        );
    }

    // ── ModelScoreStore record + for_persona ────────────────────────────────────

    #[tokio::test]
    async fn record_and_retrieve_round_trip() {
        let pool = fresh_pool().await;
        let store = ModelScoreStore::new(pool);
        let persona = PersonaId::new();
        let model = fixture_model();

        store
            .record(persona, &model, &fixture_score(1.0, 1.0, 1.0), 100)
            .await
            .expect("first record");

        let scores = store.for_persona(persona).await.expect("for_persona");
        assert_eq!(scores.len(), 1);
        let s = &scores[0];
        assert_eq!(s.model_id, "test-model-v1");
        assert_eq!(s.n_calls, 1);
        assert_eq!(s.latency_ms_sum, 100);
        // combined = (1+1+1)/3 = 1.0
        let avg = s.average();
        assert!((avg - 1.0).abs() < 1e-6, "expected 1.0, got {avg}");
    }

    #[tokio::test]
    async fn record_upserts_accumulate() {
        let pool = fresh_pool().await;
        let store = ModelScoreStore::new(pool);
        let persona = PersonaId::new();
        let model = fixture_model();

        // combined score each time = (0.9+0.8+1.0)/3 ≈ 0.9
        for _ in 0..3 {
            store
                .record(persona, &model, &fixture_score(0.9, 0.8, 1.0), 50)
                .await
                .expect("record");
        }

        let scores = store.for_persona(persona).await.expect("for_persona");
        assert_eq!(scores.len(), 1, "upsert must accumulate into one row");
        let s = &scores[0];
        assert_eq!(s.n_calls, 3);
        assert_eq!(s.latency_ms_sum, 150);
        // average should be ≈ 0.9
        let avg = s.average();
        assert!((avg - 0.9).abs() < 1e-5, "expected ~0.9, got {avg}");
    }

    #[tokio::test]
    async fn for_persona_returns_empty_for_unknown() {
        let pool = fresh_pool().await;
        let store = ModelScoreStore::new(pool);
        let unknown = PersonaId::new();
        let scores = store.for_persona(unknown).await.expect("for_persona");
        assert!(scores.is_empty());
    }

    #[tokio::test]
    async fn for_persona_isolates_by_persona() {
        let pool = fresh_pool().await;
        let store = ModelScoreStore::new(pool);
        let p1 = PersonaId::new();
        let p2 = PersonaId::new();
        let model = fixture_model();

        store
            .record(p1, &model, &fixture_score(1.0, 1.0, 1.0), 10)
            .await
            .expect("record p1");

        let scores_p1 = store.for_persona(p1).await.expect("p1");
        let scores_p2 = store.for_persona(p2).await.expect("p2");

        assert_eq!(scores_p1.len(), 1);
        assert!(scores_p2.is_empty(), "p2 must not see p1 scores");
    }
}
