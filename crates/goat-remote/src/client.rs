use std::pin::Pin;
use std::sync::Arc;

use futures::{Sink, Stream};
use goat_wire::{ClientFrame, ServerFrame, WireError};
use rcgen::{CertificateParams, DnType, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use crate::RemoteError;
use crate::verify::PinnedServer;
use crate::ws;

pub type DeviceSink = Pin<Box<dyn Sink<ClientFrame, Error = WireError> + Send>>;
pub type DeviceStream = Pin<Box<dyn Stream<Item = Result<ServerFrame, WireError>> + Send>>;

#[derive(Debug, Clone)]
pub struct DeviceCredentials {
    pub key_pem: String,
    pub cert_pem: String,
    pub ca_cert_pem: String,
    pub server_fingerprint: String,
}

#[derive(Debug, Clone)]
pub struct Enrollment {
    pub key_pem: String,
    pub cert_pem: String,
    pub ca_cert_pem: String,
}

const MAX_HEAD: usize = 16 * 1024;
const MAX_BODY: usize = 256 * 1024;

pub async fn enroll(
    host: &str,
    code: &str,
    server_fingerprint: &str,
) -> Result<Enrollment, RemoteError> {
    let (key_pem, csr_pem) = generate_csr()?;
    let mut tls = dial(host, server_fingerprint, None).await?;

    let body = serde_json::to_vec(&PairRequest {
        code: code.to_owned(),
        csr_pem,
    })?;
    let head = format!(
        "POST /pair HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    tls.write_all(head.as_bytes()).await?;
    tls.write_all(&body).await?;
    tls.flush().await?;

    let head = read_head(&mut tls).await?;
    let body = read_body(&mut tls, head.content_length).await?;
    if head.status != 200 {
        return Err(RemoteError::Pairing(reason(head.status, &body)));
    }
    let parsed: PairResponse = serde_json::from_slice(&body)?;
    Ok(Enrollment {
        key_pem,
        cert_pem: parsed.device_cert_pem,
        ca_cert_pem: parsed.ca_cert_pem,
    })
}

pub async fn connect(
    host: &str,
    credentials: &DeviceCredentials,
) -> Result<(DeviceSink, DeviceStream), RemoteError> {
    let certs = load_certs(&credentials.cert_pem)?;
    let key = load_key(&credentials.key_pem)?;
    let tls = dial(host, &credentials.server_fingerprint, Some((certs, key))).await?;
    let (ws, _response) = tokio_tungstenite::client_async_with_config(
        format!("ws://{host}/ws"),
        tls,
        Some(ws::config()),
    )
    .await
    .map_err(|err| RemoteError::Handshake(err.to_string()))?;
    Ok(ws::adapt::<_, ClientFrame, ServerFrame>(ws))
}

async fn dial(
    host: &str,
    fingerprint: &str,
    client_auth: Option<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)>,
) -> Result<TlsStream<TcpStream>, RemoteError> {
    let provider = provider();
    let verifier = Arc::new(PinnedServer::new(fingerprint.to_owned(), provider.clone()));
    let builder = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(RemoteError::Tls)?
        .dangerous()
        .with_custom_certificate_verifier(verifier);
    let config = match client_auth {
        Some((certs, key)) => builder
            .with_client_auth_cert(certs, key)
            .map_err(RemoteError::Tls)?,
        None => builder.with_no_client_auth(),
    };
    let connector = TlsConnector::from(Arc::new(config));
    let tcp = TcpStream::connect(host).await?;
    Ok(connector.connect(server_name(host), tcp).await?)
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    rustls::crypto::CryptoProvider::get_default()
        .cloned()
        .unwrap_or_else(|| Arc::new(rustls::crypto::ring::default_provider()))
}

fn server_name(host: &str) -> ServerName<'static> {
    let name = host.rsplit_once(':').map_or(host, |(name, _)| name);
    ServerName::try_from(name.to_owned())
        .unwrap_or_else(|_| ServerName::try_from("goat").expect("goat is a valid dns name"))
}

