use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::SinkExt;
use goat_api::{
    DaemonStatus, Empty, ResumeMode, SessionKill, SessionKillParams, SessionOpen,
    SessionOpenParams, SessionWatch, SessionWatchParams, WatchFrom,
};
use goat_client::{ApiSession, Link};
use goat_wire::transport;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

fn config(dir: &Path) -> goat_daemon::DaemonConfig {
    goat_daemon::DaemonConfig {
        socket_path: dir.join("d.sock"),
        lock_path: dir.join("daemon.lock"),
        auth_path: dir.join("auth.json"),
        config_json: dir.join("config.json"),
        db_path: dir.join("store.sqlite"),
        remote: None,
    }
}

async fn start_daemon(dir: &Path) -> PathBuf {
    let cfg = config(dir);
    let socket = cfg.socket_path.clone();
    tokio::spawn(async move {
        let _ = goat_daemon::serve(cfg).await;
    });
    wait_for_socket(&socket).await;
    socket
}

async fn wait_for_socket(socket: &Path) {
    for _ in 0..100 {
        if transport::connect(socket).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("daemon did not start");
}

async fn socket_is_gone(socket: &Path) -> bool {
    for _ in 0..200 {
        if transport::connect(socket).await.is_err() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

async fn connect(socket: &Path) -> ApiSession {
    goat_client::open_api(
        &Link::local(socket.to_path_buf(), PathBuf::new()),
        "lifecycle",
    )
    .await
    .expect("the daemon greets")
}

async fn busy(socket: &Path) -> (usize, usize) {
    let session = connect(socket).await;
    let status = session
        .api
        .call::<DaemonStatus>(Empty {})
        .await
        .expect("status answers");
    (status.sessions, status.turns)
}

#[tokio::test]
async fn greets_before_the_client_sends_anything() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;

    let session = connect(&socket).await;
    assert_eq!(session.daemon.role, goat_wire::envelope::Role::Daemon);
    assert!(session.daemon.compatible());
    assert_eq!(
        session.daemon.info["pid"].as_u64(),
        Some(u64::from(std::process::id()))
    );
}

#[tokio::test]
async fn a_client_that_cannot_encode_our_frames_can_still_stop_the_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;

    let stream = transport::connect(&socket).await.unwrap();
    let mut framed = Framed::new(stream, LengthDelimitedCodec::new());
    let hello = serde_json::to_vec(&serde_json::json!({
        "kind": "hello",
        "role": "client",
        "envelope": goat_wire::envelope_fingerprint(),
        "agent": "raw",
    }))
    .unwrap();
    framed.send(bytes::Bytes::from(hello)).await.unwrap();
    let stop = serde_json::to_vec(&serde_json::json!({
        "kind": "req",
        "id": "1",
        "method": "admin.daemon_stop",
        "version": 1,
        "params": { "if_idle": false },
    }))
    .unwrap();
    framed.send(bytes::Bytes::from(stop)).await.unwrap();

    assert!(
        socket_is_gone(&socket).await,
        "a hand-written admin.daemon_stop must shut the daemon down"
    );
}

#[tokio::test]
async fn busy_counts_a_live_session_and_clears_when_it_is_killed() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;

    assert_eq!(busy(&socket).await, (0, 0), "a fresh daemon is idle");

    let holder = connect(&socket).await;
    let session = holder
        .api
        .call::<SessionOpen>(SessionOpenParams {
            cwd: dir.path().display().to_string(),
            resume: ResumeMode::New {},
        })
        .await
        .unwrap()
        .session;
    let _watch = holder
        .api
        .open::<SessionWatch>(SessionWatchParams {
            session,
            from: WatchFrom::Snapshot {},
        })
        .await
        .expect("watch opens");

    let (sessions, turns) = busy(&socket).await;
    assert_eq!(sessions, 1, "an attached session is busy");
    assert_eq!(turns, 0, "no agent runtime is attached in this test");

    let probe = connect(&socket).await;
    probe
        .api
        .call::<SessionKill>(SessionKillParams { session })
        .await
        .unwrap();
    holder.shutdown();

    for _ in 0..100 {
        if busy(&socket).await == (0, 0) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("busy never returned to idle after the session was killed");
}

#[tokio::test]
async fn a_second_daemon_loses_the_lock_and_leaves_the_first_alive() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;

    let taken = goat_daemon::acquire(&dir.path().join("daemon.lock"), Duration::ZERO).await;
    assert!(
        matches!(taken, Err(goat_daemon::DaemonError::AlreadyRunning(_))),
        "the running daemon must own the lock"
    );
    drop(taken);

    assert!(connect(&socket).await.daemon.compatible());
}

#[tokio::test]
async fn a_stale_socket_is_reclaimed() {
    let dir = tempfile::tempdir().unwrap();
    let stale = dir.path().join("d.sock");
    std::fs::write(&stale, b"").unwrap();

    let socket = start_daemon(dir.path()).await;
    assert!(connect(&socket).await.daemon.compatible());
}

#[tokio::test]
async fn the_lock_is_released_when_the_daemon_exits() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;

    let link = Link::local(socket.clone(), PathBuf::new());
    goat_client::stop(&link).await.expect("the daemon stops");
    assert!(socket_is_gone(&socket).await);

    goat_daemon::acquire(&dir.path().join("daemon.lock"), Duration::from_secs(5))
        .await
        .expect("the lock is free once the daemon is down");
}
