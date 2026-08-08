use std::{
    ffi::OsString,
    fmt::Write as _,
    path::{Path, PathBuf},
    process::Stdio,
    sync::OnceLock,
    time::Duration,
};

use goat_protocol::{GitFacts, ToolDisplay, ToolOutcome};
use goat_tool::{
    SandboxPolicy, Tool, ToolContext, ToolError, ToolErrorClass, ToolFuture, ToolOutcomeExtension,
    ToolOutput, display,
};
use serde::Deserialize;
use tokio::{io::AsyncReadExt, process::Command, time};

const MIN_TIMEOUT_MS: u64 = 100;
const MAX_TIMEOUT_MS: u64 = 300_000;
const DEFAULT_TIMEOUT: Duration = Duration::from_mins(2);

fn sandbox_tmp() -> &'static PathBuf {
    static TMP: OnceLock<PathBuf> = OnceLock::new();
    TMP.get_or_init(|| {
        let raw = std::env::temp_dir();
        raw.canonicalize().unwrap_or(raw)
    })
}

struct ChildGuard {
    child: tokio::process::Child,
    reaped: bool,
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        if let Some(pgid) = self.child.id().and_then(|pid| i32::try_from(pid).ok())
            && let Err(err) = goat_process::kill_group(pgid)
        {
            tracing::warn!(%err, pgid, "failed to kill process group");
        }
        let _ = self.child.start_kill();
    }
}

pub const NAME: &str = "Bash";

pub struct BashTool;

#[derive(Deserialize)]
struct Input {
    command: String,
    timeout_ms: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
enum ShellError {
    #[error("no sandbox backend is available, so shell commands are disabled while planning")]
    SandboxUnavailable,
    #[error("failed to spawn command: {0}")]
    Spawn(std::io::Error),
    #[error("command timed out after {0}ms")]
    Timeout(u64),
}

impl From<ShellError> for ToolError {
    fn from(error: ShellError) -> Self {
        let class = match error {
            ShellError::SandboxUnavailable => ToolErrorClass::Policy,
            ShellError::Spawn(_) => ToolErrorClass::Io,
            ShellError::Timeout(_) => ToolErrorClass::Timeout,
        };
        ToolError::new(class, error.to_string())
    }
}

struct GitOutcome(GitFacts);

impl ToolOutcomeExtension for GitOutcome {
    fn apply(&self, outcome: &mut ToolOutcome) {
        outcome.git = Some(Box::new(self.0.clone()));
    }
}

impl Tool for BashTool {
    fn name(&self) -> &'static str {
        NAME
    }

    fn description(&self) -> &'static str {
        "Run a shell command via `sh -c` in the session directory and return its combined output. A nonzero exit code is reported in the output, not as an error."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {"type": "string"},
                "timeout_ms": {"type": "integer"}
            },
            "required": ["command"]
        })
    }

    fn display_input(&self, input: &str) -> ToolDisplay {
        match serde_json::from_str::<Input>(input) {
            Ok(args) => ToolDisplay::primary(display::call_sig(
                "Bash",
                &[display::flatten(&args.command).as_str()],
            )),
            Err(_) => display::generic(input),
        }
    }

    fn run<'a>(&'a self, input: &'a str, ctx: &'a ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let args: Input = serde_json::from_str(input)?;
            let timeout_dur = match args.timeout_ms {
                Some(ms) => Duration::from_millis(ms.clamp(MIN_TIMEOUT_MS, MAX_TIMEOUT_MS)),
                None => DEFAULT_TIMEOUT,
            };

            let read_only = ctx.exec_policy.is_read_only();
            let (program, prog_args, tmpdir) = match &ctx.exec_policy {
                SandboxPolicy::Full => (
                    OsString::from("sh"),
                    vec![OsString::from("-c"), OsString::from(&args.command)],
                    None,
                ),
                SandboxPolicy::ReadOnly { network } => {
                    let tmp = sandbox_tmp();
                    match goat_sandbox::read_only_command(&args.command, &ctx.cwd, tmp, *network) {
                        Ok(sc) => (sc.program, sc.args, Some(tmp.clone())),
                        Err(_) => {
                            return Err(ShellError::SandboxUnavailable.into());
                        }
                    }
                }
            };

            let mut builder = Command::new(&program);
            builder
                .args(&prog_args)
                .env_clear()
                .envs(goat_process::child_environment())
                .current_dir(&ctx.cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            #[cfg(unix)]
            builder.process_group(0);
            if let Some(tmp) = &tmpdir {
                builder.env("TMPDIR", tmp);
            }
            let child = builder.spawn().map_err(ShellError::Spawn)?;

            let mut guard = ChildGuard {
                child,
                reaped: false,
            };
            let mut stdout_pipe = guard.child.stdout.take();
            let mut stderr_pipe = guard.child.stderr.take();
            let read_cap = ctx.max_output_bytes.saturating_mul(4).max(1);

            let result = time::timeout(timeout_dur, async {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                let (stdout_result, stderr_result) = tokio::join!(
                    async {
                        if let Some(pipe) = stdout_pipe.as_mut() {
                            read_capped(pipe, &mut stdout, read_cap).await
                        } else {
                            Ok(())
                        }
                    },
                    async {
                        if let Some(pipe) = stderr_pipe.as_mut() {
                            read_capped(pipe, &mut stderr, read_cap).await
                        } else {
                            Ok(())
                        }
                    }
                );
                if let Err(err) = stdout_result {
                    tracing::debug!(error = %err, "shell stdout read error; output may be truncated");
                }
                if let Err(err) = stderr_result {
                    tracing::debug!(error = %err, "shell stderr read error; output may be truncated");
                }
                let status = guard.child.wait().await;
                guard.reaped = true;
                (stdout, stderr, status)
            })
            .await;

            let Ok((stdout, stderr, status)) = result else {
                return Err(ShellError::Timeout(
                    u64::try_from(timeout_dur.as_millis()).unwrap_or(MAX_TIMEOUT_MS),
                )
                .into());
            };

            let code = status.ok().and_then(|s| s.code());
            let output = build_output(&stdout, &stderr, code, ctx.max_output_bytes, read_only);
            let Some(facts) = git_facts(&args.command, &ctx.cwd, &stdout, code).await else {
                return Ok(output);
            };
            Ok(output.with_extension(GitOutcome(facts)))
        })
    }
}

