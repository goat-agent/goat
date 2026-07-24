use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use goat_remote::{Device, Devices, RemoteHandler, RemoteSink, RemoteStream};

use crate::conn::{ClientOrigin, serve_connection};
use crate::manager::Manager;

pub(crate) struct DaemonRemoteHandler {
    pub(crate) manager: Manager,
    pub(crate) devices: Devices,
    pub(crate) shutdown: tokio_util::sync::CancellationToken,
}

impl RemoteHandler for DaemonRemoteHandler {
    fn handle(
        &self,
        device: Device,
        sink: RemoteSink,
        stream: RemoteStream,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let manager = self.manager.clone();
        let shutdown = self.shutdown.clone();
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
            serve_connection(sink, stream, manager, shutdown, origin, disconnect).await;
            revocation.abort();
        })
    }
}

pub(crate) fn handler(
    manager: Manager,
    devices: Devices,
    shutdown: tokio_util::sync::CancellationToken,
) -> Arc<DaemonRemoteHandler> {
    Arc::new(DaemonRemoteHandler {
        manager,
        devices,
        shutdown,
    })
}
