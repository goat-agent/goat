use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use goat_wire::transport::{self, Stream};
use goat_wire::{ClientConn, ClientFrame, PROTOCOL_VERSION, ResumeMode, ServerFrame, WireConn};
use rustls::pki_types::{CertificateDer, ServerName};
use tokio_rustls::TlsConnector;
use tokio_tungstenite::tungstenite::Message;

async fn start_remote_daemon(dir: &std::path::Path, port: u16) -> PathBuf {
    let socket = dir.join("d.sock");
    let cfg = goat_daemon::DaemonConfig {
        socket_path: socket.clone(),
        auth_path: dir.join("auth.json"),
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

async fn local_conn(socket: &std::path::Path) -> ClientConn<Stream> {
    let stream = transport::connect(socket).await.unwrap();
    let mut conn: ClientConn<Stream> = WireConn::new(stream);
    conn.send(&ClientFrame::Hello {
        version: PROTOCOL_VERSION,
    })
    .await
    .unwrap();
    match conn.recv().await.unwrap() {
        ServerFrame::Welcome { .. } => {}
        other => panic!("expected Welcome, got {other:?}"),
    }
    conn
}

async fn mint_code(socket: &std::path::Path) -> (String, String) {
    let mut conn = local_conn(socket).await;
    conn.send(&ClientFrame::PairDevice {
        label: "phone".to_owned(),
    })
    .await
    .unwrap();
    match conn.recv().await.unwrap() {
        ServerFrame::PairingCode {
            code,
            server_fingerprint,
            ..
        } => (code, server_fingerprint),
        other => panic!("expected PairingCode, got {other:?}"),
    }
}

fn make_csr() -> (rcgen::KeyPair, String) {
    let key = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::new(vec!["device".to_owned()]).unwrap();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "device");
    let csr = params.serialize_request(&key).unwrap().pem().unwrap();
    (key, csr)
}

#[derive(Debug)]
struct PinnedVerifier {
    fingerprint: String,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl rustls::client::danger::ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        let got = goat_remote::fingerprint_der(end_entity.as_ref());
        if got == self.fingerprint {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("server pin mismatch".to_owned()))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn load_certs(pem: &str) -> Vec<CertificateDer<'static>> {
    let mut reader = pem.as_bytes();
    rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

async fn tls_connect(
    port: u16,
    fingerprint: &str,
    client_cert: Option<(
        Vec<CertificateDer<'static>>,
        rustls::pki_types::PrivateKeyDer<'static>,
    )>,
) -> tokio_rustls::client::TlsStream<tokio::net::TcpStream> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(PinnedVerifier {
        fingerprint: fingerprint.to_owned(),
        provider: provider.clone(),
    });
    let builder = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier);
    let config = match client_cert {
        Some((certs, key)) => builder.with_client_auth_cert(certs, key).unwrap(),
        None => builder.with_no_client_auth(),
    };
    let connector = TlsConnector::from(Arc::new(config));
    let domain = ServerName::try_from("127.0.0.1").unwrap();
    for _ in 0..50 {
        if let Ok(tcp) = tokio::net::TcpStream::connect(("127.0.0.1", port)).await
            && let Ok(tls) = connector.connect(domain.clone(), tcp).await
        {
            return tls;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("could not establish TLS to remote listener");
}

async fn pair_device(port: u16, fingerprint: &str, code: &str) -> (String, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let tls = tls_connect(port, fingerprint, None).await;
    let (key, csr) = make_csr();
    let body = serde_json::json!({ "code": code, "csr_pem": csr }).to_string();
    let request = format!(
        "POST /pair HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let mut tls = tls;
    tls.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match tls.read(&mut chunk).await {
            Ok(n) if n > 0 => response.extend_from_slice(&chunk[..n]),
            _ => break,
        }
    }
    let text = String::from_utf8_lossy(&response);
    let body = text.split("\r\n\r\n").nth(1).unwrap_or_default();
    let parsed: serde_json::Value = serde_json::from_str(body).unwrap();
    let device_cert = parsed["device_cert_pem"].as_str().unwrap().to_owned();
    (key.serialize_pem(), device_cert)
}

fn install_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[tokio::test]
async fn remote_pair_and_open_session_over_mtls() {
    install_provider();
    let dir = tempfile::tempdir().unwrap();
    let port = 47318;
    let socket = start_remote_daemon(dir.path(), port).await;

    let (code, fingerprint) = mint_code(&socket).await;
    let (key_pem, device_cert_pem) = pair_device(port, &fingerprint, &code).await;

    let certs = load_certs(&device_cert_pem);
    let key = rustls_pemfile::private_key(&mut key_pem.as_bytes())
        .unwrap()
        .unwrap();
    let tls = tls_connect(port, &fingerprint, Some((certs, key))).await;

    let (mut ws, _resp) = tokio_tungstenite::client_async("ws://127.0.0.1/ws", tls)
        .await
        .expect("ws upgrade");

    send_frame(
        &mut ws,
        &ClientFrame::Hello {
            version: PROTOCOL_VERSION,
        },
    )
    .await;
    match recv_frame(&mut ws).await {
        ServerFrame::Welcome { version, .. } => assert_eq!(version, PROTOCOL_VERSION),
        other => panic!("expected Welcome, got {other:?}"),
    }

    send_frame(
        &mut ws,
        &ClientFrame::OpenSession {
            cwd: dir.path().display().to_string(),
            resume: ResumeMode::New {},
        },
    )
    .await;
    match recv_frame(&mut ws).await {
        ServerFrame::SessionOpened { .. } => {}
        other => panic!("expected SessionOpened, got {other:?}"),
    }
}

#[tokio::test]
async fn revoked_device_cannot_reconnect() {
    install_provider();
    let dir = tempfile::tempdir().unwrap();
    let port = 47319;
    let socket = start_remote_daemon(dir.path(), port).await;

    let (code, fingerprint) = mint_code(&socket).await;
    let (key_pem, device_cert_pem) = pair_device(port, &fingerprint, &code).await;

    let device_id = {
        let mut conn = local_conn(&socket).await;
        conn.send(&ClientFrame::ListDevices {}).await.unwrap();
        match conn.recv().await.unwrap() {
            ServerFrame::Devices { devices } => devices[0].id.clone(),
            other => panic!("expected Devices, got {other:?}"),
        }
    };
    {
        let mut conn = local_conn(&socket).await;
        conn.send(&ClientFrame::RevokeDevice {
            device: device_id.clone(),
        })
        .await
        .unwrap();
        match conn.recv().await.unwrap() {
            ServerFrame::DeviceRevoked { ok } => assert!(ok),
            other => panic!("expected DeviceRevoked, got {other:?}"),
        }
    }

    let certs = load_certs(&device_cert_pem);
    let key = rustls_pemfile::private_key(&mut key_pem.as_bytes())
        .unwrap()
        .unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(PinnedVerifier {
        fingerprint: fingerprint.clone(),
        provider: provider.clone(),
    });
    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(certs, key)
        .unwrap();
    let connector = TlsConnector::from(Arc::new(config));
    let domain = ServerName::try_from("127.0.0.1").unwrap();
    let outcome: Result<(), std::io::Error> = async {
        let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        let tls = connector.connect(domain, tcp).await?;
        let (mut ws, _) = tokio_tungstenite::client_async("ws://127.0.0.1/ws", tls)
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let hello = serde_json::to_string(&ClientFrame::Hello {
            version: PROTOCOL_VERSION,
        })
        .unwrap();
        ws.send(Message::Text(hello.into()))
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        match ws.next().await {
            Some(Ok(_)) => Ok(()),
            _ => Err(std::io::Error::other("closed")),
        }
    }
    .await;
    assert!(
        outcome.is_err(),
        "revoked device must be refused before any frame exchange"
    );
}

async fn send_frame<S>(ws: &mut tokio_tungstenite::WebSocketStream<S>, frame: &ClientFrame)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let text = serde_json::to_string(frame).unwrap();
    ws.send(Message::Text(text.into())).await.unwrap();
}

async fn recv_frame<S>(ws: &mut tokio_tungstenite::WebSocketStream<S>) -> ServerFrame
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        match ws.next().await.expect("ws closed").unwrap() {
            Message::Text(text) => return serde_json::from_str(&text).unwrap(),
            Message::Binary(bytes) => return serde_json::from_slice(&bytes).unwrap(),
            _ => {}
        }
    }
}
