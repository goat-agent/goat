use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use goat_remote::{Device, Devices, RemoteHandler, RemoteSink, RemoteStream};

use crate::envelope_conn::{ClientOrigin, EnvelopeHost, serve_envelope};

pub(crate) struct DaemonRemoteHandler {
    pub(crate) host: EnvelopeHost,
    pub(crate) devices: Devices,
}

impl RemoteHandler for DaemonRemoteHandler {
    fn handle(
        &self,
        device: Device,
        sink: RemoteSink,
        stream: RemoteStream,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let host = self.host.clone();
        let devices = self.devices.clone();
        let fingerprint = device.fingerprint.clone();
        let origin = ClientOrigin::Remote { device: device.id };
        let disconnect = tokio_util::sync::CancellationToken::new();
        let watcher = disconnect.clone();
        Box::pin(async move {
            let revocation = tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    if !devices.contains_fingerprint(&fingerprint).await {
                        watcher.cancel();
                        break;
                    }
                }
            });
            serve_envelope(host, origin, sink, stream, disconnect).await;
            revocation.abort();
        })
    }
}

pub(crate) fn handler(host: EnvelopeHost, devices: Devices) -> Arc<DaemonRemoteHandler> {
    Arc::new(DaemonRemoteHandler { host, devices })
}
