use std::marker::PhantomData;

use futures::{Sink, SinkExt, Stream, StreamExt};
use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("encode error: {0}")]
    Encode(serde_json::Error),
    #[error("decode error: {0}")]
    Decode(serde_json::Error),
    #[error("connection closed")]
    Closed,
}

pub struct WireConn<S, Tx, Rx> {
    framed: Framed<S, LengthDelimitedCodec>,
    _tx: PhantomData<Tx>,
    _rx: PhantomData<Rx>,
}

impl<S, Tx, Rx> WireConn<S, Tx, Rx>
where
    S: AsyncRead + AsyncWrite + Unpin,
    Tx: Serialize,
    Rx: DeserializeOwned,
{
    pub fn new(stream: S) -> Self {
        let codec = LengthDelimitedCodec::builder()
            .max_frame_length(64 * 1024 * 1024)
            .new_codec();
        Self {
            framed: Framed::new(stream, codec),
            _tx: PhantomData,
            _rx: PhantomData,
        }
    }

    pub async fn send(&mut self, msg: &Tx) -> Result<(), WireError> {
        let bytes = serde_json::to_vec(msg).map_err(WireError::Encode)?;
        self.framed.send(bytes.into()).await?;
        Ok(())
    }

    pub async fn recv(&mut self) -> Result<Rx, WireError> {
        match self.framed.next().await {
            Some(Ok(bytes)) => serde_json::from_slice::<Rx>(&bytes).map_err(WireError::Decode),
            Some(Err(err)) => Err(WireError::Io(err)),
            None => Err(WireError::Closed),
        }
    }

    pub fn split(
        self,
    ) -> (
        impl Sink<Tx, Error = WireError>,
        impl Stream<Item = Result<Rx, WireError>>,
    ) {
        let (sink, stream) = self.framed.split();
        let sink = sink.with(|msg: Tx| async move {
            serde_json::to_vec(&msg)
                .map(bytes::Bytes::from)
                .map_err(WireError::Encode)
        });
        let stream = stream.map(|item| match item {
            Ok(bytes) => serde_json::from_slice::<Rx>(&bytes).map_err(WireError::Decode),
            Err(err) => Err(WireError::Io(err)),
        });
        (sink, stream)
    }
}

#[cfg(test)]
mod tests {
    use super::WireConn;
    use serde_json::Value;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn messages_use_a_big_endian_four_byte_length_prefix() {
        let (stream, mut raw) = tokio::io::duplex(64);
        let mut conn = WireConn::<_, Value, Value>::new(stream);
        conn.send(&serde_json::json!({"ok": true})).await.unwrap();

        let mut prefix = [0; 4];
        raw.read_exact(&mut prefix).await.unwrap();
        let len = u32::from_be_bytes(prefix);
        let mut body = vec![0; usize::try_from(len).unwrap()];
        raw.read_exact(&mut body).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            serde_json::json!({"ok": true})
        );
    }

    #[tokio::test]
    async fn a_big_endian_length_delimited_message_decodes() {
        let (stream, mut raw) = tokio::io::duplex(64);
        let mut conn = WireConn::<_, Value, Value>::new(stream);
        raw.write_all(&4_u32.to_be_bytes()).await.unwrap();
        raw.write_all(b"null").await.unwrap();
        assert_eq!(conn.recv().await.unwrap(), Value::Null);
    }
}
