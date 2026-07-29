use std::process::Stdio;
use std::time::Duration;

use goat_integration::{IntegrationError, IntegrationResult};
use serde_json::Value;
use tokio::process::Command;

const CALL_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_PER_PAGE: usize = 100;

pub async fn login() -> IntegrationResult<String> {
    let handle = run(&["api", "user", "--jq", ".login"]).await?;
    let handle = handle.trim().to_string();
    if handle.is_empty() {
        return Err(IntegrationError::Service(
            "gh returned no login; run `gh auth login`".into(),
        ));
    }
    Ok(handle)
}

pub async fn search(query: &str, limit: usize) -> IntegrationResult<Value> {
    let q = format!("q={query}");
    let per_page = format!("per_page={}", limit.clamp(1, MAX_PER_PAGE));
    let raw = run(&[
        "api",
        "-X",
        "GET",
        "search/issues",
        "-f",
        &q,
        "-f",
        &per_page,
    ])
    .await?;
    serde_json::from_str(&raw)
        .map_err(|e| IntegrationError::Service(format!("gh returned unreadable json: {e}")))
}

async fn run(args: &[&str]) -> IntegrationResult<String> {
    let spawned = Command::new("gh")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(CALL_TIMEOUT, spawned)
        .await
        .map_err(|_| {
            IntegrationError::Service(format!("gh timed out after {}s", CALL_TIMEOUT.as_secs()))
        })?
        .map_err(|e| IntegrationError::Service(format!("gh failed to start: {e}")))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    Err(classify(&String::from_utf8_lossy(&output.stderr)))
}

const AUTH_MARKERS: &[&str] = &[
    "not logged in",
    "gh auth login",
    "requires authentication",
    "authentication required",
    "bad credentials",
    "http 401",
    "http 403",
];

pub fn classify(stderr: &str) -> IntegrationError {
    let detail = stderr.trim();
    let lowered = detail.to_ascii_lowercase();
    let detail = if detail.is_empty() {
        "no error detail"
    } else {
        detail
    };
    if AUTH_MARKERS.iter().any(|marker| lowered.contains(marker)) {
        IntegrationError::Auth(format!("gh: {detail}"))
    } else {
        IntegrationError::Service(format!("gh: {detail}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logged_out_and_rejected_tokens_are_auth_failures() {
        for stderr in [
            "You are not logged into any GitHub hosts. To log in, run: gh auth login",
            "gh: Bad credentials (HTTP 401)",
            "gh: Resource not accessible by personal access token (HTTP 403)",
            "error: Requires authentication",
        ] {
            assert!(
                matches!(classify(stderr), IntegrationError::Auth(_)),
                "expected an auth failure for {stderr:?}",
            );
        }
    }

    #[test]
    fn everything_else_is_a_service_failure() {
        for stderr in [
            "gh: Validation Failed (HTTP 422)",
            "error connecting to api.github.com",
            "gh: Not Found (HTTP 404)",
        ] {
            assert!(
                matches!(classify(stderr), IntegrationError::Service(_)),
                "expected a service failure for {stderr:?}",
            );
        }
    }

    #[test]
    fn silent_failures_still_carry_a_message() {
        let IntegrationError::Service(message) = classify("   ") else {
            panic!("expected a service failure");
        };
        assert!(message.contains("no error detail"));
    }
}
