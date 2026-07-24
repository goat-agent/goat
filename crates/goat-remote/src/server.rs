use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::tungstenite::Message;

use crate::ca::Authority;
use crate::devices::{Device, Devices};
use crate::pairing::Pairing;
use crate::verify::DeviceVerifier;
use crate::{RemoteConfig, RemoteError, RemoteHandler, RemoteSink, RemoteStream};

pub struct RemoteServer {
    authority: Arc<Authority>,
    devices: Devices,
    pairing: Pairing,
    config: RemoteConfig,
}

impl RemoteServer {
    pub fn new(config: RemoteConfig, devices: Devices) -> Result<Self, RemoteError> {
        let authority = Authority::load_or_create(&config.remote_dir, &config.advertised)?;
        Ok(Self {
            authority: Arc::new(authority),
            devices,
            pairing: Pairing::default(),
            config,
        })
    }

    pub fn pairing(&self) -> Pairing {
        self.pairing.clone()
    }

    pub fn devices(&self) -> Devices {
        self.devices.clone()
    }

    pub fn server_fingerprint(&self) -> &str {
        self.authority.server_fingerprint()
    }

    pub fn advertised(&self) -> &[String] {
        &self.config.advertised
    }

    pub async fn run<H>(
        self,
        handler: Arc<H>,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> Result<(), RemoteError>
    where
        H: RemoteHandler,
    {
        let tls = self.build_tls_config()?;
        let acceptor = TlsAcceptor::from(Arc::new(tls));
        let devices_changed = self.devices.changed();
        let pairing_changed = self.pairing.changed();
        let server = Arc::new(self);

        loop {
            let dev_wait = devices_changed.notified();
            let pair_wait = pairing_changed.notified();
            tokio::pin!(dev_wait, pair_wait);
            dev_wait.as_mut().enable();
            pair_wait.as_mut().enable();

            let should_listen =
                !server.devices.is_empty().await || server.pairing.has_pending().await;
            if !should_listen {
                tokio::select! {
                    () = shutdown.cancelled() => break,
                    () = &mut dev_wait => continue,
                    () = &mut pair_wait => continue,
                }
            }

            let listener = match TcpListener::bind(server.config.bind).await {
                Ok(listener) => listener,
                Err(err) => {
                    tracing::warn!(%err, addr = %server.config.bind, "remote bind failed");
                    tokio::select! {
                        () = shutdown.cancelled() => break,
                        () = tokio::time::sleep(std::time::Duration::from_secs(5)) => continue,
                    }
                }
            };
            tracing::info!(addr = %server.config.bind, "remote listener up");

            loop {
                let wind_down =
                    !server.devices.is_empty().await || server.pairing.has_pending().await;
                if !wind_down {
                    tracing::info!("no devices or pending pairings; remote listener down");
                    break;
                }
                tokio::select! {
                    () = shutdown.cancelled() => return Ok(()),
                    () = devices_changed.notified() => {}
                    () = pairing_changed.notified() => {}
                    () = tokio::time::sleep(std::time::Duration::from_secs(30)) => {}
                    accepted = listener.accept() => {
                        let Ok((tcp, _peer)) = accepted else { continue };
                        let acceptor = acceptor.clone();
                        let server = server.clone();
                        let handler = handler.clone();
                        tokio::spawn(async move {
                            if let Err(err) = server.serve_one(acceptor, tcp, handler).await {
                                tracing::debug!(%err, "remote connection ended");
                            }
                        });
                    }
                }
            }
        }
        Ok(())
    }

    fn build_tls_config(&self) -> Result<rustls::ServerConfig, RemoteError> {
        let server_certs = load_certs(self.authority.server_cert_pem())?;
        let server_key = load_key(self.authority.server_key_pem())?;
        let ca_certs = load_certs(self.authority.ca_cert_pem())?;

        let mut roots = rustls::RootCertStore::empty();
        for cert in ca_certs {
            roots.add(cert).map_err(RemoteError::Tls)?;
        }
        let chain = WebPkiClientVerifier::builder(Arc::new(roots))
            .build()
            .map_err(|e| RemoteError::Bind(e.to_string()))?;
        let verifier = Arc::new(DeviceVerifier::new(chain, self.devices.allowlist()));

        let config = rustls::ServerConfig::builder()
            .with_client_cert_verifier(verifier)
            .with_single_cert(server_certs, server_key)
            .map_err(RemoteError::Tls)?;
        Ok(config)
    }

    async fn serve_one<H>(
        self: Arc<Self>,
        acceptor: TlsAcceptor,
        tcp: tokio::net::TcpStream,
        handler: Arc<H>,
    ) -> Result<(), RemoteError>
    where
        H: RemoteHandler,
    {
        let mut tls = acceptor.accept(tcp).await?;
        let device_fp = tls
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|c| c.first())
            .map(|cert| crate::ca::fingerprint_der(cert.as_ref()));

        let request = read_request_head(&mut tls).await?;
        match request.route() {
            Route::Pair => self.handle_pair(tls, request).await,
            Route::Ws => self.handle_ws(tls, request, device_fp, handler).await,
            Route::Unknown => {
                write_simple(tls, "404 Not Found").await?;
                Ok(())
            }
        }
    }

