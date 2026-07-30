use std::sync::Arc;

use async_trait::async_trait;
use goat_types::{AgentId, ChannelId, InstanceId, MessageId, OutgoingBody, ThreadId};
use tokio::sync::Mutex;

use crate::{
    ChannelCapabilities, ChannelHandle, ChannelIdentity, ChannelResult, SentRef, TypingGuard,
};

#[derive(Clone, Debug)]
pub enum MockEvent {
    Send {
        conv: ThreadId,
        body: OutgoingBody,
        reply_to: Option<MessageId>,
        sent_id: MessageId,
    },
    Edit {
        sent: SentRef,
        body: OutgoingBody,
    },
    Typing {
        conv: ThreadId,
    },
    OpenThread {
        parent: ThreadId,
        anchor: Option<MessageId>,
        title: String,
    },
}

pub struct MockChannelHandle {
    id: ChannelId,
    agent: AgentId,
    instance: InstanceId,
    identity: ChannelIdentity,
    capabilities: ChannelCapabilities,
    supports_threads: bool,
    events: Mutex<Vec<MockEvent>>,
    next_id: Mutex<u64>,
}

impl MockChannelHandle {
    pub fn new(
        id: ChannelId,
        agent: AgentId,
        instance: InstanceId,
        identity: ChannelIdentity,
        capabilities: ChannelCapabilities,
    ) -> Arc<Self> {
        Self::build(id, agent, instance, identity, capabilities, false)
    }

    pub fn with_threads(
        id: ChannelId,
        agent: AgentId,
        instance: InstanceId,
        identity: ChannelIdentity,
        capabilities: ChannelCapabilities,
    ) -> Arc<Self> {
        Self::build(id, agent, instance, identity, capabilities, true)
    }

    fn build(
        id: ChannelId,
        agent: AgentId,
        instance: InstanceId,
        identity: ChannelIdentity,
        capabilities: ChannelCapabilities,
        supports_threads: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            id,
            agent,
            instance,
            identity,
            capabilities,
            supports_threads,
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
    fn agent(&self) -> AgentId {
        self.agent
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
        conv: &ThreadId,
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

    async fn typing(&self, conv: &ThreadId) -> ChannelResult<TypingGuard> {
        self.events
            .lock()
            .await
            .push(MockEvent::Typing { conv: conv.clone() });
        Ok(TypingGuard::noop())
    }

    fn supports_threads(&self) -> bool {
        self.supports_threads
    }

    async fn open_thread(
        &self,
        parent: &ThreadId,
        anchor: Option<&MessageId>,
        title: &str,
    ) -> ChannelResult<ThreadId> {
        self.events.lock().await.push(MockEvent::OpenThread {
            parent: parent.clone(),
            anchor: anchor.cloned(),
            title: title.to_string(),
        });
        Ok(ThreadId::new(
            parent.channel.clone(),
            parent.instance,
            "g:1:c:99999",
        ))
    }
}

impl MockEvent {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            MockEvent::Send {
                body: OutgoingBody::Text(t),
                ..
            }
            | MockEvent::Edit {
                body: OutgoingBody::Text(t),
                ..
            } => Some(t.as_str()),
            _ => None,
        }
    }
}
