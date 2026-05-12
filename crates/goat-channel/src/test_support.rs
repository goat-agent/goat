use std::sync::Arc;

use async_trait::async_trait;
use goat_types::{ChannelId, ConversationId, InstanceId, MessageId, OutgoingBody, PersonaId};
use tokio::sync::Mutex;

use crate::{
    ChannelCapabilities, ChannelHandle, ChannelIdentity, ChannelResult, SentRef, TypingGuard,
};

#[derive(Clone, Debug)]
pub enum MockEvent {
    Send {
        conv: ConversationId,
        body: OutgoingBody,
        reply_to: Option<MessageId>,
        sent_id: MessageId,
    },
    Edit {
        sent: SentRef,
        body: OutgoingBody,
    },
    Typing {
        conv: ConversationId,
    },
}

pub struct MockChannelHandle {
    id: ChannelId,
    persona: PersonaId,
    instance: InstanceId,
    identity: ChannelIdentity,
    capabilities: ChannelCapabilities,
    events: Mutex<Vec<MockEvent>>,
    next_id: Mutex<u64>,
}

impl MockChannelHandle {
    pub fn new(
        id: ChannelId,
        persona: PersonaId,
        instance: InstanceId,
        identity: ChannelIdentity,
        capabilities: ChannelCapabilities,
    ) -> Arc<Self> {
        Arc::new(Self {
            id,
            persona,
            instance,
            identity,
            capabilities,
            events: Mutex::new(Vec::new()),
            next_id: Mutex::new(0),
        })
    }

    pub async fn events(&self) -> Vec<MockEvent> {
        self.events.lock().await.clone()
    }
}

#[async_trait]
impl ChannelHandle for MockChannelHandle {
    fn instance(&self) -> InstanceId {
        self.instance
    }
    fn persona(&self) -> PersonaId {
        self.persona
    }
    fn id(&self) -> ChannelId {
        self.id.clone()
    }
    fn identity(&self) -> ChannelIdentity {
        self.identity.clone()
    }
    fn capabilities(&self) -> ChannelCapabilities {
        self.capabilities
    }

    async fn send(
        &self,
        conv: &ConversationId,
        body: OutgoingBody,
        reply_to: Option<MessageId>,
    ) -> ChannelResult<SentRef> {
        let id = {
            let mut next = self.next_id.lock().await;
            *next += 1;
            MessageId(format!("mock-{}", *next))
        };
        self.events.lock().await.push(MockEvent::Send {
            conv: conv.clone(),
            body: body.clone(),
            reply_to,
            sent_id: id.clone(),
        });
        Ok(SentRef {
            channel: self.id.clone(),
            message_id: id.clone(),
            raw: serde_json::json!({ "mock_id": id.0 }),
        })
    }

    async fn edit(&self, sent: &SentRef, body: OutgoingBody) -> ChannelResult<()> {
        self.events.lock().await.push(MockEvent::Edit {
            sent: sent.clone(),
            body,
        });
        Ok(())
    }

    async fn typing(&self, conv: &ConversationId) -> ChannelResult<TypingGuard> {
        self.events
            .lock()
            .await
            .push(MockEvent::Typing { conv: conv.clone() });
        Ok(TypingGuard::noop())
    }
}

impl MockEvent {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            MockEvent::Send {
                body: OutgoingBody::Text(t),
                ..
            } => Some(t.as_str()),
            MockEvent::Edit {
                body: OutgoingBody::Text(t),
                ..
            } => Some(t.as_str()),
            _ => None,
        }
    }
}