const SUMMARY_LINE_THRESHOLD: usize = 5;

async fn git_facts(
    command: &str,
    cwd: &Path,
    stdout: &[u8],
    code: Option<i32>,
) -> Option<GitFacts> {
    if code != Some(0) {
        return None;
    }
    let ops = goat_git::classify(command)?;
    let cwd = cwd.to_path_buf();
    let wants_head = ops.iter().any(|op| op.verb.moves_head());
    let observed = tokio::task::spawn_blocking(move || observe(&cwd, wants_head))
        .await
        .ok()??;
    let mut facts = observed;
    if ops.iter().any(|op| op.verb == goat_git::GitVerb::PrCreate)
        && let Some(url) = pull_request_url(&String::from_utf8_lossy(stdout))
    {
        facts.pr = pull_request_number(&url);
        facts.pr_url = Some(url);
    }
    Some(facts)
}

fn observe(cwd: &Path, wants_head: bool) -> Option<GitFacts> {
    let snapshot = goat_git::snapshot(cwd).ok()?;
    let mut facts = GitFacts {
        branch: snapshot.branch,
        upstream: snapshot.upstream,
        ..GitFacts::default()
    };
    if wants_head && let Ok((head, subject)) = goat_git::head_subject(cwd) {
        facts.head = Some(head);
        facts.subject = Some(subject);
    }
    Some(facts)
}

fn pull_request_url(stdout: &str) -> Option<String> {
    stdout
        .split_whitespace()
        .find(|token| token.starts_with("https://") && token.contains("/pull/"))
        .map(str::to_owned)
}

fn pull_request_number(url: &str) -> Option<u64> {
    url.rsplit('/').next()?.parse().ok()
}

async fn read_capped<R>(reader: &mut R, buf: &mut Vec<u8>, cap: usize) -> std::io::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut chunk = vec![0u8; 8192];
    loop {
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        if buf.len() < cap {
            let take = (cap - buf.len()).min(n);
            buf.extend_from_slice(&chunk[..take]);
        }
    }
}

const DENIAL_MARKERS: [&str; 3] = [
    "Operation not permitted",
    "Read-only file system",
    "Permission denied",
];

fn build_output(
    stdout: &[u8],
    stderr: &[u8],
    code: Option<i32>,
    max_bytes: usize,
    read_only: bool,
) -> ToolOutput {
    let mut out = String::new();
    out.push_str(&String::from_utf8_lossy(stdout));
    if !stderr.is_empty() {
        out.push_str("\n--- stderr ---\n");
        out.push_str(&String::from_utf8_lossy(stderr));
    }
    let mut out = goat_tool::truncate(out, max_bytes);
    let summary = build_summary(&out, code);
    let denied = read_only
        && matches!(code, Some(c) if c != 0)
        && DENIAL_MARKERS.iter().any(|marker| out.contains(marker));
    if let Some(c) = code
        && c != 0
    {
        let _ = write!(out, "\nexit code: {c}");
    }
    if denied {
        out.push_str(
            "\n[note] this command ran under a read-only sandbox; the permission errors above may be from writes blocked outside the scratch space",
        );
    }
    let output = ToolOutput::text(out);
    match summary {
        Some(summary) => output.with_summary(summary),
        None => output,
    }
}

