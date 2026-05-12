use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use goat_types::PersonaId;

pub mod backoff;

pub use backoff::DecorrelatedJitter;

#[derive(Clone)]
pub struct SignalCtx {
    pub persona: PersonaId,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum SignalKind {
    PendingIntent {
        note: String,
    },
    PushEvent {
        source: String,
        payload: serde_json::Value,
    },
    RecurringPattern {
        rule: String,
    },
    PriorFollowUp {
        reason: String,
    },
    ExternalTrigger {
        kind: String,
    },
}

impl SignalKind {
    pub fn tag(&self) -> &'static str {
        match self {
            Self::PendingIntent { .. } => "pending_intent",
            Self::PushEvent { .. } => "push_event",
            Self::RecurringPattern { .. } => "recurring_pattern",
            Self::PriorFollowUp { .. } => "prior_follow_up",
            Self::ExternalTrigger { .. } => "external_trigger",
        }
    }

    pub fn canonical_reason(&self) -> String {
        match self {
            Self::PendingIntent { note } => note.clone(),
            Self::PushEvent { source, payload } => format!("{source}:{payload}"),
            Self::RecurringPattern { rule } => rule.clone(),
            Self::PriorFollowUp { reason } => reason.clone(),
            Self::ExternalTrigger { kind } => kind.clone(),
        }
    }
}

#[async_trait]
pub trait SignalSource: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    async fn poll(&self, ctx: &SignalCtx) -> Option<SignalKind>;
    fn next_hint(&self) -> Duration;
}

pub fn idempotency_key(persona: PersonaId, signal: &SignalKind) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(persona.0.as_bytes());
    hasher.update(signal.tag().as_bytes());
    hasher.update(b"\0");
    hasher.update(signal.canonical_reason().as_bytes());
    *hasher.finalize().as_bytes()
}

pub struct NullSource(pub &'static str);

#[async_trait]
impl SignalSource for NullSource {
    fn name(&self) -> &'static str {
        self.0
    }
    async fn poll(&self, _ctx: &SignalCtx) -> Option<SignalKind> {
        None
    }
    fn next_hint(&self) -> Duration {
        Duration::from_secs(3600)
    }
}

pub struct PersonaTick {
    pub persona: PersonaId,
    pub sources: Vec<Arc<dyn SignalSource>>,
    pub backoff: DecorrelatedJitter,
}
