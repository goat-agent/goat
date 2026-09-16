use std::io::Read;

use goat_provider::{Model, StreamError};

use crate::proto::{self, Reader, Value, as_str, as_u64};

pub const DEFAULT_BASE_URL: &str = "https://server.codeium.com";
pub const DEVIN_API_URL: &str = "https://api.devin.ai";
pub const WEBAPP_HOST: &str = "app.devin.ai";
pub const SESSION_TOKEN_PREFIX: &str = "devin-session-token$";

const GET_USER_JWT: &str = "/exa.auth_pb.AuthService/GetUserJwt";
const GET_USER_STATUS: &str = "/exa.seat_management_pb.SeatManagementService/GetUserStatus";
const GET_CLI_MODEL_CONFIGS: &str = "/exa.api_server_pb.ApiServerService/GetCliModelConfigs";
pub const GET_CHAT_MESSAGE: &str = "/exa.api_server_pb.ApiServerService/GetChatMessage";

const IDE_NAME: &str = "chisel";
const IDE_VERSION: &str = "0.0.0-dev";
const EXTENSION_NAME: &str = "chisel";
const EXTENSION_VERSION: &str = "0.0.0-dev";

const CONNECT_COMPRESSED: u8 = 0x01;
const CONNECT_END_STREAM: u8 = 0x02;
const MAX_FRAME_PAYLOAD: usize = 16 * 1024 * 1024;

pub fn normalize_session_token(token: &str) -> String {
    if token.starts_with(SESSION_TOKEN_PREFIX) || token.starts_with("cog_") {
        token.to_owned()
    } else {
        format!("{SESSION_TOKEN_PREFIX}{token}")
    }
}

fn os_name() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    }
}

pub fn metadata(api_key: &str, user_jwt: Option<&str>, session_id: &str) -> Vec<u8> {
    let mut out = Vec::new();
    proto::field_str(&mut out, 1, IDE_NAME);
    proto::field_str(&mut out, 2, EXTENSION_VERSION);
    proto::field_str(&mut out, 3, api_key);
    proto::field_str(&mut out, 4, "en");
    proto::field_str(&mut out, 5, os_name());
    proto::field_str(&mut out, 7, IDE_VERSION);
    proto::field_varint(&mut out, 9, rand::random::<u64>());
    proto::field_str(&mut out, 10, session_id);
    proto::field_str(&mut out, 12, EXTENSION_NAME);
    if let Some(jwt) = user_jwt {
        proto::field_str(&mut out, 21, jwt);
    }
    out
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("connect error ({code}): {message}")]
    Rpc { code: String, message: String },
    #[error("malformed response")]
    Malformed,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl ConnectError {
    pub fn rpc(status: reqwest::StatusCode, body: &str) -> Self {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(body)
            && let Some(code) = value.get("code").and_then(serde_json::Value::as_str)
        {
            let message = value
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_owned();
            return Self::Rpc {
                code: code.to_owned(),
                message,
            };
        }
        Self::Rpc {
            code: format!("http_{status}"),
            message: body.chars().take(512).collect(),
        }
    }

    pub fn into_stream_error(self) -> StreamError {
        match self {
            Self::Http(err) => StreamError::transport(err.to_string()),
            Self::Io(err) => StreamError::transport(err.to_string()),
            Self::Malformed => StreamError::other("malformed provider response"),
            Self::Rpc { code, message } => classify_code(&code, &message),
        }
    }
}

fn classify_code(code: &str, message: &str) -> StreamError {
    let detail = if message.is_empty() { code } else { message };
    match code {
        "unauthenticated" | "permission_denied" | "http_401" | "http_403" => {
            StreamError::auth(detail)
        }
        "resource_exhausted" | "http_429" => StreamError::rate_limited(detail, None),
        "unavailable" | "internal" | "http_500" | "http_502" | "http_503" | "http_504" => {
            StreamError::overloaded(detail)
        }
        "invalid_argument"
        | "failed_precondition"
        | "out_of_range"
        | "unimplemented"
        | "http_400"
        | "http_404"
        | "http_422" => StreamError::invalid_request(detail),
        "deadline_exceeded" | "canceled" => StreamError::transport(detail),
        _ => StreamError::other(detail),
    }
}

