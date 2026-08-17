mod routes;

pub use routes::routes;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use goat_api::Holder;
use goat_wire::envelope::{CallError, ErrorCode, Execution};
use goat_wire::peer::PeerHandle;
use serde_json::Value;
use tokio::sync::Mutex;
use tokio::time::Instant;

pub const DEFAULT_LEASE_TTL: Duration = Duration::from_mins(5);
pub const DEFAULT_CALL_DEADLINE: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProviderId {
    pub device: String,
    pub instance: String,
}

impl ProviderId {
    pub fn new(device: impl Into<String>, instance: impl Into<String>) -> Self {
        Self {
            device: device.into(),
            instance: instance.into(),
        }
    }
}

impl std::fmt::Display for ProviderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.device, self.instance)
    }
}

#[async_trait::async_trait]
pub trait Caller: Send + Sync {
    async fn call(
        &self,
        method: &str,
        version: u16,
        params: Value,
        deadline: Duration,
    ) -> Result<Value, CallError>;

    fn is_connected(&self) -> bool;
}

#[async_trait::async_trait]
impl Caller for PeerHandle {
    async fn call(
        &self,
        method: &str,
        version: u16,
        params: Value,
        deadline: Duration,
    ) -> Result<Value, CallError> {
        self.call_with_deadline(method, version, params, deadline)
            .await
    }

    fn is_connected(&self) -> bool {
        true
    }
}

#[derive(Clone)]
pub struct Registration {
    pub id: ProviderId,
    pub label: String,
    pub capability: String,
    pub version: u16,
    pub boot_epoch: u64,
    pub max_in_flight: usize,
    pub caller: Arc<dyn Caller>,
}

