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
