use std::sync::Arc;

use color_eyre::eyre::eyre;
use goat_api::{BrowserEvent, BrowserEventParams, CdpEvent};
use goat_browser_host::native::{Bridge, NativeError, Reassembler, frame, read_message};
use goat_browser_host::{BrowserHost, BrowserPort, advertise, advertisement, withdrawal};
use goat_wire::envelope::{CallError, ErrorCode, Execution, Hello, Role};
use serde_json::{Value, json};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;

pub struct StdoutPort<W> {
    writer: Mutex<W>,
    seq: std::sync::atomic::AtomicU64,
}

impl<W: AsyncWrite + Unpin + Send> StdoutPort<W> {
    pub fn new(writer: W) -> Self {
        Self {
            writer: Mutex::new(writer),
            seq: std::sync::atomic::AtomicU64::new(0),
        }
    }

    async fn emit(&self, body: &Value) -> Result<(), String> {
        let seq = self
            .seq
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            .wrapping_add(1);
        let mut writer = self.writer.lock().await;
        for piece in frame(seq, body) {
            let encoded = serde_json::to_vec(&piece).map_err(|err| err.to_string())?;
            let len = u32::try_from(encoded.len()).map_err(|_| "frame too large".to_owned())?;
            writer
                .write_all(&len.to_le_bytes())
                .await
                .map_err(|err| err.to_string())?;
            writer
                .write_all(&encoded)
                .await
                .map_err(|err| err.to_string())?;
        }
        writer.flush().await.map_err(|err| err.to_string())?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl<W: AsyncWrite + Unpin + Send + Sync> BrowserPort for StdoutPort<W> {
    async fn dispatch(&self, request_id: u64, params: Value) -> Result<(), String> {
        self.emit(&json!({
            "type": "browser.request",
            "request_id": request_id.to_string(),
            "params": params,
        }))
        .await
    }
}

pub fn parse_event(body: &Value) -> Option<CdpEvent> {
    if body.get("type").and_then(Value::as_str) != Some("browser.event") {
        return None;
    }
    serde_json::from_value(body.get("event")?.clone()).ok()
}

pub fn parse_reply(body: &Value) -> Option<(u64, Result<Value, CallError>)> {
    if body.get("type").and_then(Value::as_str) != Some("browser.reply") {
        return None;
    }
    let request_id = body
        .get("request_id")
        .and_then(Value::as_str)
        .and_then(|text| text.parse::<u64>().ok())?;
    if let Some(error) = body.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("the browser reported an error")
            .to_owned();
        let started = error
            .get("started")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let execution = if started {
            Execution::OutcomeUnknown
        } else {
            Execution::NotStarted
        };
        return Some((
            request_id,
            Err(CallError::new(ErrorCode::Denied, message).with_execution(execution)),
        ));
    }
    let result = body.get("result").cloned().unwrap_or(Value::Null);
    Some((request_id, Ok(result)))
}

pub async fn run(instance: Option<String>, label: Option<String>) -> color_eyre::Result<()> {
    let link = crate::remote::resolve(None)?;
    let instance = instance.unwrap_or_else(|| "chrome-default".to_owned());
    let label = label.unwrap_or_else(|| "Chrome".to_owned());
    let boot_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();

    let port = Arc::new(StdoutPort::new(tokio::io::stdout()));
    let host = Arc::new(BrowserHost::new(port));

    let hello = Hello::new(
        Role::Client,
        concat!("goat-browser-host/", env!("CARGO_PKG_VERSION")),
    )
    .with_method(
        goat_browser_host::CAPABILITY,
        vec![goat_browser_host::CAPABILITY_VERSION],
    );
    let session = goat_client::open_serving(&link, "goat-browser-host", host.clone(), hello)
        .await
        .map_err(|err| eyre!("{err}"))?;

    advertise(
        &session.api,
        advertisement(instance.clone(), label, boot_epoch),
    )
    .await
    .map_err(|err| eyre!("{err}"))?;

    let mut stdin = tokio::io::stdin();
    let mut reassembler = Reassembler::new();
    loop {
        let message = match read_message(&mut stdin).await {
            Ok(message) => message,
            Err(NativeError::Closed) => break,
            Err(err) => return Err(eyre!("{err}")),
        };
        let body = match serde_json::from_value::<Bridge>(message.clone()) {
            Ok(bridge) => reassembler.accept(bridge).map_err(|err| eyre!("{err}"))?,
            Err(_) => Some(message),
        };
        let Some(body) = body else { continue };
        if let Some((request_id, result)) = parse_reply(&body) {
            host.settle(request_id, result).await;
        } else if let Some(event) = parse_event(&body) {
            let _ = session
                .api
                .call::<BrowserEvent>(BrowserEventParams {
                    instance: instance.clone(),
                    event,
                })
                .await;
        }
    }

    host.fail_all("the browser closed the port").await;
    let _ = advertise(&session.api, withdrawal(instance, boot_epoch)).await;
    session.shutdown();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{StdoutPort, parse_event, parse_reply};
    use goat_browser_host::BrowserPort;
    use goat_browser_host::native::Bridge;
    use goat_wire::envelope::{ErrorCode, Execution};
    use serde_json::json;

    fn decode_frames(buffer: &[u8]) -> Vec<Bridge> {
        let mut out = Vec::new();
        let mut cursor = 0usize;
        while cursor + 4 <= buffer.len() {
            let len = u32::from_le_bytes(buffer[cursor..cursor + 4].try_into().unwrap()) as usize;
            cursor += 4;
            out.push(serde_json::from_slice(&buffer[cursor..cursor + len]).unwrap());
            cursor += len;
        }
        out
    }

    #[tokio::test]
    async fn a_dispatch_writes_one_length_prefixed_request() {
        let mut buffer = Vec::new();
        {
            let port = StdoutPort::new(&mut buffer);
            port.dispatch(7, json!({"action": "navigate"}))
                .await
                .unwrap();
        }
        let frames = decode_frames(&buffer);
        assert_eq!(frames.len(), 1);
        let Bridge::Message { seq, body } = &frames[0] else {
            panic!("a small request must not be chunked")
        };
        assert_eq!(*seq, 1);
        assert_eq!(body["type"], "browser.request");
        assert_eq!(body["request_id"], "7");
        assert_eq!(body["params"]["action"], "navigate");
    }

    #[tokio::test]
    async fn successive_dispatches_get_distinct_sequence_numbers() {
        let mut buffer = Vec::new();
        {
            let port = StdoutPort::new(&mut buffer);
            port.dispatch(1, json!({})).await.unwrap();
            port.dispatch(2, json!({})).await.unwrap();
        }
        let frames = decode_frames(&buffer);
        let seqs: Vec<u64> = frames
            .iter()
            .map(|piece| match piece {
                Bridge::Message { seq, .. } | Bridge::Chunk { seq, .. } => *seq,
            })
            .collect();
        assert_eq!(seqs, vec![1, 2]);
    }

    #[tokio::test]
    async fn a_large_request_is_chunked_over_the_port() {
        let mut buffer = Vec::new();
        {
            let port = StdoutPort::new(&mut buffer);
            port.dispatch(1, json!({"blob": "x".repeat(1024 * 1024)}))
                .await
                .unwrap();
        }
        let frames = decode_frames(&buffer);
        assert!(frames.len() > 1);
        assert!(
            frames
                .iter()
                .all(|piece| matches!(piece, Bridge::Chunk { .. }))
        );
    }

    #[test]
    fn a_successful_reply_is_parsed() {
        let (id, result) = parse_reply(&json!({
            "type": "browser.reply",
            "request_id": "12",
            "result": {"summary": "navigated"}
        }))
        .expect("a reply parses");
        assert_eq!(id, 12);
        assert_eq!(result.unwrap()["summary"], "navigated");
    }

    #[test]
    fn an_error_reply_that_never_started_is_retry_safe() {
        let (_id, result) = parse_reply(&json!({
            "type": "browser.reply",
            "request_id": "1",
            "error": {"message": "no active tab", "started": false}
        }))
        .expect("a reply parses");
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::Denied);
        assert_eq!(err.execution, Some(Execution::NotStarted));
        assert!(err.retry_is_safe());
    }

