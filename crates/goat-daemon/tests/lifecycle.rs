use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::SinkExt;
use goat_wire::transport::{self, Stream};
use goat_wire::{ClientConn, ResumeMode, ServerFrame, WireConn};
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

#[tokio::test]
async fn greets_before_the_client_sends_anything() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;

    let stream = transport::connect(&socket).await.unwrap();
    let mut conn: ClientConn<Stream> = WireConn::new(stream);
    match conn.recv().await.unwrap() {
        ServerFrame::Welcome {
            wire, pid, ready, ..
        } => {
            assert_eq!(wire, goat_wire::wire_fingerprint());
            assert_eq!(pid, std::process::id());
            assert!(ready);
        }
        other => panic!("expected Welcome, got {other:?}"),
    }
}

#[tokio::test]
async fn a_client_that_cannot_encode_our_frames_can_still_stop_the_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;

    let stream = transport::connect(&socket).await.unwrap();
    let mut framed = Framed::new(stream, LengthDelimitedCodec::new());
    framed
        .send(bytes::Bytes::from_static(br#"{"type":"StopDaemon"}"#))
        .await
        .unwrap();

    assert!(
        socket_is_gone(&socket).await,
        "a raw StopDaemon must shut the daemon down"
    );
}

#[tokio::test]
async fn busy_counts_a_live_session_and_clears_when_it_is_killed() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;

    let stream = transport::connect(&socket).await.unwrap();
    let mut conn: ClientConn<Stream> = WireConn::new(stream);
    let ServerFrame::Welcome { busy, .. } = conn.recv().await.unwrap() else {
        panic!("expected Welcome");
    };
    assert!(busy.is_idle(), "a fresh daemon is idle");

    conn.send(&goat_wire::ClientFrame::OpenSession {
        cwd: dir.path().display().to_string(),
        resume: ResumeMode::New {},
    })
    .await
    .unwrap();
    let session = loop {
        if let ServerFrame::SessionOpened { session, .. } = conn.recv().await.unwrap() {
            break session;
        }
    };

    let second = transport::connect(&socket).await.unwrap();
    let mut probe: ClientConn<Stream> = WireConn::new(second);
    let ServerFrame::Welcome { busy, .. } = probe.recv().await.unwrap() else {
        panic!("expected Welcome");
    };
    assert_eq!(busy.sessions, 1, "an attached session is busy");
    assert_eq!(busy.turns, 0, "no agent runtime is attached in this test");

    probe
        .send(&goat_wire::ClientFrame::KillSession { session })
        .await
        .unwrap();
    drop(conn);

    for _ in 0..100 {
        let stream = transport::connect(&socket).await.unwrap();
        let mut c: ClientConn<Stream> = WireConn::new(stream);
        if let ServerFrame::Welcome { busy, .. } = c.recv().await.unwrap()
            && busy.is_idle()
        {
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

    let stream = transport::connect(&socket).await.unwrap();
    let mut conn: ClientConn<Stream> = WireConn::new(stream);
    assert!(matches!(
        conn.recv().await.unwrap(),
        ServerFrame::Welcome { .. }
    ));
}

#[tokio::test]
async fn a_stale_socket_is_reclaimed() {
    let dir = tempfile::tempdir().unwrap();
    let stale = dir.path().join("d.sock");
    std::fs::write(&stale, b"").unwrap();

    let socket = start_daemon(dir.path()).await;
    let stream = transport::connect(&socket).await.unwrap();
    let mut conn: ClientConn<Stream> = WireConn::new(stream);
    assert!(matches!(
        conn.recv().await.unwrap(),
        ServerFrame::Welcome { .. }
    ));
}

#[tokio::test]
async fn the_lock_is_released_when_the_daemon_exits() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;

    let stream = transport::connect(&socket).await.unwrap();
    let mut conn: ClientConn<Stream> = WireConn::new(stream);
    let _ = conn.recv().await.unwrap();
    conn.send(&goat_wire::ClientFrame::StopDaemon {})
        .await
        .unwrap();
    assert!(socket_is_gone(&socket).await);

    goat_daemon::acquire(&dir.path().join("daemon.lock"), Duration::from_secs(5))
        .await
        .expect("the lock is free once the daemon is down");
}
