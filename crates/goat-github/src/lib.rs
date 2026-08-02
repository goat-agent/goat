use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Stdio};

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrInfo {
    pub number: u64,
    pub state: PrState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
    Open,
    Merged,
    Closed,
}

#[derive(Deserialize)]
struct PrView {
    number: u64,
    state: String,
}

pub fn gh_available() -> bool {
    std::env::var_os("PATH").is_some_and(|paths| gh_on_path(&paths))
}

fn gh_on_path(paths: &OsStr) -> bool {
    let candidates: &[&str] = if cfg!(windows) {
        &["gh.exe", "gh"]
    } else {
        &["gh"]
    };
    std::env::split_paths(paths).any(|dir| candidates.iter().any(|name| dir.join(name).is_file()))
}

pub fn pr_for_branch(repo_root: &Path, branch: &str) -> Option<PrInfo> {
    let output = Command::new("gh")
        .args(["pr", "view", branch, "--json", "number,state"])
        .current_dir(repo_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_pr(&output.stdout)
}

fn parse_pr(stdout: &[u8]) -> Option<PrInfo> {
    let view: PrView = serde_json::from_slice(stdout).ok()?;
    let state = match view.state.as_str() {
        "OPEN" => PrState::Open,
        "MERGED" => PrState::Merged,
        "CLOSED" => PrState::Closed,
        _ => return None,
    };
    Some(PrInfo {
        number: view.number,
        state,
    })
}

#[cfg(test)]
mod tests {
    use super::{PrState, gh_on_path, parse_pr};

    #[test]
    fn parse_open_merged_closed() {
        assert_eq!(
            parse_pr(br#"{"number":124,"state":"OPEN"}"#).map(|p| (p.number, p.state)),
            Some((124, PrState::Open))
        );
        assert_eq!(
            parse_pr(br#"{"number":7,"state":"MERGED"}"#).map(|p| p.state),
            Some(PrState::Merged)
        );
        assert_eq!(
            parse_pr(br#"{"number":7,"state":"CLOSED"}"#).map(|p| p.state),
            Some(PrState::Closed)
        );
    }

    #[test]
    fn parse_rejects_unknown_state_and_garbage() {
        assert_eq!(parse_pr(br#"{"number":1,"state":"WAT"}"#), None);
        assert_eq!(parse_pr(b"not json"), None);
        assert_eq!(parse_pr(b""), None);
    }

    #[test]
    fn parse_ignores_extra_fields() {
        assert_eq!(
            parse_pr(br#"{"number":9,"state":"OPEN","title":"x","url":"y"}"#).map(|p| p.number),
            Some(9)
        );
    }

    #[test]
    fn gh_on_path_detects_binary() {
        let dir = tempfile::tempdir().unwrap();
        let name = if cfg!(windows) { "gh.exe" } else { "gh" };
        std::fs::write(dir.path().join(name), b"").unwrap();
        let joined = std::env::join_paths([dir.path()]).unwrap();
        assert!(gh_on_path(&joined));

        let empty = tempfile::tempdir().unwrap();
        let joined_empty = std::env::join_paths([empty.path()]).unwrap();
        assert!(!gh_on_path(&joined_empty));
    }
}

pub mod cli {
    use std::process::Stdio;
    use std::time::Duration;

    use serde_json::Value;
    use tokio::process::Command;

    const CALL_TIMEOUT: Duration = Duration::from_secs(30);
    const MAX_PER_PAGE: usize = 100;

    const AUTH_MARKERS: &[&str] = &[
        "not logged in",
        "gh auth login",
        "requires authentication",
        "authentication required",
        "bad credentials",
        "http 401",
        "http 403",
    ];

    #[derive(Debug, thiserror::Error)]
    #[non_exhaustive]
    pub enum GhError {
        #[error("gh needs to be signed in: {0}")]
        Auth(String),
        #[error("gh failed: {0}")]
        Failed(String),
    }

    pub type GhResult<T> = Result<T, GhError>;

    pub async fn login() -> GhResult<String> {
        let handle = run(&["api", "user", "--jq", ".login"]).await?;
        let handle = handle.trim().to_owned();
        if handle.is_empty() {
            return Err(GhError::Failed(
                "gh returned no login; run `gh auth login`".to_owned(),
            ));
        }
        Ok(handle)
    }

    pub async fn search_issues(query: &str, limit: usize) -> GhResult<Value> {
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
            .map_err(|e| GhError::Failed(format!("gh returned unreadable json: {e}")))
    }

    async fn run(args: &[&str]) -> GhResult<String> {
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
                GhError::Failed(format!("gh timed out after {}s", CALL_TIMEOUT.as_secs()))
            })?
            .map_err(|e| GhError::Failed(format!("gh failed to start: {e}")))?;
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
        }
        Err(classify(&String::from_utf8_lossy(&output.stderr)))
    }

    pub fn classify(stderr: &str) -> GhError {
        let lowered = stderr.to_ascii_lowercase();
        let detail = stderr.trim();
        let detail = if detail.is_empty() {
            "no detail".to_owned()
        } else {
            detail.to_owned()
        };
        if AUTH_MARKERS.iter().any(|marker| lowered.contains(marker)) {
            GhError::Auth(detail)
        } else {
            GhError::Failed(detail)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn an_authentication_message_is_classified_as_auth() {
            for stderr in [
                "gh: Not logged in",
                "run gh auth login",
                "HTTP 401: Bad credentials",
                "HTTP 403: requires authentication",
            ] {
                assert!(matches!(classify(stderr), GhError::Auth(_)), "{stderr}");
            }
        }

        #[test]
        fn anything_else_is_a_plain_failure() {
            assert!(matches!(classify("boom"), GhError::Failed(_)));
        }

        #[test]
        fn an_empty_stderr_still_carries_a_message() {
            assert!(matches!(classify("   "), GhError::Failed(m) if m == "no detail"));
        }
    }
}
