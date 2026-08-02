mod codec;
mod protocol;
pub mod transport;

pub use codec::{WireConn, WireError};
pub use protocol::{
    BuildId, Busy, ClientFrame, ClientId, DeviceInfo, DirEntry, DirEntryKind, RateLimitEntry,
    ReloadFailure, ReloadReport, ResumeMode, ServerFrame, SessionId, SessionInfo, SessionLiveState,
    ThreadInfo, wire_fingerprint,
};

pub type ServerConn<S> = WireConn<S, ServerFrame, ClientFrame>;
pub type ClientConn<S> = WireConn<S, ClientFrame, ServerFrame>;

#[cfg(test)]
mod tests {
    use super::*;
    use goat_protocol::{Op, TaskId};

    #[test]
    fn fingerprint_is_sixteen_lowercase_hex() {
        let fp = wire_fingerprint();
        assert_eq!(fp.len(), 16, "{fp}");
        assert!(
            fp.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
        );
    }

    fn welcome() -> ServerFrame {
        ServerFrame::Welcome {
            wire: wire_fingerprint().to_owned(),
            build: Some(BuildId {
                path: "/bin/goat".to_owned(),
                len: 42,
                mtime: 7,
            }),
            busy: Busy::default(),
            version: "0.1.27".to_owned(),
            pid: 100,
            started_at: 5,
            ready: true,
            client_id: ClientId(7),
        }
    }

    #[tokio::test]
    async fn daemon_greets_before_the_client_speaks() {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let mut server: ServerConn<_> = WireConn::new(a);
        let mut client: ClientConn<_> = WireConn::new(b);

        server.send(&welcome()).await.unwrap();
        assert_eq!(client.recv().await.unwrap(), welcome());

        client.send(&ClientFrame::StopDaemon {}).await.unwrap();
        assert_eq!(server.recv().await.unwrap(), ClientFrame::StopDaemon {});
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
    fn stop_daemon_stays_decodable_from_its_frozen_bytes() {
        let frame: ClientFrame = serde_json::from_str(r#"{"type":"StopDaemon"}"#).unwrap();
        assert_eq!(frame, ClientFrame::StopDaemon {});
        assert_eq!(
            serde_json::to_string(&frame).unwrap(),
            r#"{"type":"StopDaemon"}"#
        );
    }

    #[test]
    fn welcome_stays_decodable_from_its_frozen_bytes() {
        let frozen = r#"{"type":"Welcome","wire":"deadbeefdeadbeef","build":{"path":"/bin/goat","len":42,"mtime":7},"busy":{"sessions":0,"turns":0},"version":"0.1.27","pid":100,"started_at":5,"ready":true,"client_id":"7"}"#;
        let frame: ServerFrame = serde_json::from_str(frozen).unwrap();
        let ServerFrame::Welcome {
            wire,
            build,
            busy,
            version,
            pid,
            started_at,
            ready,
            client_id,
        } = frame
        else {
            panic!("expected Welcome");
        };
        assert_eq!(wire, "deadbeefdeadbeef");
        assert_eq!(build.unwrap().len, 42);
        assert!(busy.is_idle());
        assert_eq!(version, "0.1.27");
        assert_eq!(pid, 100);
        assert_eq!(started_at, 5);
        assert!(ready);
        assert_eq!(client_id, ClientId(7));
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
