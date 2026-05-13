use std::collections::HashMap;
use std::time::{Duration, Instant};

use goat_types::MessageId;
use tokio::sync::Mutex;
use twilight_model::id::marker::{ApplicationMarker, ChannelMarker};
use twilight_model::id::Id;

const INTERACTION_TTL: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Debug)]
pub(crate) struct PendingInteraction {
    pub application_id: Id<ApplicationMarker>,
    pub token: String,
    pub channel_id: Id<ChannelMarker>,
}

#[derive(Clone, Debug)]
pub(crate) struct InteractionResponseRef {
    pub application_id: Id<ApplicationMarker>,
    pub token: String,
}

#[derive(Clone, Debug)]
struct Timed<T> {
    value: T,
    created_at: Instant,
}

#[derive(Default)]
pub(crate) struct InteractionState {
    pending: Mutex<HashMap<MessageId, Timed<PendingInteraction>>>,
    responses: Mutex<HashMap<MessageId, Timed<InteractionResponseRef>>>,
}

impl InteractionState {
    pub async fn insert_pending(&self, interaction_id: MessageId, pending: PendingInteraction) {
        let mut pending_map = self.pending.lock().await;
        prune_expired(&mut pending_map);
        pending_map.insert(interaction_id, Timed::new(pending));
    }

    pub async fn has_pending(&self, interaction_id: &MessageId) -> bool {
        let mut pending_map = self.pending.lock().await;
        prune_expired(&mut pending_map);
        pending_map.contains_key(interaction_id)
    }

    pub async fn take_pending(&self, interaction_id: &MessageId) -> Option<PendingInteraction> {
        let mut pending_map = self.pending.lock().await;
        prune_expired(&mut pending_map);
        pending_map.remove(interaction_id).map(|timed| timed.value)
    }

    pub async fn insert_response(&self, message_id: MessageId, response: InteractionResponseRef) {
        let mut response_map = self.responses.lock().await;
        prune_expired(&mut response_map);
        response_map.insert(message_id, Timed::new(response));
    }

    pub async fn response(&self, message_id: &MessageId) -> Option<InteractionResponseRef> {
        let mut response_map = self.responses.lock().await;
        prune_expired(&mut response_map);
        response_map
            .get(message_id)
            .map(|timed| timed.value.clone())
    }
}

impl<T> Timed<T> {
    fn new(value: T) -> Self {
        Self {
            value,
            created_at: Instant::now(),
        }
    }
}

fn prune_expired<T>(map: &mut HashMap<MessageId, Timed<T>>) {
    let now = Instant::now();
    map.retain(|_, timed| now.duration_since(timed.created_at) < INTERACTION_TTL);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending() -> PendingInteraction {
        PendingInteraction {
            application_id: Id::new(123),
            token: "token".to_string(),
            channel_id: Id::new(456),
        }
    }

    #[tokio::test]
    async fn pending_interaction_is_taken_once() {
        let state = InteractionState::default();
        let id = MessageId("interaction-1".to_string());
        state.insert_pending(id.clone(), pending()).await;

        assert!(state.has_pending(&id).await);
        assert!(state.take_pending(&id).await.is_some());
        assert!(!state.has_pending(&id).await);
        assert!(state.take_pending(&id).await.is_none());
    }

    #[tokio::test]
    async fn response_ref_stays_channel_local() {
        let state = InteractionState::default();
        let id = MessageId("message-1".to_string());
        state
            .insert_response(
                id.clone(),
                InteractionResponseRef {
                    application_id: Id::new(123),
                    token: "token".to_string(),
                },
            )
            .await;

        let response = state.response(&id).await.unwrap();
        assert_eq!(response.application_id.get(), 123);
        assert_eq!(response.token, "token");
    }
}
