use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use goat_api::AgentMessage;
use goat_channel::{
    BindOutput, Channel, ChannelBinding, ChannelCapabilities, ChannelError, ChannelFactory,
    ChannelHandle, ChannelIdentity, ChannelMetadata, ChannelResult, ChannelSecrets, SentRef,
    TypingGuard,
};
use goat_types::{
    AgentId, ChannelId, ConversationId, IncomingMessage, InstanceId, MessageId, OutgoingBody,
    Surface, UserHandle,
};
use parking_lot::Mutex;
use serde_json::{Value, json};
use thiserror::Error;
use tokio::sync::{broadcast, mpsc, oneshot};
use uuid::Uuid;

pub const ID: ChannelId = ChannelId::from_static("desktop");

pub const DEFAULT_CONVERSATION: &str = "main";

#[derive(Clone, Debug)]
pub enum Outbound {
    Created {
        conversation: String,
        message: AgentMessage,
    },
    Updated {
        conversation: String,
        id: String,
        text: String,
    },
    Typing {
        conversation: String,
        active: bool,
    },
}

impl Outbound {
    #[must_use]
    pub fn conversation(&self) -> &str {
        match self {
            Self::Created { conversation, .. }
            | Self::Updated { conversation, .. }
            | Self::Typing { conversation, .. } => conversation,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum HubError {
    #[error("agent has no desktop channel")]
    UnknownAgent,
    #[error("desktop channel is closed")]
    Closed,
    #[error("desktop channel is full")]
    Full,
}

struct Registered {
    instance: InstanceId,
    tx: mpsc::Sender<IncomingMessage>,
    outbound: broadcast::Sender<Outbound>,
    conversations: HashMap<String, VecDeque<AgentMessage>>,
}

impl Registered {
    fn publish(&mut self, event: Outbound) {
        let cached = self
            .conversations
            .entry(event.conversation().to_owned())
            .or_default();
        match &event {
            Outbound::Created { message, .. } => {
                if cached.len() == 200 {
                    cached.pop_front();
                }
                cached.push_back(message.clone());
            }
            Outbound::Updated { id, text, .. } => {
                if let Some(message) = cached.iter_mut().find(|message| message.id == *id) {
                    message.text.clone_from(text);
                }
            }
            Outbound::Typing { .. } => {}
        }
        let _ = self.outbound.send(event);
    }
}

#[derive(Default)]
pub struct Hub {
    agents: Mutex<HashMap<AgentId, Registered>>,
}

#[must_use]
pub fn hub() -> &'static Hub {
    static HUB: LazyLock<Hub> = LazyLock::new(Hub::default);
    &HUB
}

impl Hub {
    pub fn send(
        &self,
        agent: AgentId,
        conversation: &str,
        text: String,
    ) -> Result<MessageId, HubError> {
        let mut agents = self.agents.lock();
        let registered = agents.get_mut(&agent).ok_or(HubError::UnknownAgent)?;
        let id = MessageId(Uuid::new_v4().to_string());
        let ts = Utc::now();
        registered
            .tx
            .try_send(IncomingMessage {
                id: id.clone(),
                agent,
                conversation: ConversationId::new(ID, registered.instance, conversation),
                from: UserHandle {
                    external: "desktop".into(),
                    display: Some("You".into()),
                },
                text: text.clone(),
                attachments: Vec::new(),
                command: None,
                surface: Surface::Dm,
                addressed: true,
                parent: None,
                ts,
                raw: json!({}),
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => HubError::Full,
                mpsc::error::TrySendError::Closed(_) => HubError::Closed,
            })?;
        registered.publish(Outbound::Created {
            conversation: conversation.to_owned(),
            message: AgentMessage {
                id: id.0.clone(),
                outgoing: false,
                text,
                ts: ts.to_rfc3339(),
                reply_to: None,
            },
        });
        Ok(id)
    }

    #[must_use]
    pub fn subscribe(&self, agent: AgentId) -> Option<broadcast::Receiver<Outbound>> {
        self.agents
            .lock()
            .get(&agent)
            .map(|registered| registered.outbound.subscribe())
    }

    #[must_use]
    pub fn conversation(&self, agent: AgentId, external: &str) -> Option<ConversationId> {
        self.agents
            .lock()
            .get(&agent)
            .map(|registered| ConversationId::new(ID, registered.instance, external))
    }

    #[must_use]
    pub fn messages(&self, agent: AgentId, external: &str) -> Option<Vec<AgentMessage>> {
        self.agents.lock().get(&agent).map(|registered| {
            registered
                .conversations
                .get(external)
                .map(|cached| cached.iter().cloned().collect())
                .unwrap_or_default()
        })
    }

    #[must_use]
    pub fn conversations(&self, agent: AgentId) -> Option<Vec<String>> {
        self.agents
            .lock()
            .get(&agent)
            .map(|registered| registered.conversations.keys().cloned().collect())
    }
}

pub struct DesktopChannel;

fn metadata() -> ChannelMetadata {
    ChannelMetadata::new(
        "Desktop",
        "Chat from the goat-desktop app; needs no setup.",
        &[],
    )
}

fn validate_config(value: &Value) -> ChannelResult<()> {
    if value.is_object() {
        Ok(())
    } else {
        Err(ChannelError::Config("desktop: expected an object".into()))
    }
}

inventory::submit! {
    ChannelFactory { id: ID, ctor: || Arc::new(DesktopChannel), validate_config, metadata }
}

#[async_trait]
impl Channel for DesktopChannel {
    fn id(&self) -> ChannelId {
        ID
    }

    async fn bind(
        self: Arc<Self>,
        agent: AgentId,
        binding: ChannelBinding,
    ) -> ChannelResult<BindOutput> {
        validate_config(&binding.config)?;
        let (tx, rx) = mpsc::channel(64);
        let (outbound, _) = broadcast::channel(256);
        let handle = Arc::new(DesktopHandle {
            agent,
            instance: binding.instance,
            outbound: outbound.clone(),
        });
        hub().agents.lock().insert(
            agent,
            Registered {
                instance: binding.instance,
                tx,
                outbound,
                conversations: HashMap::new(),
            },
        );
        Ok((handle, rx))
    }

    async fn verify(
        &self,
        config: &Value,
        _secrets: &ChannelSecrets,
    ) -> ChannelResult<ChannelIdentity> {
        validate_config(config)?;
        Ok(ChannelIdentity::new("goat", "goat"))
    }
}

struct DesktopHandle {
    agent: AgentId,
    instance: InstanceId,
    outbound: broadcast::Sender<Outbound>,
}

impl DesktopHandle {
    fn publish(&self, event: Outbound) -> ChannelResult<()> {
        let mut agents = hub().agents.lock();
        let registered = agents
            .get_mut(&self.agent)
            .filter(|registered| registered.outbound.same_channel(&self.outbound))
            .ok_or_else(|| ChannelError::Provider("desktop binding was replaced".into()))?;
        registered.publish(event);
        Ok(())
    }
}

impl Drop for DesktopHandle {
    fn drop(&mut self) {
        let mut agents = hub().agents.lock();
        if agents
            .get(&self.agent)
            .is_some_and(|registered| registered.outbound.same_channel(&self.outbound))
        {
            agents.remove(&self.agent);
        }
    }
}

struct StopTyping(broadcast::Sender<Outbound>, String);

impl Drop for StopTyping {
    fn drop(&mut self) {
        let _ = self.0.send(Outbound::Typing {
            conversation: std::mem::take(&mut self.1),
            active: false,
        });
    }
}

#[async_trait]
impl ChannelHandle for DesktopHandle {
    fn instance(&self) -> InstanceId {
        self.instance
    }

    fn agent(&self) -> AgentId {
        self.agent
    }

    fn id(&self) -> ChannelId {
        ID
    }

    fn identity(&self) -> ChannelIdentity {
        ChannelIdentity::new("goat", "goat")
    }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities::new(usize::MAX / 2, Duration::from_millis(50), None)
    }

    async fn surface(&self, _conversation: &ConversationId) -> ChannelResult<Surface> {
        Ok(Surface::Dm)
    }

    async fn send(
        &self,
        conv: &ConversationId,
        body: OutgoingBody,
        reply_to: Option<MessageId>,
    ) -> ChannelResult<SentRef> {
        let OutgoingBody::Text(text) = body else {
            return Err(ChannelError::Unsupported("desktop carries text only"));
        };
        let message_id = MessageId(Uuid::new_v4().to_string());
        self.publish(Outbound::Created {
            conversation: conv.external.clone(),
            message: AgentMessage {
                id: message_id.0.clone(),
                outgoing: true,
                text,
                ts: Utc::now().to_rfc3339(),
                reply_to: reply_to.map(|id| id.0),
            },
        })?;
        Ok(SentRef {
            channel: ID,
            message_id,
            raw: json!({ "conversation": conv.external }),
        })
    }

    async fn edit(&self, sent: &SentRef, body: OutgoingBody) -> ChannelResult<()> {
        let OutgoingBody::Text(text) = body else {
            return Err(ChannelError::Unsupported("desktop carries text only"));
        };
        let conversation = sent.raw["conversation"]
            .as_str()
            .unwrap_or(DEFAULT_CONVERSATION)
            .to_owned();
        self.publish(Outbound::Updated {
            conversation,
            id: sent.message_id.0.clone(),
            text,
        })
    }

    async fn typing(&self, conv: &ConversationId) -> ChannelResult<TypingGuard> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|error| ChannelError::Provider(error.to_string()))?;
        let (stop_tx, stop_rx) = oneshot::channel();
        self.publish(Outbound::Typing {
            conversation: conv.external.clone(),
            active: true,
        })?;
        let stop = StopTyping(self.outbound.clone(), conv.external.clone());
        runtime.spawn(async move {
            let _ = stop_rx.await;
            drop(stop);
        });
        Ok(TypingGuard::new(stop_tx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(instance: InstanceId) -> ChannelBinding {
        ChannelBinding {
            instance,
            config: json!({}),
            commands: Vec::new(),
            secrets: ChannelSecrets::default(),
        }
    }

    async fn bind(agent: AgentId) -> BindOutput {
        Arc::new(DesktopChannel)
            .bind(agent, binding(InstanceId::new()))
            .await
            .unwrap()
    }

    #[test]
    fn unknown_agent_cannot_send_or_subscribe() {
        let agent = AgentId::new();
        assert_eq!(
            hub().send(agent, DEFAULT_CONVERSATION, "hello".into()),
            Err(HubError::UnknownAgent)
        );
        assert!(hub().subscribe(agent).is_none());
        assert!(hub().conversation(agent, DEFAULT_CONVERSATION).is_none());
        assert!(hub().messages(agent, DEFAULT_CONVERSATION).is_none());
    }

    #[tokio::test]
    async fn empty_binding_delivers_inbound_to_agent_and_all_clients() {
        let agent = AgentId::new();
        let (handle, mut incoming) = bind(agent).await;
        let mut first = hub().subscribe(agent).unwrap();
        let mut second = hub().subscribe(agent).unwrap();
        let id = hub()
            .send(agent, DEFAULT_CONVERSATION, "hello".into())
            .unwrap();
        let delivered = incoming.recv().await.unwrap();
        assert_eq!(delivered.id, id);
        assert_eq!(delivered.agent, agent);
        assert_eq!(
            delivered.conversation,
            hub().conversation(agent, DEFAULT_CONVERSATION).unwrap()
        );
        assert_eq!(delivered.text, "hello");
        assert_eq!(delivered.surface, Surface::Dm);
        assert!(delivered.addressed);
        for client in [&mut first, &mut second] {
            let Outbound::Created { message, .. } = client.recv().await.unwrap() else {
                panic!("expected inbound creation");
            };
            assert_eq!(message.id, id.0);
            assert_eq!(message.text, delivered.text);
            assert!(!message.outgoing);
        }
        let messages = hub().messages(agent, DEFAULT_CONVERSATION).unwrap();
        assert_eq!(messages[0].id, id.0);
        assert_eq!(messages[0].text, "hello");
        assert!(!messages[0].outgoing);
        drop(handle);
        assert_eq!(
            hub().send(agent, DEFAULT_CONVERSATION, "after drop".into()),
            Err(HubError::UnknownAgent)
        );
    }

    #[tokio::test]
    async fn conversations_keep_separate_histories_on_one_binding() {
        let agent = AgentId::new();
        let (handle, mut incoming) = bind(agent).await;
        hub().send(agent, "left", "first".into()).unwrap();
        hub().send(agent, "right", "second".into()).unwrap();
        let left = incoming.recv().await.unwrap();
        let right = incoming.recv().await.unwrap();
        assert_eq!(left.conversation.external, "left");
        assert_eq!(right.conversation.external, "right");
        assert_ne!(left.conversation, right.conversation);

        handle
            .send(
                &hub().conversation(agent, "left").unwrap(),
                OutgoingBody::Text("reply".into()),
                None,
            )
            .await
            .unwrap();

        let texts = |external: &str| {
            hub()
                .messages(agent, external)
                .unwrap()
                .into_iter()
                .map(|message| message.text)
                .collect::<Vec<_>>()
        };
        assert_eq!(texts("left"), ["first", "reply"]);
        assert_eq!(texts("right"), ["second"]);
        assert!(texts("never-used").is_empty());

        let mut listed = hub().conversations(agent).unwrap();
        listed.sort();
        assert_eq!(listed, ["left", "right"]);
    }

    #[tokio::test]
    async fn editing_updates_only_its_own_conversation() {
        let agent = AgentId::new();
        let (handle, _incoming) = bind(agent).await;
        let sent = handle
            .send(
                &hub().conversation(agent, "left").unwrap(),
                OutgoingBody::Text("draft".into()),
                None,
            )
            .await
            .unwrap();
        handle
            .send(
                &hub().conversation(agent, "right").unwrap(),
                OutgoingBody::Text("other".into()),
                None,
            )
            .await
            .unwrap();
        handle
            .edit(&sent, OutgoingBody::Text("finished".into()))
            .await
            .unwrap();
        assert_eq!(hub().messages(agent, "left").unwrap()[0].text, "finished");
        assert_eq!(hub().messages(agent, "right").unwrap()[0].text, "other");
    }

    #[tokio::test]
    async fn rejected_incoming_is_not_broadcast() {
        let agent = AgentId::new();
        let (_handle, incoming) = bind(agent).await;
        let mut events = hub().subscribe(agent).unwrap();
        let mut accepted = Vec::new();
        for _ in 0..64 {
            accepted.push(
                hub()
                    .send(agent, DEFAULT_CONVERSATION, "queued".into())
                    .unwrap()
                    .0,
            );
            assert!(matches!(events.try_recv(), Ok(Outbound::Created { .. })));
        }
        assert_eq!(
            hub().send(agent, DEFAULT_CONVERSATION, "overflow".into()),
            Err(HubError::Full)
        );
        assert!(matches!(
            events.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        drop(incoming);
        assert_eq!(
            hub().send(agent, DEFAULT_CONVERSATION, "closed".into()),
            Err(HubError::Closed)
        );
        assert!(matches!(
            events.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        assert_eq!(
            hub()
                .messages(agent, DEFAULT_CONVERSATION)
                .unwrap()
                .into_iter()
                .map(|message| message.id)
                .collect::<Vec<_>>(),
            accepted
        );
    }

    #[tokio::test]
    async fn outgoing_send_and_edit_share_message_id() {
        let agent = AgentId::new();
        let (handle, _incoming) = bind(agent).await;
        let conversation = hub().conversation(agent, DEFAULT_CONVERSATION).unwrap();
        let mut events = hub().subscribe(agent).unwrap();
        let sent = handle
            .send(
                &conversation,
                OutgoingBody::Text("draft".into()),
                Some(MessageId("parent".into())),
            )
            .await
            .unwrap();
        handle
            .edit(&sent, OutgoingBody::Text("finished".into()))
            .await
            .unwrap();
        let Outbound::Created { message, .. } = events.recv().await.unwrap() else {
            panic!("expected outgoing creation");
        };
        assert_eq!(message.id, sent.message_id.0);
        assert_eq!(message.text, "draft");
        assert!(message.outgoing);
        assert_eq!(message.reply_to.as_deref(), Some("parent"));
        let Outbound::Updated { id, text, .. } = events.recv().await.unwrap() else {
            panic!("expected outgoing edit");
        };
        assert_eq!(id, message.id);
        assert_eq!(text, "finished");
        let messages = hub().messages(agent, DEFAULT_CONVERSATION).unwrap();
        assert_eq!(messages[0].id, id);
        assert_eq!(messages[0].text, "finished");
        assert!(messages[0].outgoing);
        let reaction = OutgoingBody::Reaction {
            target: sent.message_id.clone(),
            emoji: String::new(),
        };
        assert!(matches!(
            handle.send(&conversation, reaction.clone(), None).await,
            Err(ChannelError::Unsupported(_))
        ));
        assert!(matches!(
            handle.edit(&sent, reaction).await,
            Err(ChannelError::Unsupported(_))
        ));
        assert!(matches!(
            events.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn reconnect_history_retains_latest_two_hundred_messages() {
        let agent = AgentId::new();
        let (handle, _incoming) = bind(agent).await;
        let conversation = hub().conversation(agent, DEFAULT_CONVERSATION).unwrap();
        let mut expected = Vec::new();
        for index in 0..201 {
            let sent = handle
                .send(&conversation, OutgoingBody::Text(index.to_string()), None)
                .await
                .unwrap();
            if index > 0 {
                expected.push(sent.message_id.0);
            }
        }
        assert_eq!(
            hub()
                .messages(agent, DEFAULT_CONVERSATION)
                .unwrap()
                .into_iter()
                .map(|message| message.id)
                .collect::<Vec<_>>(),
            expected
        );
    }

    #[tokio::test]
    async fn dropping_replaced_handle_preserves_same_instance_registration() {
        let agent = AgentId::new();
        let instance = InstanceId::new();
        let (old, mut old_incoming) = Arc::new(DesktopChannel)
            .bind(agent, binding(instance))
            .await
            .unwrap();
        let (current, mut incoming) = Arc::new(DesktopChannel)
            .bind(agent, binding(instance))
            .await
            .unwrap();
        let conversation = hub().conversation(agent, DEFAULT_CONVERSATION).unwrap();
        assert!(matches!(
            old.send(&conversation, OutgoingBody::Text("stale".into()), None)
                .await,
            Err(ChannelError::Provider(_))
        ));
        drop(old);
        assert!(old_incoming.recv().await.is_none());
        let id = hub()
            .send(agent, DEFAULT_CONVERSATION, "replacement".into())
            .unwrap();
        assert_eq!(incoming.recv().await.unwrap().id, id);
        drop(current);
        assert!(hub().conversation(agent, DEFAULT_CONVERSATION).is_none());
    }

    #[test]
    fn typing_guard_can_drop_outside_runtime_context() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let agent = AgentId::new();
        let (handle, _incoming) = runtime.block_on(bind(agent));
        let conversation = hub().conversation(agent, DEFAULT_CONVERSATION).unwrap();
        let mut events = hub().subscribe(agent).unwrap();
        let guard = runtime.block_on(handle.typing(&conversation)).unwrap();
        assert!(matches!(
            events.try_recv(),
            Ok(Outbound::Typing { active: true, .. })
        ));
        drop(guard);
        assert!(matches!(
            runtime.block_on(events.recv()),
            Ok(Outbound::Typing { active: false, .. })
        ));
    }

    #[test]
    fn runtime_shutdown_stops_typing_even_before_waiter_is_polled() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let agent = AgentId::new();
        let (handle, _incoming) = runtime.block_on(bind(agent));
        let conversation = hub().conversation(agent, DEFAULT_CONVERSATION).unwrap();
        let mut events = hub().subscribe(agent).unwrap();
        let guard = runtime.block_on(handle.typing(&conversation)).unwrap();
        assert!(matches!(
            events.try_recv(),
            Ok(Outbound::Typing { active: true, .. })
        ));
        drop(runtime);
        assert!(matches!(
            events.try_recv(),
            Ok(Outbound::Typing { active: false, .. })
        ));
        drop(guard);
    }

    #[tokio::test]
    async fn factory_accepts_objects_and_rejects_other_config_shapes() {
        let factory = goat_channel::factory_for("desktop").unwrap();
        (factory.validate_config)(&json!({ "extra": true })).unwrap();
        for value in [Value::Null, json!([]), json!("desktop")] {
            assert!(matches!(
                (factory.validate_config)(&value),
                Err(ChannelError::Config(_))
            ));
            assert!(matches!(
                (factory.ctor)()
                    .bind(
                        AgentId::new(),
                        ChannelBinding {
                            config: value,
                            ..binding(InstanceId::new())
                        }
                    )
                    .await,
                Err(ChannelError::Config(_))
            ));
        }
    }
}