struct Provider {
    label: String,
    version: u16,
    boot_epoch: u64,
    caller: Arc<dyn Caller>,
    in_flight: usize,
    max_in_flight: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    pub provider: ProviderId,
    pub boot_epoch: u64,
    pub expires_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseState {
    Active,
    Paused,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSummary {
    pub id: ProviderId,
    pub label: String,
    pub version: u16,
}

type Key = (String, String);
type LeaseKey = (Holder, String);

#[derive(Default)]
struct State {
    providers: HashMap<Key, Provider>,
    leases: HashMap<LeaseKey, Lease>,
}

#[derive(Default)]
pub struct Broker {
    state: Mutex<State>,
    ttl: Duration,
}

impl Broker {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State::default()),
            ttl: DEFAULT_LEASE_TTL,
        }
    }

    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            state: Mutex::new(State::default()),
            ttl,
        }
    }

    pub async fn register(&self, registration: Registration) {
        let key = (
            registration.id.instance.clone(),
            registration.capability.clone(),
        );
        let mut state = self.state.lock().await;
        state.providers.insert(
            key,
            Provider {
                label: registration.label,
                version: registration.version,
                boot_epoch: registration.boot_epoch,
                caller: registration.caller,
                in_flight: 0,
                max_in_flight: registration.max_in_flight.max(1),
            },
        );
    }

    pub async fn unregister(&self, id: &ProviderId, capability: &str) {
        let mut state = self.state.lock().await;
        state
            .providers
            .remove(&(id.instance.clone(), capability.to_owned()));
    }

    pub async fn unregister_instance(&self, instance: &str) {
        let mut state = self.state.lock().await;
        state.providers.retain(|(inst, _), _| inst != instance);
    }

    pub async fn providers(&self, capability: &str) -> Vec<ProviderSummary> {
        let state = self.state.lock().await;
        let mut out: Vec<ProviderSummary> = state
            .providers
            .iter()
            .filter(|((_, cap), _)| cap == capability)
            .map(|((instance, _), provider)| ProviderSummary {
                id: ProviderId::new(String::new(), instance.clone()),
                label: provider.label.clone(),
                version: provider.version,
            })
            .collect();
        out.sort_by(|a, b| a.id.instance.cmp(&b.id.instance));
        out
    }

    pub async fn bind(
        &self,
        holder: &Holder,
        capability: &str,
        provider: &ProviderId,
    ) -> Result<(), CallError> {
        let mut state = self.state.lock().await;
        let key = (provider.instance.clone(), capability.to_owned());
        let Some(found) = state.providers.get(&key) else {
            return Err(no_host(capability));
        };
        let boot_epoch = found.boot_epoch;
        state.leases.insert(
            (holder.clone(), capability.to_owned()),
            Lease {
                provider: provider.clone(),
                boot_epoch,
                expires_at: Instant::now() + self.ttl,
            },
        );
        Ok(())
    }

    pub async fn release(&self, holder: &Holder, capability: &str) {
        let mut state = self.state.lock().await;
        state
            .leases
            .remove(&(holder.clone(), capability.to_owned()));
    }

    pub async fn release_holder(&self, holder: &Holder) {
        let mut state = self.state.lock().await;
        state.leases.retain(|(held, _), _| held != holder);
    }

    pub async fn holders(&self, instance: &str, capability: &str) -> Vec<Holder> {
        let state = self.state.lock().await;
        state
            .leases
            .iter()
            .filter(|((_, held), lease)| held == capability && lease.provider.instance == instance)
            .map(|((holder, _), _)| holder.clone())
            .collect()
    }

    pub async fn lease_state(&self, holder: &Holder, capability: &str) -> Option<LeaseState> {
        let state = self.state.lock().await;
        let lease = state.leases.get(&(holder.clone(), capability.to_owned()))?;
        if lease.expires_at <= Instant::now() {
            return Some(LeaseState::Expired);
        }
        let key = (lease.provider.instance.clone(), capability.to_owned());
        match state.providers.get(&key) {
            Some(provider) if provider.boot_epoch == lease.boot_epoch => Some(LeaseState::Active),
            _ => Some(LeaseState::Paused),
        }
    }

    pub async fn invoke(
        &self,
        holder: &Holder,
        capability: &str,
        params: Value,
        deadline: Duration,
    ) -> Result<Value, CallError> {
        let (caller, version) = self.acquire(holder, capability).await?;
        let result = caller.call(capability, version, params, deadline).await;
        self.finish(holder, capability).await;
        result
    }

    async fn acquire(
        &self,
        holder: &Holder,
        capability: &str,
    ) -> Result<(Arc<dyn Caller>, u16), CallError> {
        let mut state = self.state.lock().await;
        let now = Instant::now();
        let lease_key = (holder.clone(), capability.to_owned());

        let instance = match state.leases.get(&lease_key) {
            Some(lease) if lease.expires_at > now => {
                let key = (lease.provider.instance.clone(), capability.to_owned());
                let epoch = lease.boot_epoch;
                match state.providers.get(&key) {
                    Some(provider) if provider.boot_epoch == epoch => {
                        lease.provider.instance.clone()
                    }
                    Some(_) => {
                        return Err(CallError::new(
                            ErrorCode::HostGone,
                            format!(
                                "this session is bound to a previous run of the {capability} provider; nothing was sent. Rebind before using it again."
                            ),
                        )
                        .with_execution(Execution::NotStarted));
                    }
                    None => {
                        return Err(CallError::new(
                            ErrorCode::HostGone,
                            format!(
                                "the {capability} provider bound to this session is disconnected; nothing was sent. Do not retry or use another provider until it reconnects or the user rebinds."
                            ),
                        )
                        .with_execution(Execution::NotStarted));
                    }
                }
            }
            _ => {
                let mut eligible: Vec<String> = state
                    .providers
                    .iter()
                    .filter(|((_, cap), _)| cap == capability)
                    .map(|((instance, _), _)| instance.clone())
                    .collect();
                eligible.sort();
                match eligible.len() {
                    0 => return Err(no_host(capability)),
                    1 => {
                        let instance = eligible.remove(0);
                        let epoch = state
                            .providers
                            .get(&(instance.clone(), capability.to_owned()))
                            .map_or(0, |p| p.boot_epoch);
                        state.leases.insert(
                            lease_key,
                            Lease {
                                provider: ProviderId::new(String::new(), instance.clone()),
                                boot_epoch: epoch,
                                expires_at: now + self.ttl,
                            },
                        );
                        instance
                    }
                    _ => {
                        return Err(CallError::new(
                            ErrorCode::Conflict,
                            format!(
                                "{capability} is offered by several connected providers and this session has none selected; nothing was sent. Ask the user to choose one, then retry."
                            ),
                        )
                        .with_execution(Execution::NotStarted)
                        .with_data(serde_json::json!({ "providers": eligible })));
                    }
                }
            }
        };

        let key = (instance, capability.to_owned());
        let Some(provider) = state.providers.get_mut(&key) else {
            return Err(no_host(capability));
        };
        if !provider.caller.is_connected() {
            return Err(CallError::new(
                ErrorCode::HostGone,
                format!("the {capability} provider is disconnected; nothing was sent."),
            )
            .with_execution(Execution::NotStarted));
        }
        if provider.in_flight >= provider.max_in_flight {
            return Err(CallError::new(
                ErrorCode::Conflict,
                format!(
                    "another {capability} operation is already in flight on \"{}\"; nothing was sent. Retry serially after it returns.",
                    provider.label
                ),
            )
            .with_execution(Execution::NotStarted));
        }
        provider.in_flight += 1;
        Ok((provider.caller.clone(), provider.version))
    }

    async fn finish(&self, holder: &Holder, capability: &str) {
        let mut state = self.state.lock().await;
        let instance = state
            .leases
            .get(&(holder.clone(), capability.to_owned()))
            .map(|lease| lease.provider.instance.clone());
        if let Some(instance) = instance
            && let Some(provider) = state.providers.get_mut(&(instance, capability.to_owned()))
        {
            provider.in_flight = provider.in_flight.saturating_sub(1);
        }
    }
}