pub fn unary_request(metadata: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    proto::field_msg(&mut out, 1, metadata);
    out
}

pub async fn unary(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    body: Vec<u8>,
    session_token: &str,
) -> Result<Vec<u8>, ConnectError> {
    let response = client
        .post(format!("{}{path}", base.trim_end_matches('/')))
        .header("content-type", "application/proto")
        .header("connect-protocol-version", "1")
        .header("accept", "*/*")
        .header(
            "authorization",
            format!("Basic {}", normalize_session_token(session_token)),
        )
        .body(body)
        .send()
        .await?;
    let status = response.status();
    let payload = response.bytes().await?;
    if !status.is_success() {
        return Err(ConnectError::rpc(
            status,
            &String::from_utf8_lossy(&payload),
        ));
    }
    Ok(payload.to_vec())
}

fn unwrap_gzip(payload: &[u8]) -> Vec<u8> {
    if payload.starts_with(&[0x1f, 0x8b]) {
        gunzip(payload).unwrap_or_else(|_| payload.to_vec())
    } else {
        payload.to_vec()
    }
}

pub struct AuthInfo {
    pub user_jwt: String,
    pub base_url: Option<String>,
}

pub async fn get_user_jwt(
    client: &reqwest::Client,
    base: &str,
    session_token: &str,
    session_id: &str,
) -> Result<AuthInfo, ConnectError> {
    let body = unary_request(&metadata(
        &normalize_session_token(session_token),
        None,
        session_id,
    ));
    let payload = unwrap_gzip(&unary(client, base, GET_USER_JWT, body, session_token).await?);
    let mut user_jwt = None;
    let mut custom_url = None;
    let mut reader = Reader::new(&payload);
    while let Some((field, value)) = reader.next() {
        match field {
            1 => user_jwt = as_str(value).map(str::to_owned),
            2 => custom_url = as_str(value).map(str::to_owned),
            _ => {}
        }
    }
    let user_jwt = user_jwt
        .filter(|jwt| !jwt.is_empty())
        .ok_or(ConnectError::Malformed)?;
    let base_url = custom_url
        .filter(|url| !url.trim().is_empty())
        .map(|url| url.trim_end_matches('/').to_owned());
    Ok(AuthInfo { user_jwt, base_url })
}

pub async fn verify_status(
    client: &reqwest::Client,
    base: &str,
    session_token: &str,
    session_id: &str,
) -> Result<(), ConnectError> {
    let body = unary_request(&metadata(
        &normalize_session_token(session_token),
        None,
        session_id,
    ));
    let payload = unary(client, base, GET_USER_STATUS, body, session_token).await?;
    let mut reader = Reader::new(&payload);
    match reader.next() {
        Some((1, Value::Bytes(_))) => Ok(()),
        _ => Err(ConnectError::Malformed),
    }
}

pub async fn model_configs(
    client: &reqwest::Client,
    base: &str,
    session_token: &str,
    session_id: &str,
) -> Result<Vec<Model>, ConnectError> {
    let body = unary_request(&metadata(
        &normalize_session_token(session_token),
        None,
        session_id,
    ));
    let payload = unary(client, base, GET_CLI_MODEL_CONFIGS, body, session_token).await?;
    let mut models = Vec::new();
    let mut reader = Reader::new(&payload);
    while let Some((field, value)) = reader.next() {
        if field != 1 {
            continue;
        }
        if let Value::Bytes(config) = value {
            let mut uid = None;
            let mut images = false;
            let mut disabled = false;
            let mut inner = Reader::new(config);
            while let Some((f, v)) = inner.next() {
                match f {
                    4 => disabled = as_u64(v) == Some(1),
                    5 => images = as_u64(v) == Some(1),
                    22 => uid = as_str(v).map(str::to_owned),
                    _ => {}
                }
            }
            if let Some(id) = uid
                && !disabled
                && !id.is_empty()
            {
                models.push(Model {
                    id,
                    supports_images: images,
                });
            }
        }
    }
    Ok(models)
}

pub fn gzip(payload: &[u8]) -> Result<Vec<u8>, std::io::Error> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    std::io::Write::write_all(&mut encoder, payload)?;
    encoder.finish()
}

