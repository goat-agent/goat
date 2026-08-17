use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use goat_api::{BrowserCommand, BrowserEvent, BrowserEventParams, CdpEvent, Empty, Holder, Router};
use goat_capability::{Broker, DEFAULT_CALL_DEADLINE};
use goat_tool_browser::{BrowserError, Transport, TransportFuture};
use tokio::sync::broadcast;

const EVENT_QUEUE: usize = 256;
pub const CAPABILITY: &str = "host.browser";

#[derive(Default)]
pub struct BrowserEvents {
    lanes: Mutex<HashMap<Holder, broadcast::Sender<CdpEvent>>>,
}

impl BrowserEvents {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe(&self, holder: &Holder) -> broadcast::Receiver<CdpEvent> {
        let mut lanes = self
            .lanes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        lanes
            .entry(holder.clone())
            .or_insert_with(|| broadcast::channel(EVENT_QUEUE).0)
            .subscribe()
    }

    fn publish(&self, holder: &Holder, event: &CdpEvent) {
        let mut lanes = self
            .lanes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(lane) = lanes.get(holder) else {
            return;
        };
        if lane.send(event.clone()).is_err() {
            lanes.remove(holder);
        }
    }
}

pub struct BrowserRelay {
    broker: Arc<Broker>,
    events: Arc<BrowserEvents>,
    holder: Holder,
}

impl BrowserRelay {
    #[must_use]
    pub fn new(broker: Arc<Broker>, events: Arc<BrowserEvents>, holder: Holder) -> Self {
        Self {
            broker,
            events,
            holder,
        }
    }
}

impl Transport for BrowserRelay {
    fn call(&self, command: BrowserCommand) -> TransportFuture<'_> {
        Box::pin(async move {
            let params = serde_json::to_value(&command)
                .map_err(|err| BrowserError::Message(err.to_string()))?;
            let value = self
                .broker
                .invoke(&self.holder, CAPABILITY, params, DEFAULT_CALL_DEADLINE)
                .await
                .map_err(|err| BrowserError::Message(err.message))?;
            serde_json::from_value(value).map_err(|err| BrowserError::Message(err.to_string()))
        })
    }

    fn events(&self) -> broadcast::Receiver<CdpEvent> {
        self.events.subscribe(&self.holder)
    }
}

#[must_use]
pub fn routes(router: Router, broker: Arc<Broker>, events: Arc<BrowserEvents>) -> Router {
    router.unary::<BrowserEvent, _, _>(move |params: BrowserEventParams, _ctx| {
        let broker = broker.clone();
        let events = events.clone();
        async move {
            for holder in broker.holders(&params.instance, CAPABILITY).await {
                events.publish(&holder, &params.event);
            }
            Ok(Empty {})
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{BrowserEvents, BrowserRelay, CAPABILITY};
    use goat_api::{BrowserCommand, CdpEvent, Holder, HostBrowserOutput, SessionId};
    use goat_capability::{Broker, Caller, ProviderId, Registration};
    use goat_tool_browser::Transport;
    use goat_wire::envelope::{CallError, ErrorCode};
    use serde_json::{Value, json};
    use std::sync::Arc;
    use std::time::Duration;

    struct Echo;

    #[async_trait::async_trait]
    impl Caller for Echo {
        async fn call(
            &self,
            _method: &str,
            _version: u16,
            params: Value,
            _deadline: Duration,
        ) -> Result<Value, CallError> {
            let command: BrowserCommand = serde_json::from_value(params).unwrap();
            let BrowserCommand::Cdp { method, .. } = command else {
                return Err(CallError::new(ErrorCode::Internal, "unexpected command"));
            };
            Ok(json!({ "reply": "cdp", "result": { "did": method } }))
        }

        fn is_connected(&self) -> bool {
            true
        }
    }

    async fn bound(broker: &Broker, holder: &Holder) -> String {
        let id = ProviderId::new("local", "laptop");
        broker
            .register(Registration {
                id: id.clone(),
                label: "chrome".to_owned(),
                capability: CAPABILITY.to_owned(),
                version: 1,
                boot_epoch: 1,
                max_in_flight: 8,
                caller: Arc::new(Echo),
            })
            .await;
        broker.bind(holder, CAPABILITY, &id).await.unwrap();
        id.instance
    }

    #[tokio::test]
    async fn a_command_reaches_the_provider_and_the_reply_is_typed() {
        let broker = Arc::new(Broker::new());
        let events = Arc::new(BrowserEvents::new());
        let holder = Holder::session(SessionId(1));
        bound(&broker, &holder).await;

        let relay = BrowserRelay::new(broker, events, holder);
        let output = relay
            .call(BrowserCommand::Cdp {
                method: "Page.navigate".to_owned(),
                params: json!({}),
            })
            .await
            .unwrap();
        assert_eq!(
            output,
            HostBrowserOutput::Cdp {
                result: json!({ "did": "Page.navigate" }),
            }
        );
    }

    #[tokio::test]
    async fn an_event_reaches_every_holder_bound_to_that_provider() {
        let broker = Arc::new(Broker::new());
        let events = Arc::new(BrowserEvents::new());
        let one = Holder::session(SessionId(1));
        let two = Holder::agent("scout");
        let instance = bound(&broker, &one).await;
        broker
            .bind(
                &two,
                CAPABILITY,
                &ProviderId::new("local", instance.clone()),
            )
            .await
            .unwrap();

        let mut first = events.subscribe(&one);
        let mut second = events.subscribe(&two);
        let event = CdpEvent {
            method: "Page.loadEventFired".to_owned(),
            params: json!({}),
        };
        for holder in broker.holders(&instance, CAPABILITY).await {
            events.publish(&holder, &event);
        }

        assert_eq!(first.recv().await.unwrap().method, "Page.loadEventFired");
        assert_eq!(second.recv().await.unwrap().method, "Page.loadEventFired");
    }

    #[tokio::test]
    async fn a_holder_bound_elsewhere_hears_nothing() {
        let broker = Arc::new(Broker::new());
        let events = Arc::new(BrowserEvents::new());
        let mine = Holder::session(SessionId(1));
        let theirs = Holder::agent("scout");
        let instance = bound(&broker, &mine).await;

        let other = ProviderId::new("desk", "workstation");
        broker
            .register(Registration {
                id: other.clone(),
                label: "other chrome".to_owned(),
                capability: CAPABILITY.to_owned(),
                version: 1,
                boot_epoch: 1,
                max_in_flight: 8,
                caller: Arc::new(Echo),
            })
            .await;
        broker.bind(&theirs, CAPABILITY, &other).await.unwrap();

        let mut ours = events.subscribe(&mine);
        let mut listening = events.subscribe(&theirs);
        let event = CdpEvent {
            method: "Page.loadEventFired".to_owned(),
            params: json!({}),
        };
        for holder in broker.holders(&instance, CAPABILITY).await {
            events.publish(&holder, &event);
        }

        assert_eq!(ours.recv().await.unwrap().method, "Page.loadEventFired");
        assert!(listening.try_recv().is_err());
    }
}
