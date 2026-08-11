use std::sync::Arc;

use goat_api::PtyItem;
use goat_wire::envelope::{CallError, ErrorCode, Execution};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::pty::{Chunker, Terminal};

const READ_BUFFER: usize = 16 * 1024;
const OUTPUT_QUEUE: usize = 256;

pub struct Spawned {
    pub output: mpsc::Receiver<PtyItem>,
    pub terminal: Arc<dyn Terminal>,
}

enum Command {
    Write(Vec<u8>),
    Resize { cols: u16, rows: u16 },
}

struct Live {
    commands: mpsc::UnboundedSender<Command>,
    stop: CancellationToken,
}

impl Terminal for Live {
    fn write(&self, data: &[u8]) -> Result<(), String> {
        self.commands
            .send(Command::Write(data.to_vec()))
            .map_err(|_| "terminal is closed".to_owned())
    }

    fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        self.commands
            .send(Command::Resize { cols, rows })
            .map_err(|_| "terminal is closed".to_owned())
    }

    fn close(&self) {
        self.stop.cancel();
    }
}

pub fn spawn(
    cwd: &str,
    cols: u16,
    rows: u16,
    command: Option<&str>,
    id: String,
) -> Result<Spawned, CallError> {
    let shell = command
        .filter(|value| !value.trim().is_empty())
        .map_or_else(
            || std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned()),
            ToOwned::to_owned,
        );

    let (pty, pts) = pty_process::open().map_err(|err| failed(format!("open pty: {err}")))?;
    pty.resize(pty_process::Size::new(rows, cols))
        .map_err(|err| failed(format!("resize pty: {err}")))?;

    let mut child = pty_process::Command::new("/bin/sh")
        .arg("-c")
        .arg(&shell)
        .current_dir(cwd)
        .env_clear()
        .envs(goat_process::child_environment())
        .spawn(pts)
        .map_err(|err| failed(format!("spawn: {err}")))?;

    let (mut reader, mut writer) = pty.into_split();
    let stop = CancellationToken::new();
    let (tx, output) = mpsc::channel(OUTPUT_QUEUE);
    let (commands, mut command_rx) = mpsc::unbounded_channel::<Command>();

    let terminal: Arc<dyn Terminal> = Arc::new(Live {
        commands,
        stop: stop.clone(),
    });

    let writer_stop = stop.clone();
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt as _;
        loop {
            let command = tokio::select! {
                biased;
                () = writer_stop.cancelled() => break,
                command = command_rx.recv() => command,
            };
            let Some(command) = command else { break };
            match command {
                Command::Write(data) => {
                    if writer.write_all(&data).await.is_err() {
                        break;
                    }
                    let _ = writer.flush().await;
                }
                Command::Resize { cols, rows } => {
                    let _ = writer.resize(pty_process::Size::new(rows, cols));
                }
            }
        }
    });

    let opened = tx.try_send(PtyItem::Opened { pty: id });
    if opened.is_err() {
        return Err(failed("output queue refused the open event".to_owned()));
    }

    tokio::spawn(async move {
        let mut buffer = vec![0u8; READ_BUFFER];
        let mut chunker = Chunker::new();
        loop {
            let read = tokio::select! {
                biased;
                () = stop.cancelled() => break,
                read = reader.read(&mut buffer) => read,
            };
            let Ok(count) = read else { break };
            if count == 0 {
                break;
            }
            for chunk in Chunker::split(&buffer[..count]) {
                let dropped = chunker.take_dropped();
                let item = PtyItem::Output { data: chunk };
                if dropped > 0 {
                    if tx.send(item).await.is_err() {
                        return;
                    }
                } else if let Err(err) = tx.try_send(item) {
                    match err {
                        mpsc::error::TrySendError::Full(_) => {
                            chunker.note_drop(1);
                        }
                        mpsc::error::TrySendError::Closed(_) => return,
                    }
                }
            }
        }
        if stop.is_cancelled() {
            let _ = child.start_kill();
        }
        let code = tokio::select! {
            biased;
            status = child.wait() => status.ok().and_then(|status| status.code()),
            () = stop.cancelled() => {
                let _ = child.start_kill();
                child.wait().await.ok().and_then(|status| status.code())
            }
        };
        let _ = tx.send(PtyItem::Exited { code }).await;
    });

    Ok(Spawned { output, terminal })
}

