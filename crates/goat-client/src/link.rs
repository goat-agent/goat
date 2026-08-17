use std::path::{Path, PathBuf};
use std::pin::Pin;

use futures::{Sink, Stream};
use goat_remote::client::DeviceCredentials;
use goat_wire::WireError;
use goat_wire::transport;

use crate::ClientError;

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

    pub async fn dial_envelope(&self) -> Result<EnvelopeConn, ClientError> {
        match self {
            Self::Local { socket_path, .. } => {
                let stream = transport::connect(socket_path).await?;
                let conn: goat_wire::EnvelopeConn<transport::Stream> =
                    goat_wire::EnvelopeConn::new(stream);
                let (sink, source) = conn.split();
                Ok(EnvelopeConn {
                    sink: Box::pin(sink),
                    source: Box::pin(source),
                })
            }
            Self::Remote {
                host, credentials, ..
            } => {
                let (sink, source) = goat_remote::client::connect::<
                    goat_wire::envelope::Frame,
                    goat_wire::envelope::Frame,
                >(host, credentials)
                .await?;
                Ok(EnvelopeConn { sink, source })
            }
        }
    }

    pub(crate) fn spawn_local(&self) -> Result<(), ClientError> {
        let Self::Local {
            socket_path,
            daemon_exe,
        } = self
        else {
            return Err(ClientError::Refused(
                "a remote daemon cannot be started from this machine".to_owned(),
            ));
        };
        spawn_daemon(daemon_exe, socket_path)
    }

    pub(crate) fn local_parts(&self) -> Option<(&Path, &Path)> {
        match self {
            Self::Local {
                socket_path,
                daemon_exe,
            } => Some((socket_path, daemon_exe)),
            Self::Remote { .. } => None,
        }
    }
}

pub type EnvelopeSink = Pin<Box<dyn Sink<goat_wire::envelope::Frame, Error = WireError> + Send>>;
pub type EnvelopeSource =
    Pin<Box<dyn Stream<Item = Result<goat_wire::envelope::Frame, WireError>> + Send>>;

pub struct EnvelopeConn {
    pub sink: EnvelopeSink,
    pub source: EnvelopeSource,
}

impl EnvelopeConn {
    #[must_use]
    pub fn split(self) -> (EnvelopeSink, EnvelopeSource) {
        (self.sink, self.source)
    }
}

fn spawn_daemon(daemon_exe: &Path, socket_path: &Path) -> Result<(), ClientError> {
    use std::process::{Command, Stdio};
    let stderr = daemon_stderr(socket_path);
    Command::new(daemon_exe)
        .arg("daemon")
        .env_clear()
        .envs(goat_process::child_environment())
        .arg("serve")
        .arg("--detached")
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

    #[test]
    fn remote_link_has_no_local_spawn_target() {
        let link = Link::remote("box".to_owned(), "127.0.0.1:1".to_owned(), credentials());
        assert!(link.local_parts().is_none());
    }
}