    #[test]
    fn an_error_reply_that_started_is_never_retry_safe() {
        let (_id, result) = parse_reply(&json!({
            "type": "browser.reply",
            "request_id": "1",
            "error": {"message": "the tab closed mid-click", "started": true}
        }))
        .expect("a reply parses");
        let err = result.unwrap_err();
        assert_eq!(err.execution, Some(Execution::OutcomeUnknown));
        assert!(!err.retry_is_safe());
    }

    #[test]
    fn unrelated_messages_are_ignored() {
        assert!(parse_reply(&json!({"type": "hello"})).is_none());
        assert!(parse_reply(&json!({"type": "browser.reply"})).is_none());
        assert!(
            parse_reply(&json!({"type": "browser.reply", "request_id": "abc"})).is_none(),
            "a non-numeric request id must not be accepted"
        );
    }

    #[test]
    fn an_unsolicited_event_is_parsed_without_a_request_id() {
        let event = parse_event(&json!({
            "type": "browser.event",
            "event": {"method": "Page.loadEventFired", "params": {"timestamp": 1.0}}
        }))
        .expect("an event parses");
        assert_eq!(event.method, "Page.loadEventFired");
        assert_eq!(event.params["timestamp"], 1.0);
    }

    #[test]
    fn a_reply_is_never_read_as_an_event() {
        assert!(parse_event(&json!({"type": "browser.reply", "request_id": "1"})).is_none());
        assert!(parse_event(&json!({"type": "browser.event"})).is_none());
    }
}
