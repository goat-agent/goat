use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::credential::Credential;

pub(crate) fn probe(v: Value) -> goat_llm::ProbeFuture {
    Box::pin(async move {
        let cred: Credential =
            serde_json::from_value(v).map_err(|e| format!("invalid codex credential: {e}"))?;
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if cred.expires_at_ms < now_ms {
            return Err("access_token expired".into());
        }
        if cred.account_id.is_none() {
            return Err("chatgpt_account_id missing".into());
        }
        Ok(())
    })
}
