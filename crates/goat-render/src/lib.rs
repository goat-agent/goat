use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use futures::StreamExt;
use goat_channel::{ChannelError, ChannelHandle, ChannelResult, SentRef};
use goat_provider::{ChunkStream, StreamChunk};
use goat_types::{ConversationId, MessageId, OutgoingBody};
use tracing::warn;

#[derive(Clone, Debug, Default)]
pub struct RenderSummary {
    pub messages_sent: u32,
    pub edits: u32,
    pub final_text: String,
}

#[async_trait]
pub trait StreamRenderer: Send + Sync {
    async fn render(
        &self,
        handle: Arc<dyn ChannelHandle>,
        conv: ConversationId,
        reply_to: Option<MessageId>,
        stream: ChunkStream,
    ) -> ChannelResult<RenderSummary>;
}

pub struct DefaultStreamRenderer;

const MIN_CHARS_PER_FLUSH: usize = 80;
const CODE_FENCE_WRAP_CHARS: usize = 4;

#[async_trait]
impl StreamRenderer for DefaultStreamRenderer {
    async fn render(
        &self,
        handle: Arc<dyn ChannelHandle>,
        conv: ConversationId,
        reply_to: Option<MessageId>,
        mut stream: ChunkStream,
    ) -> ChannelResult<RenderSummary> {
        let caps = handle.capabilities();
        let mut buf = String::new();
        let mut current_block_chars: usize = 0;
        let mut current: Option<SentRef> = None;
        let mut current_reply = reply_to.clone();
        let mut last_flush = Instant::now();
        let mut full_text = String::new();
        let mut summary = RenderSummary::default();

        while let Some(item) = stream.next().await {
            let chunk = match item {
                Ok(c) => c,
                Err(e) => {
                    warn!(error = %e, "stream error");
                    return Err(ChannelError::Provider(e.to_string()));
                }
            };

            if let StreamChunk::TextDelta { text } = chunk {
                let delta_chars = text.chars().count();
                full_text.push_str(&text);
                buf.push_str(&text);
                current_block_chars += delta_chars;

                while current_block_chars > caps.max_message_chars {
                    let (head, tail) = split_for_channel(&buf, caps.max_message_chars);
                    if let Some(sent) = current.as_ref() {
                        handle.edit(sent, OutgoingBody::Text(head)).await?;
                        summary.edits += 1;
                    } else {
                        handle
                            .send(&conv, OutgoingBody::Text(head), current_reply.clone())
                            .await?;
                        summary.messages_sent += 1;
                    }
                    buf = tail;
                    current_block_chars = buf.chars().count();
                    current = None;
                    current_reply = None;
                    last_flush = Instant::now();
                }

                let due = last_flush.elapsed() >= caps.edit_min_interval;
                let big_enough = buf.chars().count() >= MIN_CHARS_PER_FLUSH;
                if due && big_enough {
                    flush(
                        handle.as_ref(),
                        &conv,
                        &mut current,
                        &buf,
                        current_reply.clone(),
                        &mut summary,
                    )
                    .await?;
                    last_flush = Instant::now();
                }
            }
        }

        if !buf.is_empty() {
            flush(
                handle.as_ref(),
                &conv,
                &mut current,
                &buf,
                current_reply.clone(),
                &mut summary,
            )
            .await?;
        }

        summary.final_text = full_text;
        Ok(summary)
    }
}

async fn flush(
    handle: &dyn ChannelHandle,
    conv: &ConversationId,
    current: &mut Option<SentRef>,
    text: &str,
    reply_to: Option<MessageId>,
    summary: &mut RenderSummary,
) -> ChannelResult<()> {
    if text.is_empty() {
        return Ok(());
    }
    if let Some(sent) = current {
        handle
            .edit(sent, OutgoingBody::Text(text.to_string()))
            .await?;
        summary.edits += 1;
    } else {
        let sent = handle
            .send(conv, OutgoingBody::Text(text.to_string()), reply_to)
            .await?;
        summary.messages_sent += 1;
        *current = Some(sent);
    }
    Ok(())
}

