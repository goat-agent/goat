mod conn;
mod manager;
mod remote;
mod session;

use std::path::{Path, PathBuf};

use goat_wire::transport;
use tokio_util::sync::CancellationToken;

pub use crate::manager::{Manager, ReloadRequest};

#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("a daemon is already running at {0}")]
    AlreadyRunning(PathBuf),
    #[error("remote error: {0}")]
    Remote(#[from] goat_remote::RemoteError),
}

pub struct DaemonConfig {
    pub socket_path: PathBuf,
    pub auth_path: PathBuf,
    pub config_json: PathBuf,
    pub db_path: PathBuf,
    pub remote: Option<RemoteSettings>,
}

pub struct RemoteSettings {
    pub remote_dir: PathBuf,
    pub bind: std::net::SocketAddr,
    pub advertised: Vec<String>,
}

pub async fn serve(config: DaemonConfig) -> Result<(), DaemonError> {
    let manager = Manager::new(
        config.auth_path.clone(),
        goat_config::UserProviders::at(config.config_json.clone()),
        config.db_path.clone(),
    );
    let shutdown = CancellationToken::new();
    let signal = shutdown.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        tracing::info!("received termination signal, shutting down");
        signal.cancel();
    });
    serve_with(config, manager, shutdown).await
}

pub async fn serve_with(
    config: DaemonConfig,
    manager: Manager,
    shutdown: CancellationToken,
) -> Result<(), DaemonError> {
    let listener = bind(&config.socket_path)?;
    let db_path = config.db_path.clone();
    sweep_orphaned_turns(&config.db_path).await;
    sweep_orphaned_processes(&config.db_path).await;
    tracing::info!(socket = %config.socket_path.display(), "daemon listening");

    if let Some(remote_settings) = config.remote {
        spawn_remote(&manager, &shutdown, remote_settings)?;
    }

    loop {
        tokio::select! {
            () = shutdown.cancelled() => {
                tracing::info!("daemon shutting down");
                break;
            }
            accepted = listener.accept() => match accepted {
                Ok(stream) => {
                    let manager = manager.clone();
                    let shutdown = shutdown.clone();
                    tokio::spawn(conn::handle_connection(stream, manager, shutdown));
                }
                Err(err) => {
                    tracing::warn!(%err, "accept failed");
                }
            },
        }
    }

    manager.shutdown_all_sessions().await;
    sweep_orphaned_processes(&db_path).await;
    transport::cleanup(&config.socket_path);
    Ok(())
}

#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut term), Ok(mut int)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) else {
        std::future::pending::<()>().await;
        return;
    };
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

fn spawn_remote(
    manager: &Manager,
    shutdown: &tokio_util::sync::CancellationToken,
    settings: RemoteSettings,
) -> Result<(), DaemonError> {
    let devices_path = settings.remote_dir.join("devices.json");
    let devices = goat_remote::Devices::load(devices_path)?;
    let config = goat_remote::RemoteConfig {
        remote_dir: settings.remote_dir,
        bind: settings.bind,
        advertised: settings.advertised,
    };
    let server = goat_remote::RemoteServer::new(config, devices.clone())?;
    manager.set_remote(
        server.pairing(),
        server.devices(),
        server.server_fingerprint().to_owned(),
        server.advertised().to_vec(),
    );
    let handler = remote::handler(manager.clone(), devices, shutdown.clone());
    let shutdown = shutdown.clone();
    tokio::spawn(async move {
        if let Err(err) = server.run(handler, shutdown).await {
            tracing::warn!(%err, "remote server stopped");
        }
    });
    Ok(())
}

pub fn already_running(socket_path: &Path) -> bool {
    transport::exists(socket_path) && transport::probe_alive(socket_path)
}

fn bind(socket_path: &Path) -> Result<transport::Listener, DaemonError> {
    if already_running(socket_path) {
        return Err(DaemonError::AlreadyRunning(socket_path.to_path_buf()));
    }
    Ok(transport::bind(socket_path)?)
}

async fn sweep_orphaned_turns(db_path: &Path) {
    let Ok(store) = goat_store::CodeStore::open(db_path).await else {
        return;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    match store.mark_running_turns_interrupted(now).await {
        Ok(n) if n > 0 => tracing::info!(count = n, "marked orphaned turns interrupted"),
        Ok(_) => {}
        Err(err) => tracing::warn!(%err, "failed to sweep orphaned turns"),
    }
}

async fn sweep_orphaned_processes(db_path: &Path) {
    let Ok(store) = goat_store::CodeStore::open(db_path).await else {
        return;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    match store.take_orphan_processes(now).await {
        Ok(orphans) => {
            for orphan in &orphans {
                kill_process_group(orphan.pgid);
            }
            if !orphans.is_empty() {
                tracing::info!(
                    count = orphans.len(),
                    "killed orphaned background processes"
                );
            }
        }
        Err(err) => tracing::warn!(%err, "failed to sweep orphaned processes"),
    }
}

fn kill_process_group(pgid: i64) {
    let Ok(pgid) = i32::try_from(pgid) else {
        return;
    };
    if let Err(err) = goat_process::kill_group(pgid) {
        tracing::warn!(%err, pgid, "failed to kill orphaned process group");
    }
}
