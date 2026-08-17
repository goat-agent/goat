mod api;
mod envelope_conn;
mod files;
mod manager;
mod pty;
mod pty_spawn;
mod remote;
mod session;

use std::path::{Path, PathBuf};

use goat_wire::transport;
use tokio_util::sync::CancellationToken;

pub use crate::api::{LOCAL_GRANTS, REMOTE_GRANTS, build as build_router};

pub use crate::envelope_conn::{
    ClientOrigin, EnvelopeHost, device_for, grants_for, serve_envelope,
};
pub use crate::manager::{CodeSessionHub, ReloadRequest};

#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("another daemon holds {0}")]
    AlreadyRunning(PathBuf),
    #[error("remote error: {0}")]
    Remote(#[from] goat_remote::RemoteError),
}

pub struct DaemonConfig {
    pub socket_path: PathBuf,
    pub lock_path: PathBuf,
    pub auth_path: PathBuf,
    pub config_json: PathBuf,
    pub db_path: PathBuf,
    pub remote: Option<RemoteSettings>,
}

pub struct DaemonLock {
    file: std::fs::File,
}

const LOCK_POLL: std::time::Duration = std::time::Duration::from_millis(100);

pub async fn acquire(
    lock_path: &Path,
    wait: std::time::Duration,
) -> Result<DaemonLock, DaemonError> {
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(DaemonLock { file }),
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(err)) => {
                tracing::warn!(%err, path = %lock_path.display(), "daemon lock unavailable; continuing without exclusion");
                return Ok(DaemonLock { file });
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(DaemonError::AlreadyRunning(lock_path.to_path_buf()));
        }
        tokio::time::sleep(LOCK_POLL).await;
    }
}

impl Drop for DaemonLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

pub struct RemoteSettings {
    pub remote_dir: PathBuf,
    pub bind: std::net::SocketAddr,
    pub advertised: Vec<String>,
}

pub async fn serve(config: DaemonConfig) -> Result<(), DaemonError> {
    let manager = CodeSessionHub::new(
        config.auth_path.clone(),
        goat_config::UserProviders::at(config.config_json.clone()),
        config.db_path.clone(),
    );
    let lock = acquire(&config.lock_path, std::time::Duration::ZERO).await?;
    let bound = bind_daemon(config, &lock)?;
    manager.mark_ready();
    let shutdown = CancellationToken::new();
    let signal = shutdown.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        tracing::info!("received termination signal, shutting down");
        signal.cancel();
    });
    serve_bound(bound, manager, shutdown).await
}

pub struct Bound {
    listener: transport::Listener,
    config: DaemonConfig,
}

pub fn bind_daemon(config: DaemonConfig, _lock: &DaemonLock) -> Result<Bound, DaemonError> {
    let listener = bind(&config.socket_path)?;
    tracing::info!(socket = %config.socket_path.display(), "daemon listening");
    Ok(Bound { listener, config })
}

pub async fn serve_with(
    config: DaemonConfig,
    manager: CodeSessionHub,
    shutdown: CancellationToken,
    lock: &DaemonLock,
) -> Result<(), DaemonError> {
    serve_bound(bind_daemon(config, lock)?, manager, shutdown).await
}

pub async fn serve_bound(
    bound: Bound,
    manager: CodeSessionHub,
    shutdown: CancellationToken,
) -> Result<(), DaemonError> {
    let Bound { listener, config } = bound;
    let db_path = config.db_path.clone();
    sweep_orphaned_turns(&config.db_path).await;
    sweep_orphaned_processes(&config.db_path).await;

    let host = EnvelopeHost {
        manager: manager.clone(),
        broker: std::sync::Arc::new(goat_capability::Broker::new()),
        shutdown: shutdown.clone(),
        epoch: manager.started_at().to_string(),
        terminals: std::sync::Arc::new(pty::Terminals::new()),
        db_path: db_path.clone(),
    };

    if let Some(remote_settings) = config.remote {
        spawn_remote(&manager, &host, &shutdown, remote_settings)?;
    }

    loop {
        tokio::select! {
            () = shutdown.cancelled() => {
                tracing::info!("daemon shutting down");
                break;
            }
            accepted = listener.accept() => match accepted {
                Ok(stream) => {
                    let host = host.clone();
                    tokio::spawn(async move {
                        let conn: goat_wire::WireConn<_, goat_wire::envelope::Frame, goat_wire::envelope::Frame> =
                            goat_wire::WireConn::new(stream);
                        let (sink, source) = conn.split();
                        serve_envelope(
                            host,
                            ClientOrigin::Local,
                            Box::pin(sink),
                            Box::pin(source),
                            CancellationToken::new(),
                        )
                        .await;
                    });
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
    manager: &CodeSessionHub,
    host: &EnvelopeHost,
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
    let handler = remote::handler(host.clone(), devices);
    let shutdown = shutdown.clone();
    tokio::spawn(async move {
        if let Err(err) = server.run(handler, shutdown).await {
            tracing::warn!(%err, "remote server stopped");
        }
    });
    Ok(())
}

fn bind(socket_path: &Path) -> Result<transport::Listener, DaemonError> {
    transport::cleanup(socket_path);
    Ok(transport::bind(socket_path)?)
}

async fn sweep_orphaned_turns(db_path: &Path) {
    let Ok(store) = goat_code_store::CodeStore::open(db_path).await else {
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
    let Ok(store) = goat_code_store::CodeStore::open(db_path).await else {
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
