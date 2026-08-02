use std::time::Duration;

use goat_channel::{ChannelError, ChannelResult};
use serde::Deserialize;
use serde_json::{Value, json};

const BASE: &str = "https://slack.com/api";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Identity {
    pub(crate) user_id: String,
    pub(crate) user: String,
    pub(crate) team: Option<String>,
    pub(crate) bot_id: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct Posted {
    pub(crate) channel: String,
    pub(crate) ts: String,
}

pub(crate) struct SlackApi {
    http: reqwest::Client,
    bot_token: String,
}

impl SlackApi {
    pub(crate) fn new(bot_token: impl Into<String>) -> ChannelResult<Self> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| ChannelError::Provider(format!("slack: http client: {e}")))?;
        Ok(Self {
            http,
            bot_token: bot_token.into(),
        })
    }

    pub(crate) async fn auth_test(&self) -> ChannelResult<Identity> {
        let value = self.call("auth.test", &self.bot_token, None).await?;
        serde_json::from_value(value)
            .map_err(|e| ChannelError::Provider(format!("slack: auth.test shape: {e}")))
    }

    pub(crate) async fn post_message(
        &self,
        channel: &str,
        text: &str,
        thread_ts: Option<&str>,
    ) -> ChannelResult<Posted> {
        let mut body = json!({ "channel": channel, "text": text });
        if let Some(ts) = thread_ts {
            body["thread_ts"] = json!(ts);
        }
        let value = self
            .call("chat.postMessage", &self.bot_token, Some(body))
            .await?;
        let ts = string_field(&value, "ts").ok_or_else(|| {
            ChannelError::Provider("slack: chat.postMessage returned no ts".to_string())
        })?;
        Ok(Posted {
            channel: string_field(&value, "channel").unwrap_or_else(|| channel.to_string()),
            ts,
        })
    }

    pub(crate) async fn update_message(
        &self,
        channel: &str,
        ts: &str,
        text: &str,
    ) -> ChannelResult<()> {
        self.call(
            "chat.update",
            &self.bot_token,
            Some(json!({ "channel": channel, "ts": ts, "text": text })),
        )
        .await
        .map(|_| ())
    }

    pub(crate) async fn user_profile(&self, user_id: &str) -> ChannelResult<UserProfile> {
        let value = self
            .call(
                "users.info",
                &self.bot_token,
                Some(json!({ "user": user_id })),
            )
            .await?;
        let user = value.get("user").cloned().unwrap_or(Value::Null);
        Ok(UserProfile {
            display: string_field(&user, "real_name")
                .or_else(|| string_field(&user, "name"))
                .unwrap_or_else(|| user_id.to_string()),
            avatar: user
                .get("profile")
                .and_then(|agent| string_field(agent, "image_192")),
        })
    }

    async fn call(&self, method: &str, token: &str, body: Option<Value>) -> ChannelResult<Value> {
        let mut request = self
            .http
            .post(format!("{BASE}/{method}"))
            .bearer_auth(token);
        if let Some(body) = body {
            request = request.json(&body);
        } else {
            request = request.header(reqwest::header::CONTENT_LENGTH, "0");
        }
        let response = request
            .send()
            .await
            .map_err(|e| ChannelError::Provider(format!("slack: {method}: {e}")))?;
        let status = response.status();
        let value: Value = response
            .json()
            .await
            .map_err(|e| ChannelError::Provider(format!("slack: {method} ({status}): {e}")))?;
        check_ok(method, value)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct UserProfile {
    pub(crate) display: String,
    pub(crate) avatar: Option<String>,
}

pub(crate) async fn open_connection(app_token: &str) -> ChannelResult<String> {
    let http = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| ChannelError::Provider(format!("slack: http client: {e}")))?;
    let response = http
        .post(format!("{BASE}/apps.connections.open"))
        .bearer_auth(app_token)
        .header(reqwest::header::CONTENT_LENGTH, "0")
        .send()
        .await
        .map_err(|e| ChannelError::Provider(format!("slack: apps.connections.open: {e}")))?;
    let value: Value = response
        .json()
        .await
        .map_err(|e| ChannelError::Provider(format!("slack: apps.connections.open shape: {e}")))?;
    let value = check_ok("apps.connections.open", value)?;
    string_field(&value, "url").ok_or_else(|| {
        ChannelError::Provider("slack: apps.connections.open returned no url".to_string())
    })
}

