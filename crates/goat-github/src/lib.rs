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
