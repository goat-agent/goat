use std::pin::Pin;

use futures::{Sink, SinkExt, Stream, StreamExt};
use goat_wire::WireError;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

pub(crate) const MAX_MESSAGE: usize = 8 * 1024 * 1024;

pub(crate) fn config() -> WebSocketConfig {
    let mut config = WebSocketConfig::default();
    config.max_message_size = Some(MAX_MESSAGE);
    config.max_frame_size = Some(MAX_MESSAGE);
    config
}

pub(crate) type FrameSink<T> = Pin<Box<dyn Sink<T, Error = WireError> + Send>>;
pub(crate) type FrameStream<T> = Pin<Box<dyn Stream<Item = Result<T, WireError>> + Send>>;

pub(crate) fn adapt<S, Out, In>(ws: WebSocketStream<S>) -> (FrameSink<Out>, FrameStream<In>)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    Out: Serialize + Send + 'static,
    In: DeserializeOwned + Send + 'static,
{
    let (ws_sink, ws_stream) = ws.split();
    let sink = ws_sink
        .sink_map_err(|_| WireError::Closed)
        .with(|frame: Out| async move {
            let text = serde_json::to_string(&frame).map_err(WireError::Encode)?;
            Ok::<_, WireError>(Message::Text(text.into()))
        });
    let stream = ws_stream
        .filter_map(|item| async move {
            match item {
                Ok(Message::Text(text)) => {
                    Some(serde_json::from_str::<In>(&text).map_err(WireError::Decode))
                }
                Ok(Message::Binary(bytes)) => {
                    Some(serde_json::from_slice::<In>(&bytes).map_err(WireError::Decode))
                }
                Ok(Message::Close(_)) | Err(_) => Some(Err(WireError::Closed)),
                Ok(_) => None,
            }
        })
        .boxed();
    (Box::pin(sink), stream)
}