fn check_ok(method: &str, value: Value) -> ChannelResult<Value> {
    if value.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(value);
    }
    let code = string_field(&value, "error").unwrap_or_else(|| "unknown_error".to_string());
    Err(classify(method, &code))
}

fn classify(method: &str, code: &str) -> ChannelError {
    let message = format!("slack: {method}: {code}");
    match code {
        "invalid_auth"
        | "not_authed"
        | "account_inactive"
        | "token_revoked"
        | "token_expired"
        | "no_permission"
        | "missing_scope"
        | "not_allowed_token_type" => ChannelError::Auth(message),
        "ratelimited"
        | "rate_limited"
        | "service_unavailable"
        | "fatal_error"
        | "internal_error"
        | "request_timeout" => ChannelError::Provider(message),
        _ => ChannelError::BadRequest(message),
    }
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ok_payload_passes_through() {
        let value = json!({ "ok": true, "ts": "1.1" });
        assert_eq!(check_ok("chat.postMessage", value.clone()).unwrap(), value);
    }

    #[test]
    fn a_missing_ok_flag_is_a_failure() {
        assert!(check_ok("auth.test", json!({})).is_err());
        assert!(check_ok("auth.test", json!({ "ok": false })).is_err());
    }

    #[test]
    fn credential_failures_map_to_auth() {
        for code in [
            "invalid_auth",
            "not_authed",
            "account_inactive",
            "token_revoked",
            "token_expired",
            "missing_scope",
            "not_allowed_token_type",
        ] {
            assert!(
                matches!(classify("auth.test", code), ChannelError::Auth(_)),
                "{code} should be an auth error"
            );
        }
    }

    #[test]
    fn transient_failures_map_to_provider() {
        for code in ["ratelimited", "service_unavailable", "internal_error"] {
            assert!(
                matches!(classify("chat.update", code), ChannelError::Provider(_)),
                "{code} should be a provider error"
            );
        }
    }

    #[test]
    fn everything_else_maps_to_bad_request() {
        for code in ["channel_not_found", "msg_too_long", "unknown_error"] {
            assert!(
                matches!(
                    classify("chat.postMessage", code),
                    ChannelError::BadRequest(_)
                ),
                "{code} should be a bad request"
            );
        }
    }

    #[test]
    fn the_error_message_names_the_method_and_the_code() {
        let ChannelError::Auth(message) = classify("auth.test", "invalid_auth") else {
            panic!("expected an auth error");
        };
        assert!(message.contains("auth.test"));
        assert!(message.contains("invalid_auth"));
    }

    #[test]
    fn identity_parses_a_real_auth_test_payload() {
        let value = json!({
            "ok": true,
            "url": "https://team.slack.com/",
            "team": "Team",
            "user": "goat",
            "team_id": "T123",
            "user_id": "U123",
            "bot_id": "B123"
        });
        let identity: Identity = serde_json::from_value(value).unwrap();
        assert_eq!(identity.user_id, "U123");
        assert_eq!(identity.user, "goat");
        assert_eq!(identity.team.as_deref(), Some("Team"));
        assert_eq!(identity.bot_id.as_deref(), Some("B123"));
    }

    #[test]
    fn string_field_treats_blank_as_absent() {
        let value = json!({ "a": "", "b": "x", "c": 1 });
        assert_eq!(string_field(&value, "a"), None);
        assert_eq!(string_field(&value, "b"), Some("x".to_string()));
        assert_eq!(string_field(&value, "c"), None);
        assert_eq!(string_field(&value, "missing"), None);
    }
}