fn build_summary(body: &str, code: Option<i32>) -> Option<String> {
    let nonzero = !matches!(code, Some(0) | None);
    if nonzero {
        let status = match code {
            Some(c) => format!("exit {c}"),
            None => "killed".to_owned(),
        };
        return Some(match body.lines().rev().find(|l| !l.trim().is_empty()) {
            Some(last) => format!("{status} · {}", display::flatten(last)),
            None => status,
        });
    }
    if body.lines().count() > SUMMARY_LINE_THRESHOLD {
        return None;
    }
    body.lines()
        .find(|l| !l.trim().is_empty())
        .map(display::flatten)
}

#[cfg(test)]
mod tests {
    use super::{BashTool, pull_request_number, pull_request_url};
    use goat_protocol::{GitFacts, ToolOutcome};
    use goat_tool::{Tool, ToolContext, ToolErrorClass, ToolOutput};
    use std::{path::Path, process::Command};

    fn ctx() -> ToolContext {
        ToolContext::new(&std::env::temp_dir()).unwrap()
    }

    fn output_git(output: &ToolOutput) -> Option<Box<GitFacts>> {
        let mut outcome = ToolOutcome {
            ok: true,
            summary: None,
            body: None,
            image: None,
            git: None,
        };
        output.extend_outcome(&mut outcome);
        outcome.git
    }

    fn git(repo: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .is_ok_and(|out| out.status.success());
        assert!(ok, "git {args:?} failed");
    }