fn no_host(capability: &str) -> CallError {
    CallError::new(
        ErrorCode::NoHost,
        format!(
            "no connected client offers {capability}; nothing was sent. Do not retry until the user connects one."
        ),
    )
    .with_execution(Execution::NotStarted)
}

#[cfg(test)]
mod tests {
    use super::{
        Broker, CallError, Caller, DEFAULT_CALL_DEADLINE, ErrorCode, Execution, Holder, LeaseState,
        ProviderId, Registration,
    };
    use goat_wire::envelope::Execution as Exec;
    use serde_json::{Value, json};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::sync::{Mutex, mpsc, oneshot};

    struct Fake {
        name: &'static str,
        calls: AtomicUsize,
        connected: AtomicBool,
        gate: Mutex<Option<oneshot::Receiver<()>>>,
        entered: mpsc::UnboundedSender<&'static str>,
    }

    impl Fake {
        fn new(name: &'static str) -> (Arc<Self>, mpsc::UnboundedReceiver<&'static str>) {
            let (entered, rx) = mpsc::unbounded_channel();
            (
                Arc::new(Self {
                    name,
                    calls: AtomicUsize::new(0),
                    connected: AtomicBool::new(true),
                    gate: Mutex::new(None),
                    entered,
                }),
                rx,
            )
        }

        async fn gated(
            name: &'static str,
        ) -> (
            Arc<Self>,
            oneshot::Sender<()>,
            mpsc::UnboundedReceiver<&'static str>,
        ) {
            let (fake, rx) = Self::new(name);
            let (tx, gate) = oneshot::channel();
            *fake.gate.lock().await = Some(gate);
            (fake, tx, rx)
        }
    }

    #[async_trait::async_trait]
    impl Caller for Fake {
        async fn call(
            &self,
            _method: &str,
            _version: u16,
            params: Value,
            _deadline: Duration,
        ) -> Result<Value, CallError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let _ = self.entered.send(self.name);
            if let Some(gate) = self.gate.lock().await.take() {
                let _ = gate.await;
            }
            Ok(json!({"by": self.name, "params": params}))
        }

