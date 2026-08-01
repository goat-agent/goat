use std::path::Path;

use crate::run::{GitError, output};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub head: Option<String>,
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub dirty: usize,
}

pub fn snapshot(cwd: &Path) -> Result<Snapshot, GitError> {
    let output = output(
        cwd,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "--branch",
        ],
    )?;
    Ok(parse(&output.stdout))
}

pub fn head_subject(cwd: &Path) -> Result<(String, String), GitError> {
    let output = output(
        cwd,
        &["--no-optional-locks", "log", "-1", "--format=%h%x00%s"],
    )?;
    let line = output.stdout.trim_end_matches(['\n', '\r']);
    let (short, subject) = line.split_once('\0').ok_or(GitError::NotARepository)?;
    Ok((short.to_owned(), subject.to_owned()))
}

pub fn parse(stdout: &str) -> Snapshot {
    let mut out = Snapshot::default();
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("# branch.oid ") {
            out.head = named(rest, "(initial)");
        } else if let Some(rest) = line.strip_prefix("# branch.head ") {
            out.branch = named(rest, "(detached)");
        } else if let Some(rest) = line.strip_prefix("# branch.upstream ") {
            out.upstream = named(rest, "");
        } else if let Some(rest) = line.strip_prefix("# branch.ab ") {
            let (ahead, behind) = parse_ab(rest);
            out.ahead = ahead;
            out.behind = behind;
        } else if is_entry(line) {
            out.dirty += 1;
        }
    }
    out
}

fn named(value: &str, placeholder: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value == placeholder {
        None
    } else {
        Some(value.to_owned())
    }
}

fn parse_ab(value: &str) -> (u32, u32) {
    let mut ahead = 0;
    let mut behind = 0;
    for field in value.split_whitespace() {
        if let Some(rest) = field.strip_prefix('+') {
            ahead = rest.parse().unwrap_or(0);
        } else if let Some(rest) = field.strip_prefix('-') {
            behind = rest.parse().unwrap_or(0);
        }
    }
    (ahead, behind)
}

fn is_entry(line: &str) -> bool {
    matches!(line.split(' ').next(), Some("1" | "2" | "u" | "?" | "!"))
}

#[cfg(test)]
mod tests {
    use super::{Snapshot, parse};

    #[test]
    fn parses_branch_without_upstream() {
        let got = parse(
            "# branch.oid 05808dd06e679c9f7727d04a9a73760f198be0b8\n# branch.head worktree-git-ui\n",
        );
        assert_eq!(
            got,
            Snapshot {
                head: Some("05808dd06e679c9f7727d04a9a73760f198be0b8".to_owned()),
                branch: Some("worktree-git-ui".to_owned()),
                upstream: None,
                ahead: 0,
                behind: 0,
                dirty: 0,
            }
        );
    }

    #[test]
    fn parses_upstream_and_ahead_behind() {
        let got = parse(
            "# branch.oid abc\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -1\n",
        );
        assert_eq!(got.upstream.as_deref(), Some("origin/main"));
        assert_eq!(got.ahead, 2);
        assert_eq!(got.behind, 1);
    }

    #[test]
    fn initial_and_detached_are_absent() {
        let got = parse("# branch.oid (initial)\n# branch.head (detached)\n");
        assert_eq!(got.head, None);
        assert_eq!(got.branch, None);
    }

    #[test]
    fn counts_entry_lines_only() {
        let got = parse(concat!(
            "# branch.oid abc\n",
            "# branch.head main\n",
            "1 .M N... 100644 100644 100644 aaa bbb src/lib.rs\n",
            "2 R. N... 100644 100644 100644 ccc ddd R100 new.rs\told.rs\n",
            "u UU N... 100644 100644 100644 100644 eee fff ggg conflict.rs\n",
            "? untracked.rs\n",
        ));
        assert_eq!(got.dirty, 4);
    }
}