    async fn handle_pair<S>(&self, mut tls: S, request: RequestHead) -> Result<(), RemoteError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let body = read_body(&mut tls, &request).await?;
        let req: PairRequest = match serde_json::from_slice(&body) {
            Ok(req) => req,
            Err(_) => return write_http(&mut tls, 400, b"{\"error\":\"bad request\"}").await,
        };
        let Some(claim) = self.pairing.claim(&req.code).await else {
            return write_http(&mut tls, 403, b"{\"error\":\"invalid or expired code\"}").await;
        };
        let Ok(signed) = self.authority.sign_device_csr(&req.csr_pem) else {
            return write_http(&mut tls, 400, b"{\"error\":\"bad csr\"}").await;
        };
        let device = Device {
            id: short_id(&signed.fingerprint),
            label: claim.label,
            fingerprint: signed.fingerprint.clone(),
            paired_at: now_ms(),
        };
        if self.devices.enroll(device).await.is_err() {
            return write_http(&mut tls, 500, b"{\"error\":\"enroll failed\"}").await;
        }
        let response = PairResponse {
            device_cert_pem: signed.cert_pem,
            ca_cert_pem: self.authority.ca_cert_pem().to_owned(),
        };
        let bytes = serde_json::to_vec(&response)?;
        write_http(&mut tls, 200, &bytes).await
    }

    async fn handle_ws<S, H>(
        &self,
        mut tls: S,
        request: RequestHead,
        device_fp: Option<String>,
        handler: Arc<H>,
    ) -> Result<(), RemoteError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
        H: RemoteHandler,
    {
        let Some(fingerprint) = device_fp else {
            return write_http(
                &mut tls,
                403,
                b"{\"error\":\"client certificate required\"}",
            )
            .await;
        };
        let Some(device) = self.devices.find_by_fingerprint(&fingerprint).await else {
            return write_http(&mut tls, 403, b"{\"error\":\"unknown device\"}").await;
        };
        let Some(key) = request.ws_key else {
            return write_http(&mut tls, 400, b"{\"error\":\"missing websocket key\"}").await;
        };
        let accept = ws_accept_key(&key);
        let upgrade = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
        );
        {
            use tokio::io::AsyncWriteExt;
            tls.write_all(upgrade.as_bytes()).await?;
            tls.flush().await?;
        }
        let mut wsconfig = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default();
        wsconfig.max_message_size = Some(MAX_WS_MESSAGE);
        wsconfig.max_frame_size = Some(MAX_WS_MESSAGE);
        let ws = tokio_tungstenite::WebSocketStream::from_raw_socket(
            tls,
            tokio_tungstenite::tungstenite::protocol::Role::Server,
            Some(wsconfig),
        )
        .await;
        let (sink, stream) = frame_adapter(ws);
        handler.handle(device, sink, stream).await;
        Ok(())
    }
}

