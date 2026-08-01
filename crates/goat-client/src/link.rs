use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;

use futures::{Sink, SinkExt, Stream, StreamExt};
use goat_remote::client::DeviceCredentials;
use goat_wire::transport;
use goat_wire::{ClientConn, ClientFrame, ServerFrame, WireError};

use crate::ClientError;

pub type FrameSink = Pin<Box<dyn Sink<ClientFrame, Error = WireError> + Send>>;
pub type FrameStream = Pin<Box<dyn Stream<Item = Result<ServerFrame, WireError>> + Send>>;

pub const LOCAL: &str = "local";

#[derive(Debug, Clone)]
pub enum Link {
    Local {
        socket_path: PathBuf,
        daemon_exe: PathBuf,
    },
    Remote {
        name: String,
        host: String,
        credentials: DeviceCredentials,
    },
}

impl Link {
    #[must_use]
    pub fn local(socket_path: PathBuf, daemon_exe: PathBuf) -> Self {
        Self::Local {
            socket_path,
            daemon_exe,
        }
    }

    #[must_use]
    pub fn remote(name: String, host: String, credentials: DeviceCredentials) -> Self {
        Self::Remote {
            name,
            host,
            credentials,
        }
    }

    #[must_use]
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local { .. })
    }

    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Local { .. } => LOCAL,
            Self::Remote { name, .. } => name,
        }
    }

    pub async fn dial(&self) -> Result<Conn, ClientError> {
        match self {
            Self::Local { socket_path, .. } => {
                let stream = transport::connect(socket_path).await?;
                Ok(Conn::local(stream))
            }
            Self::Remote {
                host, credentials, ..
            } => {
                let (sink, stream) = goat_remote::client::connect(host, credentials).await?;
                Ok(Conn { sink, stream })
            }
        }
    }

    pub async fn dial_or_spawn(&self) -> Result<Conn, ClientError> {
        let Self::Local {
            socket_path,
            daemon_exe,
        } = self
        else {
            return self.dial().await;
        };
        if let Ok(conn) = self.dial().await {
            return Ok(conn);
        }
        spawn_daemon(daemon_exe, socket_path)?;
        for _ in 0..50 {
            if let Ok(conn) = self.dial().await {
                return Ok(conn);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err(ClientError::SpawnFailed(
            "daemon did not become reachable".to_owned(),
        ))
    }
}

pub struct Conn {
    sink: FrameSink,
    stream: FrameStream,
}

impl Conn {
    fn local(stream: transport::Stream) -> Self {
        let conn: ClientConn<transport::Stream> = ClientConn::new(stream);
        let (sink, stream) = conn.split();
        Self {
            sink: Box::pin(sink),
            stream: Box::pin(stream),
        }
    }

    pub async fn send(&mut self, frame: ClientFrame) -> Result<(), WireError> {
        self.sink.send(frame).await
    }

    pub async fn recv(&mut self) -> Result<ServerFrame, WireError> {
        match self.stream.next().await {
            Some(frame) => frame,
            None => Err(WireError::Closed),
        }
    }

    #[must_use]
    pub fn split(self) -> (FrameSink, FrameStream) {
        (self.sink, self.stream)
    }
}

fn spawn_daemon(daemon_exe: &Path, socket_path: &Path) -> Result<(), ClientError> {
    use std::process::{Command, Stdio};
    let stderr = daemon_stderr(socket_path);
    Command::new(daemon_exe)
        .arg("daemon")
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn()
        .map_err(|e| ClientError::SpawnFailed(e.to_string()))?;
    Ok(())
}

fn daemon_stderr(socket_path: &Path) -> std::process::Stdio {
    use std::process::Stdio;
    let Some(home) = socket_path.parent() else {
        return Stdio::null();
    };
    let log_dir = home.join("logs");
    if std::fs::create_dir_all(&log_dir).is_err() {
        return Stdio::null();
    }
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("daemon-stderr.log"))
    {
        Ok(file) => Stdio::from(file),
        Err(_) => Stdio::null(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credentials() -> DeviceCredentials {
        DeviceCredentials {
            key_pem: String::new(),
            cert_pem: String::new(),
            ca_cert_pem: String::new(),
            server_fingerprint: String::new(),
        }
    }

    #[test]
    fn local_link_reports_the_reserved_name() {
        let link = Link::local(PathBuf::from("/tmp/goat.sock"), PathBuf::from("/bin/goat"));
        assert!(link.is_local());
        assert_eq!(link.name(), LOCAL);
    }

    #[test]
    fn remote_link_keeps_its_name() {
        let link = Link::remote("box".to_owned(), "1.2.3.4:4317".to_owned(), credentials());
        assert!(!link.is_local());
        assert_eq!(link.name(), "box");
    }

    #[tokio::test]
    async fn a_remote_link_never_spawns_a_daemon() {
        let link = Link::remote("box".to_owned(), "127.0.0.1:1".to_owned(), credentials());
        let failure = link.dial_or_spawn().await;
        assert!(matches!(failure, Err(ClientError::Remote(_))));
    }
}
