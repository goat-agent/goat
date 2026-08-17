use std::path::PathBuf;
use std::time::Duration;

use goat_api::{ResumeMode, SessionOpen, SessionOpenParams};
use goat_client::Link;
use goat_remote::client::DeviceCredentials;
use goat_wire::transport;

async fn start_remote_daemon(dir: &std::path::Path, port: u16) -> PathBuf {
    let socket = dir.join("d.sock");
    let cfg = goat_daemon::DaemonConfig {
        socket_path: socket.clone(),
        lock_path: dir.join("daemon.lock"),
        auth_path: dir.join("auth.json"),
        config_json: dir.join("config.json"),
        db_path: dir.join("store.sqlite"),
        remote: Some(goat_daemon::RemoteSettings {
            remote_dir: dir.join("remote"),
            bind: format!("127.0.0.1:{port}").parse().unwrap(),
            advertised: vec!["127.0.0.1".to_owned()],
        }),
    };
    tokio::spawn(async move {
        let _ = goat_daemon::serve(cfg).await;
    });
    for _ in 0..100 {
        if transport::connect(&socket).await.is_ok() {
            return socket;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("daemon did not start");
}

fn local(socket: &std::path::Path) -> Link {
    Link::local(socket.to_path_buf(), PathBuf::new())
}

async fn mint_code(socket: &std::path::Path) -> (String, String) {
    let info = goat_client::pair_device(&local(socket), "phone".to_owned())
        .await
        .expect("the daemon mints a pairing code");
    (info.code, info.server_fingerprint)
}

async fn settle(
    host: &str,
    code: &str,
    fingerprint: &str,
) -> Result<goat_remote::client::Enrollment, goat_remote::RemoteError> {
    let mut last = None;
    for _ in 0..50 {
        match goat_remote::client::enroll(host, code, fingerprint).await {
            Err(goat_remote::RemoteError::Io(err))
                if err.kind() == std::io::ErrorKind::ConnectionRefused =>
            {
                last = Some(Err(goat_remote::RemoteError::Io(err)));
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            settled => return settled,
        }
    }
    last.unwrap_or_else(|| panic!("remote listener never came up"))
}

async fn enroll(host: &str, fingerprint: &str, code: &str) -> DeviceCredentials {
    match settle(host, code, fingerprint).await {
        Ok(enrollment) => DeviceCredentials {
            key_pem: enrollment.key_pem,
            cert_pem: enrollment.cert_pem,
            ca_cert_pem: enrollment.ca_cert_pem,
            server_fingerprint: fingerprint.to_owned(),
        },
        Err(err) => panic!("pairing failed: {err}"),
    }
}

fn install_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[tokio::test]
async fn remote_pair_and_open_session_over_mtls() {
    install_provider();
    let dir = tempfile::tempdir().unwrap();
    let port = 47318;
    let host = format!("127.0.0.1:{port}");
    let socket = start_remote_daemon(dir.path(), port).await;

    let (code, fingerprint) = mint_code(&socket).await;
    let credentials = enroll(&host, &fingerprint, &code).await;

    let link = Link::remote("desk".to_owned(), host, credentials);
    let session = goat_client::open_api(&link, "remote-e2e")
        .await
        .expect("the daemon greets over mtls");
    assert!(session.daemon.compatible());

    let opened = session
        .api
        .call::<SessionOpen>(SessionOpenParams {
            cwd: dir.path().display().to_string(),
            resume: ResumeMode::New {},
        })
        .await
        .expect("a remote client opens a session");
    let expected = std::fs::canonicalize(dir.path()).unwrap();
    assert_eq!(
        opened.cwd,
        expected.display().to_string(),
        "the daemon reports the cwd it normalized, not the one the client sent"
    );
}

#[tokio::test]
async fn a_remote_client_is_not_granted_the_admin_routes() {
    install_provider();
    let dir = tempfile::tempdir().unwrap();
    let port = 47322;
    let host = format!("127.0.0.1:{port}");
    let socket = start_remote_daemon(dir.path(), port).await;

    let (code, fingerprint) = mint_code(&socket).await;
    let credentials = enroll(&host, &fingerprint, &code).await;

    let link = Link::remote("desk".to_owned(), host, credentials);
    let session = goat_client::open_api(&link, "remote-e2e")
        .await
        .expect("the daemon greets over mtls");

    assert!(session.daemon.speaks("session.open", 1));
    assert!(
        !session.daemon.speaks("admin.daemon_stop", 1),
        "a paired device must not be offered the admin routes at all"
    );
    assert!(!session.daemon.speaks("admin.device_revoke", 1));
}

#[tokio::test]
async fn revoked_device_cannot_reconnect() {
    install_provider();
    let dir = tempfile::tempdir().unwrap();
    let port = 47319;
    let host = format!("127.0.0.1:{port}");
    let socket = start_remote_daemon(dir.path(), port).await;

    let (code, fingerprint) = mint_code(&socket).await;
    let credentials = enroll(&host, &fingerprint, &code).await;

    let devices = goat_client::list_devices(&local(&socket))
        .await
        .expect("devices list");
    let device_id = devices[0].id.clone();
    assert!(
        goat_client::revoke_device(&local(&socket), device_id)
            .await
            .expect("revoke answers"),
        "the paired device is revoked"
    );

    let refused = goat_remote::client::connect::<
        goat_wire::envelope::Frame,
        goat_wire::envelope::Frame,
    >(&host, &credentials)
    .await;
    assert!(refused.is_err(), "a revoked device must not connect");
}

#[tokio::test]
async fn a_bad_pairing_code_is_refused() {
    install_provider();
    let dir = tempfile::tempdir().unwrap();
    let port = 47320;
    let host = format!("127.0.0.1:{port}");
    let socket = start_remote_daemon(dir.path(), port).await;

    let (_code, fingerprint) = mint_code(&socket).await;
    match settle(&host, "not-a-real-code", &fingerprint).await {
        Err(goat_remote::RemoteError::Pairing(message)) => {
            assert!(message.contains("invalid or expired code"), "got {message}");
        }
        other => panic!("expected a pairing refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn a_wrong_fingerprint_is_refused() {
    install_provider();
    let dir = tempfile::tempdir().unwrap();
    let port = 47321;
    let host = format!("127.0.0.1:{port}");
    let socket = start_remote_daemon(dir.path(), port).await;

    let (code, _fingerprint) = mint_code(&socket).await;
    let wrong = "0".repeat(64);
    match settle(&host, &code, &wrong).await {
        Err(err) => assert!(
            err.to_string().contains("pinned fingerprint"),
            "expected the pin to reject the server, got {err}"
        ),
        Ok(_) => panic!("a mismatched server fingerprint must not pair"),
    }
}
