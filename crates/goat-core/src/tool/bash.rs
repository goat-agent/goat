use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use rustix::process::{Pid, Signal, kill_process_group};
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::time::Instant;

use super::spill::Capture;

const READ_BUFFER: usize = 8 * 1024;
const DRAIN_GRACE: Duration = Duration::from_secs(1);

#[derive(Debug, Serialize)]
pub(crate) struct Ran {
    pub(crate) code: Option<i32>,
    pub(crate) output: String,
    pub(crate) file: Option<PathBuf>,
}

pub(crate) async fn run(
    command: &str,
    cwd: &Path,
    timeout: Duration,
    spill_dir: &Path,
) -> std::io::Result<Ran> {
    let mut capture = Capture::create(spill_dir)?;
    let mut child = Command::new("bash")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()?;
    let group = Group::of(child.id());
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let mut out_buf = vec![0; READ_BUFFER];
    let mut err_buf = vec![0; READ_BUFFER];
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    let mut status: Option<ExitStatus> = None;
    let mut timed_out = false;

    while stdout.is_some() || stderr.is_some() || status.is_none() {
        tokio::select! {
            read = read_some(stdout.as_mut(), &mut out_buf), if stdout.is_some() => {
                match read? {
                    0 => stdout = None,
                    n => capture.push(&out_buf[..n])?,
                }
            }
            read = read_some(stderr.as_mut(), &mut err_buf), if stderr.is_some() => {
                match read? {
                    0 => stderr = None,
                    n => capture.push(&err_buf[..n])?,
                }
            }
            exited = child.wait(), if status.is_none() => {
                status = Some(exited?);
                group.kill();
                deadline.as_mut().reset(Instant::now() + DRAIN_GRACE);
            }
            () = &mut deadline => {
                if status.is_some() {
                    break;
                }
                timed_out = true;
                group.kill();
                deadline.as_mut().reset(Instant::now() + DRAIN_GRACE);
            }
        }
    }

    let kept = capture.finish()?;
    let mut output = kept.text;
    let code = status.and_then(|status| status.code());
    if timed_out {
        write!(
            output,
            "\n[timed out after {}s; the process group was killed]",
            timeout.as_secs()
        )
        .ok();
    } else if code.is_none() {
        write!(
            output,
            "\n[{}]",
            status.map_or_else(|| "killed".to_owned(), describe)
        )
        .ok();
    }
    Ok(Ran {
        code: code.filter(|_| !timed_out),
        output,
        file: kept.file,
    })
}

async fn read_some<R: AsyncRead + Unpin>(
    reader: Option<&mut R>,
    buf: &mut [u8],
) -> std::io::Result<usize> {
    match reader {
        Some(reader) => reader.read(buf).await,
        None => Ok(0),
    }
}

fn describe(status: ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;

    match (status.code(), status.signal()) {
        (Some(code), _) => format!("exit {code}"),
        (None, Some(signal)) => format!("killed by signal {signal}"),
        (None, None) => "exited".to_owned(),
    }
}

struct Group(Option<Pid>);

impl Group {
    fn of(pid: Option<u32>) -> Self {
        Self(
            pid.and_then(|pid| i32::try_from(pid).ok())
                .and_then(Pid::from_raw),
        )
    }

    fn kill(&self) {
        if let Some(pid) = self.0 {
            kill_process_group(pid, Signal::KILL).ok();
        }
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn bash(command: &str, timeout: Duration) -> (Ran, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let ran = run(command, dir.path(), timeout, &dir.path().join("spill"))
            .await
            .unwrap();
        (ran, dir)
    }

    #[tokio::test]
    async fn output_and_exit_code_are_reported() {
        let (ran, _dir) = bash("echo out; echo err >&2; exit 3", Duration::from_secs(5)).await;
        assert_eq!(ran.code, Some(3));
        assert_eq!(ran.output, "out\nerr\n");
    }

    #[tokio::test]
    async fn timeout_kills_the_process_group() {
        let (ran, _dir) = bash("sleep 30 & sleep 30", Duration::from_millis(300)).await;
        assert_eq!(ran.code, None);
        assert!(ran.output.contains("timed out"));
    }

    #[tokio::test]
    async fn background_children_do_not_hang_the_call() {
        let (ran, _dir) = bash("sleep 30 & echo started", Duration::from_secs(10)).await;
        assert_eq!(ran.output, "started\n");
        assert_eq!(ran.code, Some(0));
    }

    #[tokio::test]
    async fn detached_descendants_holding_the_pipe_do_not_hang_the_call() {
        let started = std::time::Instant::now();
        let (ran, _dir) = bash(
            r#"perl -MPOSIX -e 'POSIX::setsid(); exec "sleep", 30' & sleep 0.5; echo done"#,
            Duration::from_secs(10),
        )
        .await;
        assert!(ran.output.contains("done"));
        assert!(started.elapsed() >= DRAIN_GRACE);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn long_output_keeps_head_and_tail_and_spills_to_disk() {
        let (ran, dir) = bash("seq 1 100000", Duration::from_secs(10)).await;
        assert!(ran.output.starts_with("1\n2\n"));
        assert!(ran.output.contains("100000"));
        assert!(ran.output.contains("bytes omitted"));
        assert!(ran.file.is_some_and(|file| file.exists()));
        assert_eq!(
            std::fs::read_dir(dir.path().join("spill")).unwrap().count(),
            1
        );
    }

    #[tokio::test]
    async fn short_output_leaves_no_spill_file() {
        let (_, dir) = bash("echo hi", Duration::from_secs(5)).await;
        assert_eq!(
            std::fs::read_dir(dir.path().join("spill")).unwrap().count(),
            0
        );
    }
}