    fn repo() -> Option<(tempfile::TempDir, ToolContext)> {
        let ready = Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|out| out.status.success());
        if !ready {
            return None;
        }
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-b", "main"]);
        git(dir.path(), &["config", "user.email", "t@example.invalid"]);
        git(dir.path(), &["config", "user.name", "Test"]);
        let ctx = ToolContext::new(dir.path()).unwrap();
        Some((dir, ctx))
    }

    #[tokio::test]
    async fn a_commit_chain_reports_the_sha_and_subject_it_produced() {
        let Some((dir, ctx)) = repo() else {
            return;
        };
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        let out = BashTool
            .run(
                r#"{"command":"git add -A && git commit -m \"feat: first\""}"#,
                &ctx,
            )
            .await
            .unwrap();
        let facts = output_git(&out).expect("git facts");
        assert_eq!(facts.subject.as_deref(), Some("feat: first"));
        assert_eq!(facts.branch.as_deref(), Some("main"));
        assert!(facts.head.is_some_and(|head| !head.is_empty()));
        assert_eq!(facts.upstream, None);
    }

    #[tokio::test]
    async fn a_failed_git_command_reports_no_facts() {
        let Some((_dir, ctx)) = repo() else {
            return;
        };
        let out = BashTool
            .run(r#"{"command":"git commit -m \"nothing staged\""}"#, &ctx)
            .await
            .unwrap();
        assert!(output_git(&out).is_none());
    }

    #[tokio::test]
    async fn a_non_git_command_reports_no_facts() {
        let Some((_dir, ctx)) = repo() else {
            return;
        };
        let out = BashTool
            .run(r#"{"command":"echo hello"}"#, &ctx)
            .await
            .unwrap();
        assert!(output_git(&out).is_none());
        assert_eq!(out.summary.as_deref(), Some("hello"));
    }

    #[tokio::test]
    async fn a_branch_switch_reports_the_branch_it_landed_on() {
        let Some((dir, ctx)) = repo() else {
            return;
        };
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-m", "init"]);
        let out = BashTool
            .run(r#"{"command":"git switch -c feat/git-ui"}"#, &ctx)
            .await
            .unwrap();
        let facts = output_git(&out).expect("git facts");
        assert_eq!(facts.branch.as_deref(), Some("feat/git-ui"));
        assert_eq!(facts.head, None);
    }

    #[tokio::test]
    async fn a_push_reports_the_upstream_it_now_tracks() {
        let Some((dir, ctx)) = repo() else {
            return;
        };
        let remote = dir.path().join("remote.git");
        assert!(
            Command::new("git")
                .args(["init", "--bare", remote.to_str().unwrap()])
                .output()
                .is_ok_and(|out| out.status.success())
        );
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-m", "init"]);
        git(
            dir.path(),
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        let out = BashTool
            .run(r#"{"command":"git push -u origin main"}"#, &ctx)
            .await
            .unwrap();
        let facts = output_git(&out).expect("git facts");
        assert_eq!(facts.upstream.as_deref(), Some("origin/main"));
        assert_eq!(facts.branch.as_deref(), Some("main"));
    }

    #[test]
    fn a_pull_request_url_yields_its_number() {
        let stdout = "https://github.com/goat-agent/goat/pull/59\n";
        let url = pull_request_url(stdout).unwrap();
        assert_eq!(pull_request_number(&url), Some(59));
        assert_eq!(pull_request_url("nothing here"), None);
    }

    #[tokio::test]
    async fn echoes_stdout() {
        let out = BashTool
            .run(r#"{"command":"echo hello"}"#, &ctx())
            .await
            .unwrap();
        assert!(out.as_text().unwrap().contains("hello"));
    }

    #[tokio::test]
    async fn resolves_binaries_with_the_filtered_environment() {
        let out = BashTool
            .run(
                r#"{"command":"command -v sh >/dev/null && sh -c 'printf resolved'"}"#,
                &ctx(),
            )
            .await
            .unwrap();
        assert_eq!(out.as_text(), Some("resolved"));
    }

    #[tokio::test]
    async fn nonzero_exit_is_ok() {
        let out = BashTool
            .run(r#"{"command":"exit 1"}"#, &ctx())
            .await
            .unwrap();
        assert!(out.as_text().unwrap().contains("exit code: 1"));
        assert_eq!(out.summary.as_deref(), Some("exit 1"));
    }

    #[tokio::test]
    async fn failure_summary_carries_last_line() {
        let out = BashTool
            .run(r#"{"command":"echo first; echo boom; exit 3"}"#, &ctx())
            .await
            .unwrap();
        assert_eq!(out.summary.as_deref(), Some("exit 3 · boom"));
    }

    #[tokio::test]
    async fn short_success_summarizes_first_line() {
        let out = BashTool
            .run(r#"{"command":"echo hello"}"#, &ctx())
            .await
            .unwrap();
        assert_eq!(out.summary.as_deref(), Some("hello"));
    }

    #[tokio::test]
    async fn silent_success_has_no_summary() {
        let out = BashTool.run(r#"{"command":"true"}"#, &ctx()).await.unwrap();
        assert_eq!(out.summary, None);
    }

    #[tokio::test]
    async fn long_success_has_no_summary() {
        let out = BashTool
            .run(r#"{"command":"seq 1 20"}"#, &ctx())
            .await
            .unwrap();
        assert_eq!(out.summary, None);
    }

    #[tokio::test]
    async fn timeout_errors() {
        let result = BashTool
            .run(r#"{"command":"sleep 999","timeout_ms":100}"#, &ctx())
            .await;
        assert!(matches!(
            result,
            Err(error) if error.class() == ToolErrorClass::Timeout
        ));
    }

    #[tokio::test]
    async fn high_volume_output_is_capped_not_hung() {
        let mut c = ctx();
        c.max_output_bytes = 4096;
        let out = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            BashTool.run(
                r#"{"command":"head -c 5000000 /dev/zero | tr '\\0' 'a'"}"#,
                &c,
            ),
        )
        .await
        .expect("command should finish without hanging")
        .unwrap();
        let text = out.as_text().unwrap();
        assert!(text.len() < 4096 * 8, "output should be truncated");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn read_only_allows_reads() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        let mut ctx = ToolContext::new(dir.path()).unwrap();
        ctx.exec_policy = goat_tool::SandboxPolicy::ReadOnly { network: false };
        let out = BashTool
            .run(r#"{"command":"cat a.txt"}"#, &ctx)
            .await
            .unwrap();
        assert!(out.as_text().unwrap().contains("hello"));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn read_only_blocks_writes_outside_scratch() {
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap();
        let dir = home.join(format!(".goat-sandbox-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut ctx = ToolContext::new(&dir).unwrap();
        ctx.exec_policy = goat_tool::SandboxPolicy::ReadOnly { network: false };
        let target = ctx.cwd.join("should-not-exist.txt");
        let command = format!("echo x > {}", target.display());
        let input = serde_json::json!({ "command": command }).to_string();
        let _ = BashTool.run(&input, &ctx).await.unwrap();
        let blocked = !target.exists();
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            blocked,
            "read-only sandbox must block writes outside scratch"
        );
    }
}
