use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use goat_llm::{RefreshError, RefreshableCredential};
use serde::{Deserialize, Serialize};

use crate::auth;

/// 8 days — matches codex-rs `TOKEN_REFRESH_INTERVAL`.
const REFRESH_INTERVAL_MS: u64 = 8 * 86_400_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Credential {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub id_token: Option<String>,
    pub expires_at_ms: u64,
    pub last_refresh_ms: u64,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
}

#[async_trait]
impl RefreshableCredential for Credential {
    fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    fn needs_refresh(&self, now: SystemTime) -> bool {
        let now_ms = now
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        now_ms >= self.last_refresh_ms.saturating_add(REFRESH_INTERVAL_MS)
    }

    async fn refresh(&self) -> Result<Self, RefreshError> {
        auth::refresh_with(&self.refresh_token, self.label.clone()).await
    }
}