const MAX_WS_MESSAGE: usize = 8 * 1024 * 1024;

fn frame_adapter<S>(ws: tokio_tungstenite::WebSocketStream<S>) -> (RemoteSink, RemoteStream)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    use goat_wire::{ClientFrame, ServerFrame, WireError};
    let (ws_sink, ws_stream) = ws.split();
    let sink = ws_sink
        .sink_map_err(|_| WireError::Closed)
        .with(|frame: ServerFrame| async move {
            let text = serde_json::to_string(&frame).map_err(WireError::Encode)?;
            Ok::<_, WireError>(Message::Text(text.into()))
        });
    let stream = ws_stream
        .filter_map(|item| async move {
            match item {
                Ok(Message::Text(text)) => {
                    Some(serde_json::from_str::<ClientFrame>(&text).map_err(WireError::Decode))
                }
                Ok(Message::Binary(bytes)) => {
                    Some(serde_json::from_slice::<ClientFrame>(&bytes).map_err(WireError::Decode))
                }
                Ok(Message::Close(_)) | Err(_) => Some(Err(WireError::Closed)),
                Ok(_) => None,
            }
        })
        .boxed();
    (Box::pin(sink), stream)
}

#[derive(serde::Deserialize)]
struct PairRequest {
    code: String,
    csr_pem: String,
}

#[derive(serde::Serialize)]
struct PairResponse {
    device_cert_pem: String,
    ca_cert_pem: String,
}

enum Route {
    Pair,
    Ws,
    Unknown,
}

struct RequestHead {
    method: String,
    path: String,
    content_length: usize,
    ws_key: Option<String>,
}

impl RequestHead {
    fn route(&self) -> Route {
        match (self.method.as_str(), self.path.as_str()) {
            ("POST", "/pair") => Route::Pair,
            ("GET", "/ws") => Route::Ws,
            _ => Route::Unknown,
        }
    }
}

async fn read_request_head<S>(reader: &mut S) -> Result<RequestHead, RemoteError>
where
    S: AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    loop {
        let n = reader.read(&mut byte).await?;
        if n == 0 {
            return Err(RemoteError::Bind(
                "connection closed before head".to_owned(),
            ));
        }
        buf.push(byte[0]);
        if buf.len() >= 4 && &buf[buf.len() - 4..] == b"\r\n\r\n" {
            break;
        }
        if buf.len() > 16 * 1024 {
            return Err(RemoteError::Bind("request head too large".to_owned()));
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or_default().to_owned();
    let mut content_length = 0usize;
    let mut ws_key = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim();
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            } else if name.eq_ignore_ascii_case("sec-websocket-key") {
                ws_key = Some(value.trim().to_owned());
            }
        }
    }
    Ok(RequestHead {
        method,
        path,
        content_length,
        ws_key,
    })
}

async fn read_body<S>(reader: &mut S, request: &RequestHead) -> Result<Vec<u8>, RemoteError>
where
    S: AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    if request.content_length == 0 || request.content_length > MAX_WS_MESSAGE {
        return Ok(Vec::new());
    }
    let mut body = vec![0u8; request.content_length];
    reader.read_exact(&mut body).await?;
    Ok(body)
}

async fn write_http<S>(tls: &mut S, status: u16, body: &[u8]) -> Result<(), RemoteError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt;
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        500 => "Internal Server Error",
        _ => "Error",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    tls.write_all(header.as_bytes()).await?;
    tls.write_all(body).await?;
    tls.flush().await?;
    Ok(())
}

async fn write_simple<S>(mut tls: S, status: &str) -> Result<(), RemoteError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt;
    let response = format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    tls.write_all(response.as_bytes()).await?;
    tls.flush().await?;
    Ok(())
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

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn ws_accept_key(key: &str) -> String {
    tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes())
}

fn short_id(fingerprint: &str) -> String {
    fingerprint.chars().take(12).collect()
}
