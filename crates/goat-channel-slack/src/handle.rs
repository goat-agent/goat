use std::sync::Arc;

use async_trait::async_trait;
use goat_channel::{
    ChannelCapabilities, ChannelError, ChannelHandle, ChannelIdentity, ChannelResult, ChannelTurn,
    SentRef, TypingGuard,
};
use goat_types::{
    AgentId, ChannelId, IncomingMessage, InstanceId, MessageId, OutgoingBody, Surface, ThreadId,
};

use crate::api::SlackApi;
use crate::{CAPABILITIES, ID, mrkdwn, thread};

pub(crate) struct SlackHandle {
    instance: InstanceId,
    agent: AgentId,
    identity: ChannelIdentity,
    api: Arc<SlackApi>,
}

impl SlackHandle {
    pub(crate) fn new(
        instance: InstanceId,
        agent: AgentId,
        identity: ChannelIdentity,
        api: Arc<SlackApi>,
    ) -> Self {
        Self {
            instance,
            agent,
            identity,
            api,
        }
    }
}

#[async_trait]
impl ChannelHandle for SlackHandle {
    fn instance(&self) -> InstanceId {
        self.instance
    }

    fn agent(&self) -> AgentId {
        self.agent
    }

    fn id(&self) -> ChannelId {
        ID.clone()
    }

    fn identity(&self) -> ChannelIdentity {
        self.identity.clone()
    }

    fn capabilities(&self) -> ChannelCapabilities {
        CAPABILITIES
    }

    async fn surface(&self, stored_thread: &ThreadId) -> ChannelResult<Surface> {
        let coords = thread::parse(&stored_thread.external)?;
        if coords.channel.starts_with('D') {
            Ok(Surface::Dm)
        } else if coords.thread_ts.is_some() {
            Ok(Surface::Thread)
        } else {
            Ok(Surface::Channel)
        }
    }

    async fn send(
        &self,
        conv: &ThreadId,
        body: OutgoingBody,
        _reply_to: Option<MessageId>,
    ) -> ChannelResult<SentRef> {
        let coords = thread::parse(&conv.external)?;
        let text = outgoing_text(body)?;
        let posted = self
            .api
            .post_message(&coords.channel, &text, coords.thread_ts.as_deref())
            .await?;
        Ok(sent_ref(&posted.channel, &posted.ts))
    }

    async fn edit(&self, sent: &SentRef, body: OutgoingBody) -> ChannelResult<()> {
        let (channel, ts) = sent_coords(sent)?;
        let text = outgoing_text(body)?;
        self.api.update_message(&channel, &ts, &text).await
    }

    async fn typing(&self, _conv: &ThreadId) -> ChannelResult<TypingGuard> {
        Ok(TypingGuard::noop())
    }

    async fn prepare_turn(&self, _msg: &IncomingMessage) -> ChannelResult<ChannelTurn> {
        Ok(ChannelTurn {
            reply_to: None,
            typing: TypingGuard::noop(),
        })
    }

    fn supports_threads(&self) -> bool {
        true
    }

    async fn open_thread(
        &self,
        parent: &ThreadId,
        anchor: Option<&MessageId>,
        _title: &str,
    ) -> ChannelResult<ThreadId> {
        let coords = thread::parse(&parent.external)?;
        if let Some(existing) = coords.thread_ts {
            return Ok(ThreadId::new(
                ID.clone(),
                parent.instance,
                thread::external(&coords.channel, Some(&existing)),
            ));
        }
        let anchor = anchor.ok_or_else(|| {
            ChannelError::BadRequest(
                "slack: a thread needs the parent message it hangs off".to_string(),
            )
        })?;
        Ok(ThreadId::new(
            ID.clone(),
            parent.instance,
            thread::external(&coords.channel, Some(&anchor.0)),
        ))
    }
}

fn outgoing_text(body: OutgoingBody) -> ChannelResult<String> {
    match body {
        OutgoingBody::Text(text) => Ok(mrkdwn::to_mrkdwn(&text)),
        _ => Err(ChannelError::Unsupported("slack: only text is supported")),
    }
}

