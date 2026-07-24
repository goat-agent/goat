use std::{
    ffi::OsString, fmt::Write as _, path::PathBuf, process::Stdio, sync::OnceLock, time::Duration,
};

use goat_protocol::ToolDisplay;
use goat_tool::{SandboxPolicy, Tool, ToolContext, ToolError, ToolFuture, ToolOutput, display};
use serde::Deserialize;
use tokio::{io::AsyncReadExt, process::Command, time};

const MIN_TIMEOUT_MS: u64 = 100;
const MAX_TIMEOUT_MS: u64 = 300_000;

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
        #[cfg(unix)]
        if let Some(pid) = self.child.id() {
            let _ = std::process::Command::new("kill")
                .arg("-KILL")
                .arg(format!("-{pid}"))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        #[cfg(windows)]
        if let Some(pid) = self.child.id() {
            let _ = std::process::Command::new("taskkill")
                .args(["/T", "/F", "/PID"])
                .arg(pid.to_string())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.child.start_kill();
    }
}

pub struct BashTool;

#[derive(Deserialize)]
struct Input {
    command: String,
    timeout_ms: Option<u64>,
}

impl Tool for BashTool {
    fn name(&self) -> &'static str {
        "Bash"
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
                None => ctx.bash_timeout,
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
                            return Err(ToolError::Execution {
                                message: "no sandbox backend is available, so shell commands are disabled while planning".to_owned(),
                            });
                        }
                    }
                }
            };

            let mut builder = Command::new(&program);
            builder
                .args(&prog_args)
                .current_dir(&ctx.cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            #[cfg(unix)]
            builder.process_group(0);
            if let Some(tmp) = &tmpdir {
                builder.env("TMPDIR", tmp);
            }
            let child = builder
                .spawn()
                .map_err(|source| ToolError::Spawn { source })?;

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
                return Err(ToolError::Timeout {
                    ms: u64::try_from(timeout_dur.as_millis()).unwrap_or(MAX_TIMEOUT_MS),
                });
            };

            let code = status.ok().and_then(|s| s.code());
            Ok(build_output(
                &stdout,
                &stderr,
                code,
                ctx.max_output_bytes,
                read_only,
            ))
        })
    }
}

const SUMMARY_LINE_THRESHOLD: usize = 5;

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
    use super::BashTool;
    use goat_tool::{Tool, ToolContext, ToolError};

    fn ctx() -> ToolContext {
        ToolContext::new(&std::env::temp_dir()).unwrap()
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
        assert!(matches!(result, Err(ToolError::Timeout { .. })));
    }

    #[tokio::test]
    async fn high_volume_output_is_capped_not_hung() {
        let mut c = ctx();
        c.max_output_bytes = 4096;
        let out = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            BashTool.run(
                r#"{"command":"head -c 50000000 /dev/zero | tr '\\0' 'a'"}"#,
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