pub fn gunzip(payload: &[u8]) -> Result<Vec<u8>, std::io::Error> {
    let mut decoder = flate2::read::GzDecoder::new(payload);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out)?;
    Ok(out)
}

pub enum Frame {
    Message(Vec<u8>),
    End(Vec<u8>),
}

pub fn encode_frame(payload: &[u8], compressed: bool) -> Result<Vec<u8>, ConnectError> {
    let (flag, body) = if compressed {
        (CONNECT_COMPRESSED, gzip(payload)?)
    } else {
        (0, payload.to_vec())
    };
    let len = u32::try_from(body.len())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "frame too large"))?;
    let mut out = Vec::with_capacity(5 + body.len());
    out.push(flag);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

pub fn take_frame(buffer: &mut Vec<u8>) -> Result<Option<Frame>, ConnectError> {
    if buffer.len() < 5 {
        return Ok(None);
    }
    let flag = buffer[0];
    let len = u32::from_be_bytes([buffer[1], buffer[2], buffer[3], buffer[4]]) as usize;
    if len > MAX_FRAME_PAYLOAD {
        return Err(ConnectError::Malformed);
    }
    if buffer.len() < 5 + len {
        return Ok(None);
    }
    let payload = buffer[5..5 + len].to_vec();
    buffer.drain(..5 + len);
    let raw = if flag & CONNECT_COMPRESSED != 0 {
        gunzip(&payload)?
    } else {
        payload
    };
    if flag & CONNECT_END_STREAM != 0 {
        return Ok(Some(Frame::End(raw)));
    }
    Ok(Some(Frame::Message(raw)))
}

pub fn trailer_error(payload: &[u8]) -> Option<ConnectError> {
    let text = std::str::from_utf8(payload).ok()?.trim().to_owned();
    if text.is_empty() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let error = value.get("error")?;
    let code = error
        .get("code")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let message = error
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_owned();
    Some(ConnectError::Rpc { code, message })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_token_normalization() {
        assert_eq!(normalize_session_token("abc"), "devin-session-token$abc");
        assert_eq!(
            normalize_session_token("devin-session-token$abc"),
            "devin-session-token$abc"
        );
        assert_eq!(normalize_session_token("cog_xyz"), "cog_xyz");
    }

    #[test]
    fn frame_round_trip() {
        let frame = encode_frame(b"hello", false).unwrap();
        let mut buffer = frame.clone();
        match take_frame(&mut buffer).unwrap() {
            Some(Frame::Message(payload)) => assert_eq!(payload, b"hello"),
            _ => panic!("expected message frame"),
        }
        assert!(buffer.is_empty());
    }

    #[test]
    fn compressed_frame_round_trip() {
        let frame = encode_frame(b"payload-bytes", true).unwrap();
        let mut buffer = frame;
        match take_frame(&mut buffer).unwrap() {
            Some(Frame::Message(payload)) => assert_eq!(payload, b"payload-bytes"),
            _ => panic!("expected message frame"),
        }
    }

    #[test]
    fn partial_frame_waits_for_more_data() {
        let frame = encode_frame(b"hello", false).unwrap();
        let mut buffer = frame[..4].to_vec();
        assert!(take_frame(&mut buffer).unwrap().is_none());
        buffer.extend_from_slice(&frame[4..]);
        assert!(take_frame(&mut buffer).unwrap().is_some());
    }

    #[test]
    fn oversized_frame_rejected() {
        let mut buffer = vec![0u8];
        buffer.extend_from_slice(&u32::MAX.to_be_bytes());
        assert!(take_frame(&mut buffer).is_err());
    }

    #[test]
    fn end_stream_frame_yields_trailer() {
        let mut out = vec![CONNECT_END_STREAM];
        let body = br#"{"error":{"code":"unauthenticated","message":"bad token"}}"#;
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(body);
        match take_frame(&mut out).unwrap() {
            Some(Frame::End(payload)) => {
                let err = trailer_error(&payload).unwrap();
                assert!(matches!(err, ConnectError::Rpc { .. }));
            }
            _ => panic!("expected end frame"),
        }
    }

    #[test]
    fn clean_trailer_has_no_error() {
        assert!(trailer_error(b"{}").is_none());
        assert!(trailer_error(b"").is_none());
        assert!(trailer_error(b"not json").is_none());
    }
}