fn sent_ref(channel: &str, ts: &str) -> SentRef {
    SentRef {
        channel: ID.clone(),
        message_id: MessageId(ts.to_string()),
        raw: serde_json::json!({ "channel": channel, "ts": ts }),
    }
}

fn sent_coords(sent: &SentRef) -> ChannelResult<(String, String)> {
    let channel = sent
        .raw
        .get("channel")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty());
    let ts = sent
        .raw
        .get("ts")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| Some(sent.message_id.0.clone()))
        .filter(|value| !value.is_empty());
    match (channel, ts) {
        (Some(channel), Some(ts)) => Ok((channel.to_string(), ts)),
        _ => Err(ChannelError::BadRequest(
            "slack: cannot edit a message without its channel and ts".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use goat_types::Attachment;

    #[test]
    fn a_sent_ref_carries_both_coordinates_slack_needs_to_edit() {
        let sent = sent_ref("C1", "1712345678.000100");
        assert_eq!(sent.channel.as_str(), "slack");
        assert_eq!(sent.message_id.0, "1712345678.000100");
        assert_eq!(
            sent_coords(&sent).unwrap(),
            ("C1".to_string(), "1712345678.000100".to_string())
        );
    }

    #[test]
    fn sent_coords_falls_back_to_the_message_id_when_raw_has_no_ts() {
        let sent = SentRef {
            channel: ID.clone(),
            message_id: MessageId("1.1".to_string()),
            raw: serde_json::json!({ "channel": "C1" }),
        };
        assert_eq!(
            sent_coords(&sent).unwrap(),
            ("C1".to_string(), "1.1".to_string())
        );
    }

    #[test]
    fn sent_coords_refuses_a_ref_with_no_channel() {
        let sent = SentRef {
            channel: ID.clone(),
            message_id: MessageId("1.1".to_string()),
            raw: serde_json::json!({}),
        };
        assert!(sent_coords(&sent).is_err());
    }

    #[tokio::test]
    async fn stored_thread_surfaces_preserve_audience_and_thread_context() {
        let handle = SlackHandle::new(
            InstanceId::new(),
            AgentId::from_slug("dev"),
            ChannelIdentity::new("bot", "bot"),
            Arc::new(SlackApi::new("token").unwrap()),
        );
        let thread = |external| ThreadId::new(ID.clone(), handle.instance, external);

        assert_eq!(handle.surface(&thread("c:D1")).await.unwrap(), Surface::Dm);
        assert_eq!(
            handle.surface(&thread("c:D1:t:1.1")).await.unwrap(),
            Surface::Dm
        );
        assert_eq!(
            handle.surface(&thread("c:C1")).await.unwrap(),
            Surface::Channel
        );
        assert_eq!(
            handle.surface(&thread("c:C1:t:1.1")).await.unwrap(),
            Surface::Thread
        );
    }

    #[tokio::test]
    async fn malformed_stored_thread_surface_is_an_error() {
        let handle = SlackHandle::new(
            InstanceId::new(),
            AgentId::from_slug("dev"),
            ChannelIdentity::new("bot", "bot"),
            Arc::new(SlackApi::new("token").unwrap()),
        );
        let stored_thread = ThreadId::new(ID.clone(), handle.instance, "unknown");
        assert!(handle.surface(&stored_thread).await.is_err());
    }

    #[test]
    fn outgoing_text_converts_to_mrkdwn() {
        let text =
            outgoing_text(OutgoingBody::Text("**bold** [a](https://x)".to_string())).unwrap();
        assert_eq!(text, "*bold* <https://x|a>");
    }

    #[test]
    fn files_and_reactions_are_declined_rather_than_dropped() {
        let file = OutgoingBody::File(Attachment {
            mime: "text/plain".to_string(),
            name: Some("a.txt".to_string()),
            size: 3,
            source: goat_types::AttachmentSource::Url("https://x/a.txt".to_string()),
        });
        assert!(matches!(
            outgoing_text(file),
            Err(ChannelError::Unsupported(_))
        ));
        let reaction = OutgoingBody::Reaction {
            target: MessageId("1.1".to_string()),
            emoji: "eyes".to_string(),
        };
        assert!(matches!(
            outgoing_text(reaction),
            Err(ChannelError::Unsupported(_))
        ));
    }
}