fn failed(message: String) -> CallError {
    CallError::new(ErrorCode::Internal, message).with_execution(Execution::KnownFailed)
}

#[cfg(test)]
mod tests {
    use super::spawn;
    use crate::pty::Terminals;
    use goat_api::PtyItem;
    use std::sync::Arc;

    async fn drain_until_exit(spawned: super::Spawned) -> (Vec<String>, Option<i32>) {
        let mut output = spawned.output;
        let mut text = Vec::new();
        let mut code = None;
        while let Some(item) = output.recv().await {
            match item {
                PtyItem::Opened { .. } => {}
                PtyItem::Output { data } => text.push(data),
                PtyItem::Exited { code: exit } => {
                    code = exit;
                    break;
                }
            }
        }
        (text, code)
    }

    #[tokio::test]
    async fn a_command_runs_and_reports_its_exit_code() {
        let spawned = spawn(
            &std::env::temp_dir().display().to_string(),
            80,
            24,
            Some("printf goatpty; exit 3"),
            "pty_1".to_owned(),
        )
        .expect("a pty spawns");
        let (text, code) = drain_until_exit(spawned).await;
        assert_eq!(code, Some(3));
        assert!(
            text.concat().contains("goatpty"),
            "expected the command output, got {text:?}"
        );
    }

    #[tokio::test]
    async fn the_first_item_is_always_the_open_marker() {
        let mut spawned = spawn(
            &std::env::temp_dir().display().to_string(),
            80,
            24,
            Some("true"),
            "pty_7".to_owned(),
        )
        .expect("a pty spawns");
        let first = spawned.output.recv().await.expect("an open marker");
        assert_eq!(
            first,
            PtyItem::Opened {
                pty: "pty_7".to_owned()
            }
        );
    }

    #[tokio::test]
    async fn input_written_to_the_terminal_is_echoed_back() {
        let spawned = spawn(
            &std::env::temp_dir().display().to_string(),
            80,
            24,
            Some("read line; printf 'got:%s' \"$line\""),
            "pty_2".to_owned(),
        )
        .expect("a pty spawns");
        let terminal = spawned.terminal.clone();
        terminal.write(b"hello\n").expect("write reaches the pty");
        let (text, _code) = drain_until_exit(spawned).await;
        assert!(
            text.concat().contains("got:hello"),
            "expected the echoed input, got {text:?}"
        );
    }

    #[tokio::test]
    async fn closing_a_terminal_stops_the_stream() {
        let terminals = Terminals::new();
        let spawned = spawn(
            &std::env::temp_dir().display().to_string(),
            80,
            24,
            Some("sleep 60"),
            "pty_3".to_owned(),
        )
        .expect("a pty spawns");
        let mut output = spawned.output;
        terminals
            .insert("pty_3".to_owned(), Arc::clone(&spawned.terminal))
            .await;

        assert!(matches!(output.recv().await, Some(PtyItem::Opened { .. })));
        terminals.close("pty_3").await;

        let closed = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while output.recv().await.is_some() {}
        })
        .await;
        assert!(closed.is_ok(), "closing must end the output stream");
        assert_eq!(terminals.count().await, 0);
    }

    #[tokio::test]
    async fn resizing_a_live_terminal_succeeds() {
        let spawned = spawn(
            &std::env::temp_dir().display().to_string(),
            80,
            24,
            Some("sleep 30"),
            "pty_4".to_owned(),
        )
        .expect("a pty spawns");
        spawned
            .terminal
            .resize(120, 40)
            .expect("resize reaches the pty");
        spawned.terminal.close();
    }

    #[tokio::test]
    async fn spawning_in_a_missing_directory_is_reported() {
        let spawned = spawn(
            "/definitely/not/a/directory",
            80,
            24,
            Some("true"),
            "pty_5".to_owned(),
        );
        let Err(err) = spawned else {
            panic!("spawning in a missing directory must fail")
        };
        assert_eq!(err.code, goat_wire::envelope::ErrorCode::Internal);
    }
}
