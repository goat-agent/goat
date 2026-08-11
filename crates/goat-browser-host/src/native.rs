use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_TO_CHROME: usize = 1024 * 1024;
pub const MAX_FROM_CHROME: usize = 64 * 1024 * 1024;
pub const CHUNK_PAYLOAD: usize = 512 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum NativeError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("message of {0} bytes exceeds the {1} byte native messaging limit")]
    TooLarge(usize, usize),
    #[error("encode error: {0}")]
    Encode(serde_json::Error),
    #[error("decode error: {0}")]
    Decode(serde_json::Error),
    #[error("the browser closed the port")]
    Closed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Bridge {
    Message {
        seq: u64,
        body: Value,
    },
    Chunk {
        seq: u64,
        index: u32,
        total: u32,
        body: String,
    },
}

pub async fn read_message<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Value, NativeError> {
    let mut header = [0u8; 4];
    match reader.read_exact(&mut header).await {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(NativeError::Closed);
        }
        Err(err) => return Err(NativeError::Io(err)),
    }
    let len = u32::from_le_bytes(header) as usize;
    if len > MAX_FROM_CHROME {
        return Err(NativeError::TooLarge(len, MAX_FROM_CHROME));
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).await?;
    serde_json::from_slice(&body).map_err(NativeError::Decode)
}