        fn is_connected(&self) -> bool {
            self.connected.load(Ordering::SeqCst)
        }
    }

    fn one() -> Holder {
        Holder::session(goat_api::SessionId(1))
    }

    fn two() -> Holder {
        Holder::agent("scout")
    }

    fn registration(id: &str, caller: Arc<Fake>, epoch: u64, max_in_flight: usize) -> Registration {
        Registration {
            id: ProviderId::new("device", id),
            label: format!("browser on {id}"),
            capability: "host.browser".to_owned(),
            version: 1,
            boot_epoch: epoch,
            max_in_flight,
            caller,
        }
    }

    #[tokio::test]
    async fn no_provider_fails_immediately_and_tells_the_model_not_to_retry() {
        let broker = Broker::new();
        let err = broker
            .invoke(&one(), "host.browser", Value::Null, DEFAULT_CALL_DEADLINE)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NoHost);
        assert_eq!(err.execution, Some(Execution::NotStarted));
        assert!(err.message.contains("nothing was sent"));
        assert!(err.message.contains("Do not retry"));
        assert!(err.retry_is_safe());
    }

    #[tokio::test]
    async fn a_lone_provider_is_bound_on_first_use_and_reused() {
        let broker = Broker::new();
        let (fake, _entered) = Fake::new("laptop");
        broker
            .register(registration("laptop", fake.clone(), 1, 1))
            .await;

        let first = broker
            .invoke(
                &one(),
                "host.browser",
                json!({"n": 1}),
                DEFAULT_CALL_DEADLINE,
            )
            .await
            .unwrap();
        assert_eq!(first["by"], "laptop");
        assert_eq!(
            broker.lease_state(&one(), "host.browser").await,
            Some(LeaseState::Active)
        );
        broker
            .invoke(
                &one(),
                "host.browser",
                json!({"n": 2}),
                DEFAULT_CALL_DEADLINE,
            )
            .await
            .unwrap();
        assert_eq!(fake.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn capability_registered_after_the_session_started_is_usable() {
        let broker = Broker::new();
        let err = broker
            .invoke(&one(), "host.browser", Value::Null, DEFAULT_CALL_DEADLINE)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NoHost);

        let (fake, _entered) = Fake::new("laptop");
        broker.register(registration("laptop", fake, 1, 1)).await;
        let got = broker
            .invoke(&one(), "host.browser", Value::Null, DEFAULT_CALL_DEADLINE)
            .await
            .unwrap();
        assert_eq!(got["by"], "laptop");
    }

    #[tokio::test]
    async fn two_providers_without_a_selection_is_a_conflict_not_a_coin_flip() {
        let broker = Broker::new();
        let (laptop, _a) = Fake::new("laptop");
        let (desktop, _b) = Fake::new("desktop");
        broker
            .register(registration("laptop", laptop.clone(), 1, 1))
            .await;
        broker
            .register(registration("desktop", desktop.clone(), 1, 1))
            .await;

        let err = broker
            .invoke(&one(), "host.browser", Value::Null, DEFAULT_CALL_DEADLINE)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Conflict);
        assert_eq!(err.execution, Some(Execution::NotStarted));
        assert_eq!(laptop.calls.load(Ordering::SeqCst), 0);
        assert_eq!(desktop.calls.load(Ordering::SeqCst), 0);
        let listed = err.data.unwrap();
        assert_eq!(listed["providers"], json!(["desktop", "laptop"]));

        broker
            .bind(
                &one(),
                "host.browser",
                &ProviderId::new("device", "desktop"),
            )
            .await
            .unwrap();
        let got = broker
            .invoke(&one(), "host.browser", Value::Null, DEFAULT_CALL_DEADLINE)
            .await
            .unwrap();
        assert_eq!(got["by"], "desktop");
    }

    #[tokio::test]
    async fn a_bound_provider_going_away_never_fails_over_to_another_machine() {
        let broker = Broker::new();
        let (laptop, _a) = Fake::new("laptop");
        broker.register(registration("laptop", laptop, 1, 1)).await;
        broker
            .invoke(&one(), "host.browser", Value::Null, DEFAULT_CALL_DEADLINE)
            .await
            .unwrap();

        let (desktop, _b) = Fake::new("desktop");
        broker
            .register(registration("desktop", desktop.clone(), 1, 1))
            .await;
        broker.unregister_instance("laptop").await;

        assert_eq!(
            broker.lease_state(&one(), "host.browser").await,
            Some(LeaseState::Paused)
        );
        let err = broker
            .invoke(&one(), "host.browser", Value::Null, DEFAULT_CALL_DEADLINE)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::HostGone);
        assert_eq!(err.execution, Some(Execution::NotStarted));
        assert_eq!(desktop.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn the_same_instance_returning_with_a_new_boot_epoch_requires_a_rebind() {
        let broker = Broker::new();
        let (first, _a) = Fake::new("laptop");
        broker.register(registration("laptop", first, 1, 1)).await;
        broker
            .invoke(&one(), "host.browser", Value::Null, DEFAULT_CALL_DEADLINE)
            .await
            .unwrap();

        let (reborn, _b) = Fake::new("laptop");
        broker
            .register(registration("laptop", reborn.clone(), 2, 1))
            .await;
        assert_eq!(
            broker.lease_state(&one(), "host.browser").await,
            Some(LeaseState::Paused)
        );
        let err = broker
            .invoke(&one(), "host.browser", Value::Null, DEFAULT_CALL_DEADLINE)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::HostGone);
        assert_eq!(reborn.calls.load(Ordering::SeqCst), 0);

        broker
            .bind(&one(), "host.browser", &ProviderId::new("device", "laptop"))
            .await
            .unwrap();
        broker
            .invoke(&one(), "host.browser", Value::Null, DEFAULT_CALL_DEADLINE)
            .await
            .unwrap();
        assert_eq!(reborn.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_second_concurrent_call_is_refused_before_it_is_sent() {
        let broker = Arc::new(Broker::new());
        let (fake, release, mut entered) = Fake::gated("laptop").await;
        broker
            .register(registration("laptop", fake.clone(), 1, 1))
            .await;

        let first = {
            let broker = broker.clone();
            tokio::spawn(async move {
                broker
                    .invoke(
                        &one(),
                        "host.browser",
                        json!({"n": 1}),
                        DEFAULT_CALL_DEADLINE,
                    )
                    .await
            })
        };
        assert_eq!(entered.recv().await, Some("laptop"));

        let err = broker
            .invoke(
                &one(),
                "host.browser",
                json!({"n": 2}),
                DEFAULT_CALL_DEADLINE,
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Conflict);
        assert_eq!(err.execution, Some(Exec::NotStarted));
        assert!(err.message.contains("in flight"));

        release.send(()).unwrap();
        first.await.unwrap().unwrap();
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);

        broker
            .invoke(
                &one(),
                "host.browser",
                json!({"n": 3}),
                DEFAULT_CALL_DEADLINE,
            )
            .await
            .unwrap();
        assert_eq!(fake.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_session_and_an_agent_get_independent_leases() {
        let broker = Broker::new();
        let (laptop, _a) = Fake::new("laptop");
        let (desktop, _b) = Fake::new("desktop");
        broker
            .register(registration("laptop", laptop.clone(), 1, 1))
            .await;
        broker
            .register(registration("desktop", desktop.clone(), 1, 1))
            .await;
        broker
            .bind(&one(), "host.browser", &ProviderId::new("device", "laptop"))
            .await
            .unwrap();
        broker
            .bind(
                &two(),
                "host.browser",
                &ProviderId::new("device", "desktop"),
            )
            .await
            .unwrap();

        assert_eq!(
            broker
                .invoke(&one(), "host.browser", Value::Null, DEFAULT_CALL_DEADLINE)
                .await
                .unwrap()["by"],
            "laptop"
        );
        assert_eq!(
            broker
                .invoke(&two(), "host.browser", Value::Null, DEFAULT_CALL_DEADLINE)
                .await
                .unwrap()["by"],
            "desktop"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_expired_lease_rebinds_to_the_only_provider() {
        let broker = Broker::with_ttl(Duration::from_mins(1));
        let (laptop, _a) = Fake::new("laptop");
        broker.register(registration("laptop", laptop, 1, 1)).await;
        broker
            .invoke(&one(), "host.browser", Value::Null, DEFAULT_CALL_DEADLINE)
            .await
            .unwrap();
        assert_eq!(
            broker.lease_state(&one(), "host.browser").await,
            Some(LeaseState::Active)
        );

        tokio::time::advance(Duration::from_secs(61)).await;
        assert_eq!(
            broker.lease_state(&one(), "host.browser").await,
            Some(LeaseState::Expired)
        );
        let got = broker
            .invoke(&one(), "host.browser", Value::Null, DEFAULT_CALL_DEADLINE)
            .await
            .unwrap();
        assert_eq!(got["by"], "laptop");
    }

    #[tokio::test(start_paused = true)]
    async fn an_expired_lease_with_two_providers_asks_the_user_again() {
        let broker = Broker::with_ttl(Duration::from_mins(1));
        let (laptop, _a) = Fake::new("laptop");
        let (desktop, _b) = Fake::new("desktop");
        broker.register(registration("laptop", laptop, 1, 1)).await;
        broker
            .bind(&one(), "host.browser", &ProviderId::new("device", "laptop"))
            .await
            .unwrap();
        broker
            .register(registration("desktop", desktop, 1, 1))
            .await;

        tokio::time::advance(Duration::from_secs(61)).await;
        let err = broker
            .invoke(&one(), "host.browser", Value::Null, DEFAULT_CALL_DEADLINE)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Conflict);
    }

    #[tokio::test]
    async fn binding_an_unknown_provider_is_refused() {
        let broker = Broker::new();
        let err = broker
            .bind(&one(), "host.browser", &ProviderId::new("device", "ghost"))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NoHost);
    }

    #[tokio::test]
    async fn releasing_a_holder_drops_only_its_leases() {
        let broker = Broker::new();
        let (laptop, _a) = Fake::new("laptop");
        broker.register(registration("laptop", laptop, 1, 1)).await;
        broker
            .bind(&one(), "host.browser", &ProviderId::new("device", "laptop"))
            .await
            .unwrap();
        broker
            .bind(&two(), "host.browser", &ProviderId::new("device", "laptop"))
            .await
            .unwrap();
        broker.release_holder(&one()).await;
        assert_eq!(broker.lease_state(&one(), "host.browser").await, None);
        assert_eq!(
            broker.lease_state(&two(), "host.browser").await,
            Some(LeaseState::Active)
        );
    }

    #[tokio::test]
    async fn holders_finds_leases_however_they_were_made() {
        let broker = Broker::new();
        let (laptop, _a) = Fake::new("laptop");
        broker.register(registration("laptop", laptop, 1, 1)).await;
        broker
            .invoke(&one(), "host.browser", Value::Null, DEFAULT_CALL_DEADLINE)
            .await
            .unwrap();
        broker
            .bind(&two(), "host.browser", &ProviderId::new("device", "laptop"))
            .await
            .unwrap();

        let (desktop, _b) = Fake::new("desktop");
        broker
            .register(registration("desktop", desktop, 1, 1))
            .await;

        let mut found = broker.holders("laptop", "host.browser").await;
        found.sort();
        assert_eq!(found, vec![two(), one()]);
        assert!(broker.holders("desktop", "host.browser").await.is_empty());
        assert!(broker.holders("laptop", "host.simulator").await.is_empty());
    }

    #[tokio::test]
    async fn a_provider_that_reports_itself_disconnected_is_not_called() {
        let broker = Broker::new();
        let (fake, _entered) = Fake::new("laptop");
        broker
            .register(registration("laptop", fake.clone(), 1, 1))
            .await;
        fake.connected.store(false, Ordering::SeqCst);
        let err = broker
            .invoke(&one(), "host.browser", Value::Null, DEFAULT_CALL_DEADLINE)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::HostGone);
        assert_eq!(err.execution, Some(Execution::NotStarted));
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn providers_are_listed_for_a_user_choice() {
        let broker = Broker::new();
        let (laptop, _a) = Fake::new("laptop");
        let (desktop, _b) = Fake::new("desktop");
        broker.register(registration("laptop", laptop, 1, 1)).await;
        broker
            .register(registration("desktop", desktop, 1, 1))
            .await;
        let listed = broker.providers("host.browser").await;
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id.instance, "desktop");
        assert_eq!(listed[0].label, "browser on desktop");
        assert!(broker.providers("host.simulator").await.is_empty());
    }
}
