use std::path::PathBuf;
use std::time::Duration;

use goat_wire::transport::{self, Stream};
use goat_wire::{ClientConn, ClientFrame, ResumeMode, ServerFrame, WireConn};

async fn start_daemon(dir: &std::path::Path) -> PathBuf {
    let socket = dir.join("d.sock");
    let auth = dir.join("auth.json");
    let db = dir.join("store.sqlite");
    let cfg = goat_daemon::DaemonConfig {
        socket_path: socket.clone(),
        lock_path: dir.join("daemon.lock"),
        auth_path: auth,
        config_json: dir.join("config.json"),
        db_path: db,
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

async fn connect(socket: &std::path::Path) -> ClientConn<Stream> {
    let stream = transport::connect(socket).await.unwrap();
    let mut conn: ClientConn<Stream> = WireConn::new(stream);
    match conn.recv().await.unwrap() {
        ServerFrame::Welcome { wire, .. } => assert_eq!(wire, goat_wire::wire_fingerprint()),
        other => panic!("expected Welcome, got {other:?}"),
    }
    conn
}

#[tokio::test]
async fn open_session_and_list() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;
    let mut conn = connect(&socket).await;

    conn.send(&ClientFrame::OpenSession {
        cwd: dir.path().display().to_string(),
        resume: ResumeMode::New {},
    })
    .await
    .unwrap();
    let session = match conn.recv().await.unwrap() {
        ServerFrame::SessionOpened { session, .. } => session,
        other => panic!("expected SessionOpened, got {other:?}"),
    };

    let mut lister = connect(&socket).await;
    lister.send(&ClientFrame::ListSessions {}).await.unwrap();
    match lister.recv().await.unwrap() {
        ServerFrame::Sessions { sessions } => {
            assert!(sessions.iter().any(|s| s.session == session));
        }
        other => panic!("expected Sessions, got {other:?}"),
    }
}

#[tokio::test]
async fn submit_message_flows_back_as_events() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;
    let mut conn = connect(&socket).await;

    conn.send(&ClientFrame::OpenSession {
        cwd: dir.path().display().to_string(),
        resume: ResumeMode::New {},
    })
    .await
    .unwrap();
    let session = match conn.recv().await.unwrap() {
        ServerFrame::SessionOpened { session, .. } => session,
        other => panic!("expected SessionOpened, got {other:?}"),
    };

    conn.send(&ClientFrame::Submit {
        session,
        correlation: 1,
        op: goat_protocol::Op::SubmitMessage {
            id: goat_protocol::TaskId(1),
            text: "hello".to_owned(),
            display: None,
            attachments: Vec::new(),
        },
    })
    .await
    .unwrap();

    let mut saw_seq_event = false;
    for _ in 0..20 {
        match tokio::time::timeout(Duration::from_secs(5), conn.recv()).await {
            Ok(Ok(ServerFrame::Event {
                session: s, seq, ..
            })) => {
                assert_eq!(s, session);
                let _ = seq;
                saw_seq_event = true;
                break;
            }
            Ok(Ok(_)) => {}
            Ok(Err(_)) | Err(_) => break,
        }
    }
    assert!(
        saw_seq_event,
        "expected at least one seq-stamped event from the engine"
    );
}

#[tokio::test]
async fn same_conversation_id_returns_same_session() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;

    let mut a = connect(&socket).await;
    a.send(&ClientFrame::OpenSession {
        cwd: dir.path().display().to_string(),
        resume: ResumeMode::Conversation {
            conversation_id: 99,
        },
    })
    .await
    .unwrap();
    let first = match a.recv().await.unwrap() {
        ServerFrame::SessionOpened { session, .. } => session,
        other => panic!("expected SessionOpened, got {other:?}"),
    };

    let mut b = connect(&socket).await;
    b.send(&ClientFrame::OpenSession {
        cwd: dir.path().display().to_string(),
        resume: ResumeMode::Conversation {
            conversation_id: 99,
        },
    })
    .await
    .unwrap();
    let second = match b.recv().await.unwrap() {
        ServerFrame::SessionOpened { session, .. } => session,
        other => panic!("expected SessionOpened, got {other:?}"),
    };

    assert_eq!(
        first, second,
        "opening the same conversation must converge on the one live session"
    );
}

#[tokio::test]
async fn distinct_conversation_ids_get_distinct_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;

    let mut a = connect(&socket).await;
    a.send(&ClientFrame::OpenSession {
        cwd: dir.path().display().to_string(),
        resume: ResumeMode::Conversation { conversation_id: 1 },
    })
    .await
    .unwrap();
    let first = match a.recv().await.unwrap() {
        ServerFrame::SessionOpened { session, .. } => session,
        other => panic!("expected SessionOpened, got {other:?}"),
    };

    let mut b = connect(&socket).await;
    b.send(&ClientFrame::OpenSession {
        cwd: dir.path().display().to_string(),
        resume: ResumeMode::Conversation { conversation_id: 2 },
    })
    .await
    .unwrap();
    let second = match b.recv().await.unwrap() {
        ServerFrame::SessionOpened { session, .. } => session,
        other => panic!("expected SessionOpened, got {other:?}"),
    };

    assert_ne!(
        first, second,
        "different conversations must run as independent sessions"
    );
}

