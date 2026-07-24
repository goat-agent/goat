use std::sync::Arc;

use goat_auth::random_state;
use goat_provider::StreamError;
use serde_json::{Value, json};

use crate::error;
use tokio::sync::Mutex;

pub const CA_BASE: &str = "https://cloudcode-pa.googleapis.com/v1internal";

fn ca_url(method: &str) -> String {
    format!("{CA_BASE}:{method}")
}

fn metadata() -> Value {
    json!({
        "ideType": "IDE_UNSPECIFIED",
        "platform": "PLATFORM_UNSPECIFIED",
        "pluginType": "GEMINI",
    })
}

fn env_project() -> Option<String> {
    std::env::var("GOOGLE_CLOUD_PROJECT")
        .or_else(|_| std::env::var("GOOGLE_CLOUD_PROJECT_ID"))
        .ok()
        .filter(|s| !s.is_empty())
}

fn extract_project_str(v: &Value) -> Option<String> {
    let field = v.get("cloudaicompanionProject")?;
    if let Some(s) = field.as_str()
        && !s.is_empty()
    {
        return Some(s.to_owned());
    }
    if let Some(id) = field.get("id").and_then(Value::as_str)
        && !id.is_empty()
    {
        return Some(id.to_owned());
    }
    None
}

async fn load_code_assist(
    client: &reqwest::Client,
    access: &str,
) -> Result<(Option<String>, Option<String>), StreamError> {
    let body = if let Some(proj) = env_project() {
        json!({ "cloudaicompanionProject": proj, "metadata": metadata() })
    } else {
        json!({ "metadata": metadata() })
    };
    let resp = client
        .post(ca_url("loadCodeAssist"))
        .bearer_auth(access)
        .json(&body)
        .send()
        .await
        .map_err(|e| StreamError::transport(e.to_string()))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    tracing::debug!(%status, body = %text, "loadCodeAssist response");
    if !status.is_success() {
        return Err(error::classify_http(status, &text));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| StreamError::other(e.to_string()))?;
    let existing_project = extract_project_str(&v);
    let default_tier = v
        .get("allowedTiers")
        .and_then(Value::as_array)
        .and_then(|tiers| {
            tiers
                .iter()
                .find(|t| t.get("isDefault").and_then(Value::as_bool).unwrap_or(false))
                .cloned()
        });
    let tier_id = default_tier.as_ref().and_then(|t| {
        t.get("id")
            .or_else(|| t.get("tierId"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    let needs_user_project = default_tier
        .as_ref()
        .and_then(|t| t.get("userDefinedCloudaicompanionProject"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    tracing::debug!(
        ?existing_project,
        ?tier_id,
        needs_user_project,
        "loadCodeAssist parsed"
    );
    if needs_user_project && env_project().is_none() && existing_project.is_none() {
        return Err(StreamError::other(
            "Gemini Code Assist standard tier requires a GCP project. \
             Set the GOOGLE_CLOUD_PROJECT environment variable to your project ID.",
        ));
    }
    Ok((existing_project, tier_id))
}

async fn onboard_user(
    client: &reqwest::Client,
    access: &str,
    tier_id: &str,
) -> Result<String, StreamError> {
    let is_free = tier_id.to_uppercase() == "FREE";
    let body = if is_free {
        json!({ "tierId": tier_id, "metadata": metadata() })
    } else if let Some(proj) = env_project() {
        json!({ "tierId": tier_id, "cloudaicompanionProject": proj, "metadata": metadata() })
    } else {
        json!({ "tierId": tier_id, "metadata": metadata() })
    };
    tracing::debug!(tier = %tier_id, "onboardUser request");
    let resp = client
        .post(ca_url("onboardUser"))
        .bearer_auth(access)
        .json(&body)
        .send()
        .await
        .map_err(|e| StreamError::transport(e.to_string()))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    tracing::debug!(%status, body = %text, "onboardUser response");
    if !status.is_success() {
        return Err(error::classify_http(status, &text));
    }
    let lro: Value = serde_json::from_str(&text).map_err(|e| StreamError::other(e.to_string()))?;
    let op_name = lro
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| StreamError::other("onboardUser: missing operation name"))?
        .to_owned();
    tracing::debug!(op = %op_name, "polling LRO");

    let mut current = lro;
    for i in 0..60u32 {
        if current
            .get("done")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            break;
        }
        if i > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
        let poll = client
            .post(ca_url("getOperation"))
            .bearer_auth(access)
            .json(&json!({ "name": op_name }))
            .send()
            .await
            .map_err(|e| StreamError::transport(e.to_string()))?;
        let poll_text = poll.text().await.unwrap_or_default();
        tracing::debug!(body = %poll_text, "getOperation poll");
        current =
            serde_json::from_str(&poll_text).map_err(|e| StreamError::other(e.to_string()))?;
    }
    let project = current
        .get("response")
        .and_then(extract_project_str)
        .or_else(|| extract_project_str(&current));
    tracing::debug!(?project, "onboardUser resolved project");
    project.ok_or_else(|| StreamError::other("onboardUser: could not resolve project id"))
}

pub async fn resolve_project(
    client: &reqwest::Client,
    access: &str,
    cache: &Arc<Mutex<Option<String>>>,
) -> Result<Option<String>, StreamError> {
    {
        let guard = cache.lock().await;
        if let Some(p) = guard.as_ref() {
            tracing::debug!(project = %p, "project from cache");
            return Ok(Some(p.clone()));
        }
    }
    let result: Result<Option<String>, StreamError> = async {
        if let Some(env_proj) = env_project() {
            tracing::debug!(project = %env_proj, "project from env");
            return Ok(Some(env_proj));
        }
        let (existing, tier_id) = load_code_assist(client, access).await?;
        if let Some(p) = existing {
            tracing::debug!(project = %p, "project from loadCodeAssist");
            return Ok(Some(p));
        }
        let Some(tier) = tier_id else {
            return Ok(None);
        };
        tracing::debug!(%tier, "no existing project, onboarding");
        Ok(Some(onboard_user(client, access, &tier).await?))
    }
    .await;

    match &result {
        Ok(Some(p)) => {
            *cache.lock().await = Some(p.clone());
            tracing::debug!(project = %p, "project cached");
        }
        Ok(None) => tracing::warn!("no project resolved, sending without project"),
        Err(e) => tracing::warn!(error = %e, "project resolution error"),
    }
    result
}

pub fn wrap_request(model: &str, project: Option<&str>, inner: Value) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("model".to_owned(), Value::String(model.to_owned()));
    if let Some(p) = project {
        obj.insert("project".to_owned(), Value::String(p.to_owned()));
    }
    obj.insert("user_prompt_id".to_owned(), Value::String(random_state()));
    obj.insert("request".to_owned(), inner);
    tracing::debug!(model, ?project, "Code Assist wrap_request");
    Value::Object(obj)
}