fn generate_csr() -> Result<(String, String), RemoteError> {
    let key = KeyPair::generate()?;
    let mut params = CertificateParams::new(vec!["device".to_owned()])?;
    params
        .distinguished_name
        .push(DnType::CommonName, "goat remote device");
    let csr = params.serialize_request(&key)?.pem()?;
    Ok((key.serialize_pem(), csr))
}

struct ResponseHead {
    status: u16,
    content_length: usize,
}

async fn read_head(tls: &mut TlsStream<TcpStream>) -> Result<ResponseHead, RemoteError> {
    let mut raw = Vec::with_capacity(512);
    let mut byte = [0u8; 1];
    loop {
        if tls.read(&mut byte).await? == 0 {
            return Err(RemoteError::Pairing("connection closed early".to_owned()));
        }
        raw.push(byte[0]);
        if raw.ends_with(b"\r\n\r\n") {
            break;
        }
        if raw.len() > MAX_HEAD {
            return Err(RemoteError::Pairing("response head too large".to_owned()));
        }
    }
    let text = String::from_utf8_lossy(&raw);
    parse_head(&text)
}

fn parse_head(raw: &str) -> Result<ResponseHead, RemoteError> {
    let mut lines = raw.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| RemoteError::Pairing("malformed status line".to_owned()))?;
    let mut content_length = 0usize;
    for line in lines {
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    Ok(ResponseHead {
        status,
        content_length,
    })
}

async fn read_body(tls: &mut TlsStream<TcpStream>, length: usize) -> Result<Vec<u8>, RemoteError> {
    if length == 0 {
        return Ok(Vec::new());
    }
    if length > MAX_BODY {
        return Err(RemoteError::Pairing("response too large".to_owned()));
    }
    let mut body = vec![0u8; length];
    tls.read_exact(&mut body).await?;
    Ok(body)
}

fn reason(status: u16, body: &[u8]) -> String {
    #[derive(serde::Deserialize)]
    struct Failure {
        error: String,
    }
    serde_json::from_slice::<Failure>(body)
        .map_or_else(|_| format!("server returned {status}"), |it| it.error)
}

fn load_certs(pem: &str) -> Result<Vec<CertificateDer<'static>>, RemoteError> {
    let mut reader = pem.as_bytes();
    rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| RemoteError::Pem)
}

fn load_key(pem: &str) -> Result<PrivateKeyDer<'static>, RemoteError> {
    let mut reader = pem.as_bytes();
    rustls_pemfile::private_key(&mut reader)
        .map_err(|_| RemoteError::Pem)?
        .ok_or(RemoteError::Pem)
}

#[derive(serde::Serialize)]
struct PairRequest {
    code: String,
    csr_pem: String,
}

#[derive(serde::Deserialize)]
struct PairResponse {
    device_cert_pem: String,
    ca_cert_pem: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csr_carries_a_usable_key() {
        let (key_pem, csr_pem) = generate_csr().unwrap();
        assert!(key_pem.contains("PRIVATE KEY"));
        assert!(csr_pem.contains("CERTIFICATE REQUEST"));
        assert!(load_key(&key_pem).is_ok());
    }

    #[test]
    fn head_carries_status_and_length() {
        let head = parse_head("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n").unwrap();
        assert_eq!(head.status, 200);
        assert_eq!(head.content_length, 2);
    }

    #[test]
    fn a_head_without_content_length_reads_no_body() {
        let head = parse_head("HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n").unwrap();
        assert_eq!(head.status, 403);
        assert_eq!(head.content_length, 0);
    }

    #[test]
    fn failure_body_becomes_the_reason() {
        assert_eq!(
            reason(403, b"{\"error\":\"invalid or expired code\"}"),
            "invalid or expired code"
        );
    }

    #[test]
    fn unparsable_failure_body_falls_back_to_the_status() {
        assert_eq!(reason(500, b"not json"), "server returned 500");
    }

    #[test]
    fn server_name_drops_the_port() {
        assert_eq!(
            server_name("example.com:4317"),
            ServerName::try_from("example.com").unwrap()
        );
    }

    #[test]
    fn a_malformed_status_line_is_rejected() {
        assert!(parse_head("garbage\r\n\r\n").is_err());
    }
}
