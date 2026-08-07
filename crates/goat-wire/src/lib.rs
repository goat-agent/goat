mod codec;
mod protocol;
pub mod transport;

pub use codec::{WireConn, WireError};
pub use protocol::{
    BuildId, Busy, ClientFrame, ClientId, DeviceInfo, DirEntry, DirEntryKind, ModeEntry,
    RateLimitEntry, ReloadFailure, ReloadReport, ResumeMode, RetryEntry, ServerFrame, SessionId,
    SessionInfo, SessionLiveState, ThreadInfo, UsageEntry, wire_fingerprint,
};

pub type ServerConn<S> = WireConn<S, ServerFrame, ClientFrame>;
pub type ClientConn<S> = WireConn<S, ClientFrame, ServerFrame>;

#[cfg(test)]
mod tests {
    use super::*;
    use goat_protocol::{Op, TaskId};

    #[tokio::test]
    async fn client_server_roundtrip_over_duplex() {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let mut server: ServerConn<_> = WireConn::new(a);
        let mut client: ClientConn<_> = WireConn::new(b);

        server
            .send(&ServerFrame::Welcome {
                wire: wire_fingerprint().to_owned(),
                build: None,
                busy: Busy::default(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                pid: 7,
                started_at: 0,
                ready: true,
                client_id: ClientId(7),
            })
            .await
            .unwrap();
        let got = client.recv().await.unwrap();
        assert_eq!(
            got,
            ServerFrame::Welcome {
                wire: wire_fingerprint().to_owned(),
                build: None,
                busy: Busy::default(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                pid: 7,
                started_at: 0,
                ready: true,
                client_id: ClientId(7),
            }
        );
    }

    #[tokio::test]
    async fn submit_op_frame_roundtrips() {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let mut server: ServerConn<_> = WireConn::new(a);
        let mut client: ClientConn<_> = WireConn::new(b);
        let frame = ClientFrame::Submit {
            session: SessionId(1),
            correlation: 42,
            op: Op::SubmitMessage {
                id: TaskId(0),
                text: "hi".to_owned(),
                display: None,
                attachments: Vec::new(),
            },
        };
        client.send(&frame).await.unwrap();
        assert_eq!(server.recv().await.unwrap(), frame);
    }

    #[tokio::test]
    async fn directory_frames_roundtrip() {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let mut server: ServerConn<_> = WireConn::new(a);
        let mut client: ClientConn<_> = WireConn::new(b);

        let request = ClientFrame::ListDirectory {
            path: "/home/me".to_owned(),
            recursive: false,
        };
        client.send(&request).await.unwrap();
        assert_eq!(server.recv().await.unwrap(), request);

        let response = ServerFrame::Directory {
            path: "/home/me".to_owned(),
            children: vec![
                DirEntry {
                    name: "src".to_owned(),
                    kind: DirEntryKind::Directory {},
                },
                DirEntry {
                    name: "main.rs".to_owned(),
                    kind: DirEntryKind::File {},
                },
            ],
        };
        server.send(&response).await.unwrap();
        assert_eq!(client.recv().await.unwrap(), response);
    }

    #[tokio::test]
    async fn sessions_and_device_frames_roundtrip() {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let mut server: ServerConn<_> = WireConn::new(a);
        let mut client: ClientConn<_> = WireConn::new(b);

        let sessions = ServerFrame::Sessions {
            sessions: Vec::new(),
        };
        server.send(&sessions).await.unwrap();
        assert_eq!(client.recv().await.unwrap(), sessions);

        let pair = ClientFrame::PairDevice {
            label: "phone".to_owned(),
        };
        client.send(&pair).await.unwrap();
        assert_eq!(server.recv().await.unwrap(), pair);

        let devices = ServerFrame::Devices {
            devices: vec![DeviceInfo {
                id: "abc".to_owned(),
                label: "phone".to_owned(),
                paired_at: 5,
            }],
        };
        server.send(&devices).await.unwrap();
        assert_eq!(client.recv().await.unwrap(), devices);
    }

    #[test]
    fn client_frame_list_sessions_serializes_as_type_object() {
        let json = serde_json::to_string(&ClientFrame::ListSessions {}).unwrap();
        assert_eq!(json, r#"{"type":"ListSessions"}"#);
        let back: ClientFrame = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ClientFrame::ListSessions {});
    }

    #[test]
    fn client_frame_open_session_serializes_flat() {
        let frame = ClientFrame::OpenSession {
            cwd: "/tmp".to_owned(),
            resume: ResumeMode::New {},
        };
        let json = serde_json::to_string(&frame).unwrap();
        assert!(json.contains(r#""type":"OpenSession""#));
        assert!(!json.contains(r#"{"OpenSession":"#));
        let back: ClientFrame = serde_json::from_str(&json).unwrap();
        assert_eq!(back, frame);
    }

    #[test]
    fn server_frame_event_nests_event_type_separately() {
        use goat_protocol::Event;
        let frame = ServerFrame::Event {
            session: SessionId(1),
            seq: 0,
            event: Event::TextDelta {
                id: goat_protocol::TaskId(1),
                chunk: "x".to_owned(),
            },
        };
        let json = serde_json::to_string(&frame).unwrap();
        assert!(json.contains(r#""type":"Event""#));
        assert!(json.contains(r#""type":"TextDelta""#));
        let back: ServerFrame = serde_json::from_str(&json).unwrap();
        assert_eq!(back, frame);
    }
}