#[tokio::test]
async fn kill_session_removes_it_from_the_list() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;
    let mut conn = connect(&socket).await;
    conn.send(&ClientFrame::OpenSession {
        cwd: dir.path().display().to_string(),
        resume: ResumeMode::New {},
    })
    .await
    .unwrap();
    let session = match conn.recv().await.unwrap() {
        ServerFrame::SessionOpened { session, .. } => session,
        other => panic!("expected SessionOpened, got {other:?}"),
    };

    let mut admin = connect(&socket).await;
    admin
        .send(&ClientFrame::KillSession { session })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    admin.send(&ClientFrame::ListSessions {}).await.unwrap();
    match admin.recv().await.unwrap() {
        ServerFrame::Sessions { sessions } => {
            assert!(!sessions.iter().any(|s| s.session == session));
        }
        other => panic!("expected Sessions, got {other:?}"),
    }
}

#[tokio::test]
async fn rebind_moves_one_window_leaving_others() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;

    let mut a = connect(&socket).await;
    a.send(&ClientFrame::OpenSession {
        cwd: dir.path().display().to_string(),
        resume: ResumeMode::Conversation { conversation_id: 1 },
    })
    .await
    .unwrap();
    let first = match a.recv().await.unwrap() {
        ServerFrame::SessionOpened { session, .. } => session,
        other => panic!("expected SessionOpened, got {other:?}"),
    };

    let mut b = connect(&socket).await;
    b.send(&ClientFrame::OpenSession {
        cwd: dir.path().display().to_string(),
        resume: ResumeMode::Conversation { conversation_id: 1 },
    })
    .await
    .unwrap();
    let shared = match b.recv().await.unwrap() {
        ServerFrame::SessionOpened { session, .. } => session,
        other => panic!("expected SessionOpened, got {other:?}"),
    };
    assert_eq!(
        first, shared,
        "both windows share the live session for conversation 1"
    );

    b.send(&ClientFrame::Submit {
        session: shared,
        correlation: 1,
        op: goat_protocol::Op::Resume { conversation_id: 2 },
    })
    .await
    .unwrap();
    let moved = loop {
        match tokio::time::timeout(Duration::from_secs(5), b.recv()).await {
            Ok(Ok(ServerFrame::SessionOpened { session, .. })) => break session,
            Ok(Ok(_)) => {}
            other => panic!("expected SessionOpened, got {other:?}"),
        }
    };
    assert_ne!(moved, first, "rebound window is on a different session");

    let mut admin = connect(&socket).await;
    admin.send(&ClientFrame::ListSessions {}).await.unwrap();
    match admin.recv().await.unwrap() {
        ServerFrame::Sessions { sessions } => {
            assert!(
                sessions.iter().any(|s| s.session == first),
                "the original session stays alive for window a"
            );
        }
        other => panic!("expected Sessions, got {other:?}"),
    }
}

#[tokio::test]
async fn list_conversations_returns_a_frame() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;
    let mut conn = connect(&socket).await;

    conn.send(&ClientFrame::ListConversations {
        cwd: dir.path().display().to_string(),
    })
    .await
    .unwrap();
    match conn.recv().await.unwrap() {
        ServerFrame::Conversations { conversations } => {
            assert!(
                conversations.is_empty(),
                "no conversations exist yet in a fresh cwd"
            );
        }
        other => panic!("expected Conversations, got {other:?}"),
    }
}

#[tokio::test]
async fn daemon_intercepts_clear_as_rebind() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;
    let mut conn = connect(&socket).await;

    conn.send(&ClientFrame::OpenSession {
        cwd: dir.path().display().to_string(),
        resume: ResumeMode::Conversation { conversation_id: 1 },
    })
    .await
    .unwrap();
    let first = match conn.recv().await.unwrap() {
        ServerFrame::SessionOpened { session, .. } => session,
        other => panic!("expected SessionOpened, got {other:?}"),
    };

    conn.send(&ClientFrame::Submit {
        session: first,
        correlation: 1,
        op: goat_protocol::Op::Clear {},
    })
    .await
    .unwrap();

    let mut detached = false;
    let mut opened: Option<goat_wire::SessionId> = None;
    for _ in 0..20 {
        match tokio::time::timeout(Duration::from_secs(5), conn.recv()).await {
            Ok(Ok(ServerFrame::Detached { session })) => {
                assert_eq!(session, first);
                detached = true;
            }
            Ok(Ok(ServerFrame::SessionOpened { session, .. })) => {
                opened = Some(session);
                break;
            }
            Ok(Ok(_)) => {}
            Ok(Err(_)) | Err(_) => break,
        }
    }
    assert!(
        detached,
        "clear must detach the window from the old session"
    );
    let opened = opened.expect("clear must open a new session");
    assert_ne!(opened, first, "clear must rebind to a different session");
}
