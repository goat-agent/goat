use goat_types::{AgentId, Event};
use tokio::sync::broadcast;
use tracing::warn;

const BUS_CAPACITY: usize = 1024;

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Event>,
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BUS_CAPACITY);
        Self { tx }
    }

    pub fn publish(&self, event: Event) {
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self, filter: EventFilter) -> EventSubscription {
        EventSubscription {
            rx: self.tx.subscribe(),
            filter,
        }
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug)]
pub enum EventFilter {
    Agent(AgentId),
    IncomingFor(AgentId),
}

pub struct EventSubscription {
    rx: broadcast::Receiver<Event>,
    filter: EventFilter,
}

impl EventSubscription {
    pub async fn recv(&mut self) -> Option<Event> {
        loop {
            match self.rx.recv().await {
                Ok(ev) if self.matches(&ev) => return Some(ev),
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(
                        dropped = n,
                        filter = ?self.filter,
                        "event bus subscriber lagged; events were dropped"
                    );
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }

    fn matches(&self, ev: &Event) -> bool {
        match &self.filter {
            EventFilter::Agent(p) => ev.agent() == *p,
            EventFilter::IncomingFor(p) => {
                matches!(ev, Event::Incoming(m) if m.agent == *p)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use goat_types::{
        ChannelId, ConversationId, IncomingMessage, InstanceId, MessageId, Surface, UserHandle,
    };

    fn mk_in(agent: AgentId) -> Event {
        Event::Incoming(IncomingMessage {
            id: MessageId("m".into()),
            agent,
            conversation: ConversationId::new(ChannelId::new("discord"), InstanceId::new(), "x"),
            from: UserHandle {
                external: "u".into(),
                display: None,
            },
            text: "hi".into(),
            attachments: vec![],
            command: None,
            surface: Surface::Dm,
            addressed: true,
            parent: None,
            ts: Utc::now(),
            raw: serde_json::Value::Null,
        })
    }

    #[tokio::test]
    async fn filter_agent_passes_only_matching() {
        let bus = EventBus::new();
        let p = AgentId::new();
        let other = AgentId::new();
        let mut sub = bus.subscribe(EventFilter::Agent(p));
        bus.publish(mk_in(other));
        bus.publish(mk_in(p));
        let got = sub.recv().await.expect("at least one event");
        assert_eq!(got.agent(), p);
    }
}
