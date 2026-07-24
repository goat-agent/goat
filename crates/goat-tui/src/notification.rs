use std::io::Write as _;

#[cfg(target_os = "macos")]
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Notification {
    Completion,
    Attention,
}

impl Notification {
    #[cfg(target_os = "macos")]
    fn message(self) -> &'static str {
        match self {
            Self::Completion => "Response completed",
            Self::Attention => "Input needed",
        }
    }
}

pub(crate) fn spawn(notification: Notification) {
    tokio::spawn(async move {
        let delivered = tokio::task::spawn_blocking(move || show_desktop(notification))
            .await
            .unwrap_or(false);
        if !delivered {
            ring_bell();
        }
    });
}

#[cfg(target_os = "macos")]
fn show_desktop(notification: Notification) -> bool {
    show_platform(notification)
}

#[cfg(not(target_os = "macos"))]
fn show_desktop(_notification: Notification) -> bool {
    false
}

#[cfg(target_os = "macos")]
fn show_platform(notification: Notification) -> bool {
    let script = format!(
        "display notification {:?} with title {:?}",
        notification.message(),
        "Goat"
    );
    let child = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        tracing::debug!("desktop notification process did not start");
        return false;
    };
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    return true;
                }
                tracing::debug!(status = ?status, "desktop notification process failed");
                return false;
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                tracing::debug!("desktop notification process timed out");
                return false;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(err) => {
                tracing::debug!(error = %err, "desktop notification process wait failed");
                return false;
            }
        }
    }
}

fn ring_bell() {
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x07");
    let _ = out.flush();
}
