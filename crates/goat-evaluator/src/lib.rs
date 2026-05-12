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
