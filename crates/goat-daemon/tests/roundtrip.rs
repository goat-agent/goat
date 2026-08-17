use std::path::PathBuf;
use std::time::Duration;

use goat_api::{
    Api, ConversationList, ConversationListParams, Empty, ResumeMode, SessionKill,
    SessionKillParams, SessionList, SessionOpen, SessionOpenParams, SessionSubmit,
    SessionSubmitParams, SessionWatch, SessionWatchParams, StreamEvent, WatchFrom, WatchItem,
};
use goat_client::{ApiSession, Link};
use goat_wire::transport;

async fn start_daemon(dir: &std::path::Path) -> PathBuf {
    let socket = dir.join("d.sock");
    let cfg = goat_daemon::DaemonConfig {
        socket_path: socket.clone(),
        lock_path: dir.join("daemon.lock"),
        auth_path: dir.join("auth.json"),
        config_json: dir.join("config.json"),
        db_path: dir.join("store.sqlite"),
        remote: None,
    };
    tokio::spawn(async move {
        let _ = goat_daemon::serve(cfg).await;
    });
    for _ in 0..50 {
        if transport::connect(&socket).await.is_ok() {
            return socket;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("daemon did not start");
}

fn link(socket: &std::path::Path) -> Link {
    Link::local(socket.to_path_buf(), PathBuf::new())
}

async fn connect(socket: &std::path::Path) -> ApiSession {
    goat_client::open_api(&link(socket), "roundtrip")
        .await
        .expect("the daemon greets an envelope client")
}

async fn open(api: &Api, cwd: &std::path::Path, resume: ResumeMode) -> goat_api::SessionId {
    api.call::<SessionOpen>(SessionOpenParams {
        cwd: cwd.display().to_string(),
        resume,
    })
    .await
    .expect("session opens")
    .session
}

#[tokio::test]
async fn the_daemon_advertises_its_method_table_before_the_client_speaks() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;
    let session = connect(&socket).await;

    assert!(session.daemon.compatible());
    assert!(session.daemon.speaks("session.open", 1));
    assert!(session.daemon.speaks("session.watch", 1));
    assert!(
        session.daemon.speaks("admin.daemon_stop", 1),
        "a local client is granted the admin routes"
    );
}

#[tokio::test]
async fn open_session_and_list() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;
    let session = connect(&socket).await;
    let opened = open(&session.api, dir.path(), ResumeMode::New {}).await;

    let other = connect(&socket).await;
    let listed = other
        .api
        .call::<SessionList>(Empty {})
        .await
        .expect("sessions list");
    assert!(
        listed.sessions.iter().any(|s| s.session == opened),
        "the open session is visible to another client"
    );
}

#[tokio::test]
async fn submit_message_flows_back_as_events() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;
    let session = connect(&socket).await;
    let opened = open(&session.api, dir.path(), ResumeMode::New {}).await;

    let mut watch = session
        .api
        .open::<SessionWatch>(SessionWatchParams {
            session: opened,
            from: WatchFrom::Snapshot {},
        })
        .await
        .expect("watch opens");

    let task = session
        .api
        .call::<SessionSubmit>(SessionSubmitParams {
            session: opened,
            op: goat_protocol::Op::SubmitMessage {
                id: goat_protocol::TaskId(0),
                text: "hello".to_owned(),
                display: None,
                attachments: Vec::new(),
            },
        })
        .await
        .expect("submit is accepted");
    assert!(
        task.task.0 > 0,
        "submit mints the task id instead of the client correlating one"
    );

    let mut saw_event = false;
    for _ in 0..20 {
        match tokio::time::timeout(Duration::from_secs(5), watch.recv()).await {
            Ok(Some(StreamEvent::Item {
                item: WatchItem::Event { .. },
                ..
            })) => {
                saw_event = true;
                break;
            }
            Ok(Some(StreamEvent::Item { .. })) => {}
            Ok(Some(StreamEvent::End(_)) | None) | Err(_) => break,
        }
    }
    assert!(saw_event, "expected at least one engine event on the watch");
}

#[tokio::test]
async fn same_conversation_id_returns_same_session() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;

    let a = connect(&socket).await;
    let first = open(
        &a.api,
        dir.path(),
        ResumeMode::Conversation { conversation_id: 1 },
    )
    .await;

    let b = connect(&socket).await;
    let second = open(
        &b.api,
        dir.path(),
        ResumeMode::Conversation { conversation_id: 1 },
    )
    .await;

    assert_eq!(first, second, "one conversation has one live session");
}

#[tokio::test]
async fn distinct_conversation_ids_get_distinct_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;

    let a = connect(&socket).await;
    let first = open(
        &a.api,
        dir.path(),
        ResumeMode::Conversation { conversation_id: 1 },
    )
    .await;

    let b = connect(&socket).await;
    let second = open(
        &b.api,
        dir.path(),
        ResumeMode::Conversation { conversation_id: 2 },
    )
    .await;

    assert_ne!(
        first, second,
        "different conversations are different sessions"
    );
}

#[tokio::test]
async fn kill_session_removes_it_from_the_list() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;
    let session = connect(&socket).await;
    let opened = open(&session.api, dir.path(), ResumeMode::New {}).await;

    session
        .api
        .call::<SessionKill>(SessionKillParams { session: opened })
        .await
        .expect("kill is accepted");

    let listed = session
        .api
        .call::<SessionList>(Empty {})
        .await
        .expect("sessions list");
    assert!(
        !listed.sessions.iter().any(|s| s.session == opened),
        "a killed session leaves the list"
    );
}

#[tokio::test]
async fn reopening_with_another_conversation_leaves_the_first_session_alive() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;

    let a = connect(&socket).await;
    let first = open(
        &a.api,
        dir.path(),
        ResumeMode::Conversation { conversation_id: 1 },
    )
    .await;

    let b = connect(&socket).await;
    let shared = open(
        &b.api,
        dir.path(),
        ResumeMode::Conversation { conversation_id: 1 },
    )
    .await;
    assert_eq!(first, shared, "both windows share the live session");

    let moved = open(
        &b.api,
        dir.path(),
        ResumeMode::Conversation { conversation_id: 2 },
    )
    .await;
    assert_ne!(moved, first, "the moved window is on a different session");

    let admin = connect(&socket).await;
    let listed = admin
        .api
        .call::<SessionList>(Empty {})
        .await
        .expect("sessions list");
    assert!(
        listed.sessions.iter().any(|s| s.session == first),
        "the original session stays alive for the window that did not move"
    );
}

#[tokio::test]
async fn clearing_opens_a_fresh_session_for_the_same_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;
    let session = connect(&socket).await;

    let first = open(&session.api, dir.path(), ResumeMode::New {}).await;
    let cleared = open(&session.api, dir.path(), ResumeMode::New {}).await;

    assert_ne!(
        first, cleared,
        "clearing is the client opening a new session, not the daemon rebinding one"
    );
}

#[tokio::test]
async fn list_conversations_answers_for_a_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;
    let session = connect(&socket).await;

    let listed = session
        .api
        .call::<ConversationList>(ConversationListParams {
            cwd: dir.path().display().to_string(),
        })
        .await
        .expect("conversation list answers");
    assert!(listed.conversations.is_empty());
}