pub async fn write_message<W: AsyncWrite + Unpin>(
    writer: &mut W,
    value: &Value,
) -> Result<(), NativeError> {
    let bytes = serde_json::to_vec(value).map_err(NativeError::Encode)?;
    if bytes.len() > MAX_TO_CHROME {
        return Err(NativeError::TooLarge(bytes.len(), MAX_TO_CHROME));
    }
    let len = u32::try_from(bytes.len())
        .map_err(|_| NativeError::TooLarge(bytes.len(), MAX_TO_CHROME))?;
    writer.write_all(&len.to_le_bytes()).await?;
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

#[must_use]
pub fn frame(seq: u64, body: &Value) -> Vec<Bridge> {
    let encoded = serde_json::to_string(body).unwrap_or_else(|_| "null".to_owned());
    if encoded.len() <= CHUNK_PAYLOAD {
        return vec![Bridge::Message {
            seq,
            body: body.clone(),
        }];
    }
    let mut pieces: Vec<String> = Vec::new();
    let mut current = String::new();
    for ch in encoded.chars() {
        if current.len() + ch.len_utf8() > CHUNK_PAYLOAD && !current.is_empty() {
            pieces.push(std::mem::take(&mut current));
        }
        current.push(ch);
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    let total = u32::try_from(pieces.len()).unwrap_or(u32::MAX);
    pieces
        .into_iter()
        .enumerate()
        .map(|(index, body)| Bridge::Chunk {
            seq,
            index: u32::try_from(index).unwrap_or(u32::MAX),
            total,
            body,
        })
        .collect()
}

#[derive(Default)]
pub struct Reassembler {
    pending: std::collections::HashMap<u64, Vec<Option<String>>>,
}

impl Reassembler {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn accept(&mut self, frame: Bridge) -> Result<Option<Value>, NativeError> {
        match frame {
            Bridge::Message { body, .. } => Ok(Some(body)),
            Bridge::Chunk {
                seq,
                index,
                total,
                body,
            } => {
                let slots = self
                    .pending
                    .entry(seq)
                    .or_insert_with(|| vec![None; total as usize]);
                if slots.len() != total as usize {
                    self.pending.remove(&seq);
                    return Ok(None);
                }
                if let Some(slot) = slots.get_mut(index as usize) {
                    *slot = Some(body);
                }
                if slots.iter().any(Option::is_none) {
                    return Ok(None);
                }
                let Some(slots) = self.pending.remove(&seq) else {
                    return Ok(None);
                };
                let joined: String = slots.into_iter().flatten().collect();
                serde_json::from_str(&joined)
                    .map(Some)
                    .map_err(NativeError::Decode)
            }
        }
    }

    #[must_use]
    pub fn pending(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Bridge, CHUNK_PAYLOAD, MAX_TO_CHROME, NativeError, Reassembler, frame, read_message,
        write_message,
    };
    use serde_json::{Value, json};

    #[tokio::test]
    async fn a_message_round_trips_through_the_length_prefix() {
        let mut buffer = Vec::new();
        let value = json!({"hello": "world"});
        write_message(&mut buffer, &value).await.unwrap();
        assert_eq!(&buffer[..4], &(buffer.len() as u32 - 4).to_le_bytes());

        let mut cursor = std::io::Cursor::new(buffer);
        let back = read_message(&mut cursor).await.unwrap();
        assert_eq!(back, value);
    }

    #[tokio::test]
    async fn a_closed_port_is_reported_as_closed_not_as_an_io_error() {
        let mut empty = std::io::Cursor::new(Vec::new());
        let err = read_message(&mut empty).await.unwrap_err();
        assert!(matches!(err, NativeError::Closed));
    }

    #[tokio::test]
    async fn an_oversized_message_to_chrome_is_refused_before_the_write() {
        let mut buffer = Vec::new();
        let value = json!({ "big": "x".repeat(MAX_TO_CHROME) });
        let err = write_message(&mut buffer, &value).await.unwrap_err();
        assert!(matches!(err, NativeError::TooLarge(_, MAX_TO_CHROME)));
        assert!(buffer.is_empty(), "nothing may be written when refused");
    }

    #[test]
    fn a_small_body_is_sent_whole() {
        let frames = frame(1, &json!({"a": 1}));
        assert_eq!(frames.len(), 1);
        assert!(matches!(frames[0], Bridge::Message { seq: 1, .. }));
    }

    #[test]
    fn a_large_body_is_split_into_bounded_chunks() {
        let value = json!({ "blob": "x".repeat(CHUNK_PAYLOAD * 2) });
        let frames = frame(7, &value);
        assert!(
            frames.len() >= 3,
            "expected several chunks, got {}",
            frames.len()
        );
        for piece in &frames {
            let Bridge::Chunk { body, seq, .. } = piece else {
                panic!("a split body must produce chunks only")
            };
            assert_eq!(*seq, 7);
            assert!(body.len() <= CHUNK_PAYLOAD);
        }
    }

    #[test]
    fn chunks_reassemble_into_the_original_value() {
        let value = json!({ "blob": "y".repeat(CHUNK_PAYLOAD * 2), "n": 42 });
        let frames = frame(9, &value);
        let mut reassembler = Reassembler::new();
        let mut done = None;
        for piece in frames {
            if let Some(complete) = reassembler.accept(piece).unwrap() {
                done = Some(complete);
            }
        }
        assert_eq!(done, Some(value));
        assert_eq!(reassembler.pending(), 0);
    }

    #[test]
    fn chunks_arriving_out_of_order_still_reassemble() {
        let value = json!({ "blob": "z".repeat(CHUNK_PAYLOAD * 2) });
        let mut frames = frame(3, &value);
        frames.reverse();
        let mut reassembler = Reassembler::new();
        let mut done = None;
        for piece in frames {
            if let Some(complete) = reassembler.accept(piece).unwrap() {
                done = Some(complete);
            }
        }
        assert_eq!(done, Some(value));
    }

    #[test]
    fn a_partial_body_never_yields_a_value() {
        let value = json!({ "blob": "w".repeat(CHUNK_PAYLOAD * 2) });
        let mut frames = frame(4, &value);
        frames.pop();
        let mut reassembler = Reassembler::new();
        for piece in frames {
            assert_eq!(reassembler.accept(piece).unwrap(), None);
        }
        assert_eq!(reassembler.pending(), 1);
    }

    #[test]
    fn a_chunk_that_disagrees_about_the_total_drops_the_group() {
        let mut reassembler = Reassembler::new();
        assert_eq!(
            reassembler
                .accept(Bridge::Chunk {
                    seq: 1,
                    index: 0,
                    total: 3,
                    body: "a".to_owned()
                })
                .unwrap(),
            None
        );
        assert_eq!(
            reassembler
                .accept(Bridge::Chunk {
                    seq: 1,
                    index: 0,
                    total: 9,
                    body: "b".to_owned()
                })
                .unwrap(),
            None
        );
        assert_eq!(reassembler.pending(), 0);
    }

    #[test]
    fn reassembled_garbage_is_a_decode_error_not_a_panic() {
        let mut reassembler = Reassembler::new();
        let err = reassembler
            .accept(Bridge::Chunk {
                seq: 2,
                index: 0,
                total: 1,
                body: "{not json".to_owned(),
            })
            .unwrap_err();
        assert!(matches!(err, NativeError::Decode(_)));
    }

    #[test]
    fn bridge_frames_round_trip_as_json() {
        for piece in [
            Bridge::Message {
                seq: 1,
                body: json!({"x": 1}),
            },
            Bridge::Chunk {
                seq: 2,
                index: 1,
                total: 4,
                body: "part".to_owned(),
            },
        ] {
            let text = serde_json::to_string(&piece).unwrap();
            let back: Bridge = serde_json::from_str(&text).unwrap();
            assert_eq!(back, piece);
        }
    }

    #[tokio::test]
    async fn a_message_larger_than_chromes_inbound_limit_is_refused_on_read() {
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&(super::MAX_FROM_CHROME as u32 + 1).to_le_bytes());
        let mut cursor = std::io::Cursor::new(buffer);
        let err = read_message(&mut cursor).await.unwrap_err();
        assert!(matches!(err, NativeError::TooLarge(_, _)));
        let _ = Value::Null;
    }
}