fn split_at_char_count(s: &str, n: usize) -> (String, String) {
    match s.char_indices().nth(n) {
        Some((idx, _)) => (s[..idx].to_string(), s[idx..].to_string()),
        None => (s.to_string(), String::new()),
    }
}

fn split_for_channel(s: &str, max_chars: usize) -> (String, String) {
    if max_chars == 0 || s.chars().count() <= max_chars {
        return (s.to_string(), String::new());
    }

    let (mut head, mut tail) = split_at_newline_or_char_count(s, max_chars);
    if has_unclosed_code_fence(&head) && max_chars > CODE_FENCE_WRAP_CHARS * 2 {
        let reserved = max_chars.saturating_sub(CODE_FENCE_WRAP_CHARS);
        let (fenced_head, fenced_tail) = split_at_newline_or_char_count(s, reserved);
        if !fenced_head.is_empty() && has_unclosed_code_fence(&fenced_head) {
            head = fenced_head;
            if !head.ends_with('\n') {
                head.push('\n');
            }
            head.push_str("```");
            tail = format!("```\n{fenced_tail}");
        }
    }

    (head, tail)
}

fn split_at_newline_or_char_count(s: &str, max_chars: usize) -> (String, String) {
    let prefix_end = byte_index_after_char_count(s, max_chars);
    let prefix = &s[..prefix_end];
    if let Some(split) = prefix.rfind('\n').map(|idx| idx + '\n'.len_utf8())
        && split > 1
    {
        return (s[..split].to_string(), s[split..].to_string());
    }
    split_at_char_count(s, max_chars)
}

fn byte_index_after_char_count(s: &str, n: usize) -> usize {
    match s.char_indices().nth(n) {
        Some((idx, _)) => idx,
        None => s.len(),
    }
}

