mod ca;
mod devices;
mod pairing;
mod server;
mod verify;

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use futures::{Sink, Stream};
use goat_wire::{ClientFrame, ServerFrame, WireError};

pub use ca::{Authority, SignedDevice, fingerprint_der, fingerprint_pem};
pub use devices::{Device, Devices};
pub use pairing::Pairing;
pub use verify::{Allowlist, DeviceVerifier};

#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("certificate error: {0}")]
    Cert(#[from] rcgen::Error),
    #[error("tls error: {0}")]
    Tls(#[from] rustls::Error),
    #[error("pem decode error")]
    Pem,
    #[error("bind error: {0}")]
    Bind(String),
}

pub type RemoteSink = Pin<Box<dyn Sink<ServerFrame, Error = WireError> + Send>>;
pub type RemoteStream = Pin<Box<dyn Stream<Item = Result<ClientFrame, WireError>> + Send>>;

pub trait RemoteHandler: Send + Sync + 'static {
    fn handle(
        &self,
        device: Device,
        sink: RemoteSink,
        stream: RemoteStream,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

pub struct RemoteConfig {
    pub remote_dir: PathBuf,
    pub bind: std::net::SocketAddr,
    pub advertised: Vec<String>,
}

pub use server::RemoteServer;