fn has_unclosed_code_fence(s: &str) -> bool {
    s.lines()
        .filter(|line| line.trim_start().starts_with("```"))
        .count()
        % 2
        == 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use goat_channel::test_support::{MockChannelHandle, MockEvent};
    use goat_channel::{ChannelCapabilities, ChannelIdentity};
    use goat_provider::{StreamChunk, StreamError};
    use goat_types::{ChannelId, ConversationId, InstanceId, ProfileId};

    fn mock_handle(caps: ChannelCapabilities) -> Arc<MockChannelHandle> {
        MockChannelHandle::new(
            ChannelId::new("telegram"),
            ProfileId::new(),
            InstanceId::new(),
            ChannelIdentity::new("goatbot", "Goat"),
            caps,
        )
    }

    fn conv(instance: InstanceId) -> ConversationId {
        ConversationId::new(ChannelId::new("telegram"), instance, "chat:1")
    }

    fn text_delta(s: &str) -> StreamChunk {
        StreamChunk::TextDelta {
            text: s.to_string(),
        }
    }

    fn make_stream(chunks: Vec<StreamChunk>) -> ChunkStream {
        let items: Vec<Result<StreamChunk, StreamError>> = chunks.into_iter().map(Ok).collect();
        Box::pin(stream::iter(items))
    }

    #[test]
    fn split_at_char_count_korean_glyphs() {
        let (head, tail) = split_at_char_count("안녕하세요", 3);
        assert_eq!(head.chars().count(), 3);
        assert_eq!(tail.chars().count(), 2);
        assert_eq!(format!("{head}{tail}"), "안녕하세요");
    }

    #[test]
    fn split_at_char_count_handles_short_input() {
        let (head, tail) = split_at_char_count("abc", 10);
        assert_eq!(head, "abc");
        assert!(tail.is_empty());
    }

    #[test]
    fn split_at_char_count_handles_emoji_surrogates() {
        let s = "🐐🐐🐐🐐🐐";
        let (head, tail) = split_at_char_count(s, 2);
        assert_eq!(head.chars().count(), 2);
        assert_eq!(tail.chars().count(), 3);
        assert_eq!(format!("{head}{tail}"), s);
    }

    #[test]
    fn split_for_channel_prefers_newline_boundaries() {
        let s = "one jmo staff\nsecond line\n";
        let (head, tail) = split_for_channel(s, 16);
        assert_eq!(head, "one jmo staff\n");
        assert_eq!(tail, "second line\n");
        assert!(!head.ends_with("jm"));
        assert!(!tail.starts_with('o'));
    }

    #[test]
    fn split_for_channel_rewraps_open_code_fences() {
        let s = "```text\nalpha\nbeta\ngamma\n```";
        let (head, tail) = split_for_channel(s, 20);
        assert!(head.chars().count() <= 20);
        assert!(head.starts_with("```text\n"));
        assert!(head.ends_with("```"));
        assert!(tail.starts_with("```\n"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn renders_short_text_as_one_send() {
        let caps = ChannelCapabilities::new(4096, std::time::Duration::from_millis(0), None);
        let handle = mock_handle(caps);
        let instance = handle.instance();
        let stream = make_stream(vec![text_delta("hello")]);
        let summary = DefaultStreamRenderer
            .render(handle.clone(), conv(instance), None, stream)
            .await
            .expect("render ok");

        assert_eq!(summary.messages_sent, 1);
        assert_eq!(summary.final_text, "hello");
        let events = handle.events().await;
        let texts: Vec<&str> = events.iter().filter_map(MockEvent::as_text).collect();
        assert!(texts.contains(&"hello"), "expected hello in: {texts:?}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn korean_text_rolls_over_at_char_boundary_not_byte() {
        let caps = ChannelCapabilities::new(10, std::time::Duration::from_millis(0), None);
        let handle = mock_handle(caps);
        let instance = handle.instance();
        let payload = "안녕하세요반가워요친구야";
        assert_eq!(payload.chars().count(), 12);
        let stream = make_stream(vec![text_delta(payload)]);
        let summary = DefaultStreamRenderer
            .render(handle.clone(), conv(instance), None, stream)
            .await
            .unwrap();

        assert_eq!(summary.messages_sent, 2, "one rollover expected");
        assert_eq!(summary.final_text, payload);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn rollover_final_text_contains_full_response() {
        let caps = ChannelCapabilities::new(100, std::time::Duration::from_millis(0), None);
        let handle = mock_handle(caps);
        let instance = handle.instance();
        let payload: String = (0..4000)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        let stream = make_stream(vec![text_delta(&payload)]);
        let summary = DefaultStreamRenderer
            .render(handle.clone(), conv(instance), None, stream)
            .await
            .unwrap();
        assert_eq!(summary.final_text, payload);
        assert_eq!(summary.messages_sent, 40);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tool_call_chunks_are_hidden_from_final_text() {
        let caps = ChannelCapabilities::new(4096, std::time::Duration::from_millis(0), None);
        let handle = mock_handle(caps);
        let instance = handle.instance();
        let stream = make_stream(vec![
            text_delta("before"),
            StreamChunk::ToolCall {
                id: "t1".into(),
                name: "search".into(),
                input: "{\"q\":1}".into(),
            },
            text_delta("after"),
        ]);
        let summary = DefaultStreamRenderer
            .render(handle.clone(), conv(instance), None, stream)
            .await
            .unwrap();
        assert_eq!(summary.final_text, "beforeafter");
        assert!(!summary.final_text.contains("calling tool"));
        assert!(!summary.final_text.contains("search"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reasoning_deltas_are_hidden() {
        let caps = ChannelCapabilities::new(4096, std::time::Duration::from_millis(0), None);
        let handle = mock_handle(caps);
        let instance = handle.instance();
        let stream = make_stream(vec![
            StreamChunk::ThinkingDelta {
                text: "[private thoughts]".into(),
            },
            text_delta("public"),
        ]);
        let summary = DefaultStreamRenderer
            .render(handle.clone(), conv(instance), None, stream)
            .await
            .unwrap();
        assert_eq!(summary.final_text, "public");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn empty_stream_sends_nothing() {
        let caps = ChannelCapabilities::new(4096, std::time::Duration::from_millis(0), None);
        let handle = mock_handle(caps);
        let instance = handle.instance();
        let stream = make_stream(vec![]);
        let summary = DefaultStreamRenderer
            .render(handle.clone(), conv(instance), None, stream)
            .await
            .unwrap();
        assert_eq!(summary.messages_sent, 0);
        assert_eq!(summary.edits, 0);
        assert_eq!(summary.final_text, "");
    }
}
