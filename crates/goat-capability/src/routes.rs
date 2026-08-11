use std::sync::Arc;

use goat_api::{
    CapabilityAdvertise, CapabilityAdvertiseParams, CapabilityBind, CapabilityBindParams,
    CapabilityList, CapabilityListOutput, CapabilityListParams, CapabilityProvider, Empty,
    RouteCtx, Router,
};
use goat_wire::envelope::CallError;

use crate::{Broker, ProviderId, Registration};

#[must_use]
pub fn routes(router: Router, broker: Arc<Broker>, device: String) -> Router {
    let advertise = broker.clone();
    let advertise_device = device.clone();
    let list = broker.clone();
    let bind = broker;
    let bind_device = device;

    router
        .unary::<CapabilityAdvertise, _, _>(move |params: CapabilityAdvertiseParams, ctx| {
            let broker = advertise.clone();
            let device = advertise_device.clone();
            async move {
                advertise_capabilities(&broker, &device, params, &ctx).await;
                Ok(Empty {})
            }
        })
        .unary::<CapabilityList, _, _>(move |params: CapabilityListParams, _ctx| {
            let broker = list.clone();
            async move {
                let providers = broker.providers(&params.capability).await;
                Ok(CapabilityListOutput {
                    providers: providers
                        .into_iter()
                        .map(|provider| CapabilityProvider {
                            instance: provider.id.instance,
                            label: provider.label,
                            capability: params.capability.clone(),
                            version: provider.version,
                            bound: false,
                        })
                        .collect(),
                })
            }
        })
        .unary::<CapabilityBind, _, _>(move |params: CapabilityBindParams, _ctx| {
            let broker = bind.clone();
            let device = bind_device.clone();
            async move {
                broker
                    .bind(
                        params.session.0,
                        &params.capability,
                        &ProviderId::new(device, params.instance),
                    )
                    .await?;
                Ok::<Empty, CallError>(Empty {})
            }
        })
}

async fn advertise_capabilities(
    broker: &Broker,
    device: &str,
    params: CapabilityAdvertiseParams,
    ctx: &RouteCtx,
) {
    if params.offers.is_empty() {
        broker.unregister_instance(&params.instance).await;
        return;
    }
    for offer in params.offers {
        broker
            .register(Registration {
                id: ProviderId::new(device, params.instance.clone()),
                label: params.label.clone(),
                capability: offer.id,
                version: offer.version,
                boot_epoch: params.boot_epoch,
                max_in_flight: offer.max_in_flight,
                caller: Arc::new(ctx.peer.clone()),
            })
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::routes;
    use crate::{Broker, DEFAULT_CALL_DEADLINE};
    use goat_api::{
        CapabilityListOutput, Empty, Grant, HostBrowser, HostBrowserOutput, HostBrowserParams,
        Router,
    };
    use goat_wire::WireConn;
    use goat_wire::envelope::{CallError, ErrorCode, Execution, Frame, Role};
    use goat_wire::peer::{Peer, RejectAll, spawn};
    use serde_json::{Value, json};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    type Conn = WireConn<tokio::io::DuplexStream, Frame, Frame>;

    struct Rig {
        client: Peer,
        _daemon: Peer,
        closed: CancellationToken,
    }

    fn daemon_router(broker: Arc<Broker>) -> Router {
        let driver = broker.clone();
        routes(Router::new([Grant::Any]), broker, "device".to_owned()).unary::<HostBrowser, _, _>(
            move |params: HostBrowserParams, _ctx| {
                let broker = driver.clone();
                async move {
                    let value = broker
                        .invoke(
                            params.origin.session.0,
                            "host.browser",
                            serde_json::to_value(&params).unwrap_or(Value::Null),
                            DEFAULT_CALL_DEADLINE,
                        )
                        .await?;
                    serde_json::from_value::<HostBrowserOutput>(value).map_err(|err| {
                        CallError::new(ErrorCode::Internal, format!("bad host reply: {err}"))
                    })
                }
            },
        )
    }

    fn browser_client(entered: mpsc::UnboundedSender<String>, hold: bool) -> Router {
        Router::new([Grant::Any]).unary::<HostBrowser, _, _>(
            move |params: HostBrowserParams, ctx| {
                let entered = entered.clone();
                async move {
                    let _ = entered.send(params.action.clone());
                    if hold {
                        ctx.cancel.cancelled().await;
                        return Err(CallError::new(ErrorCode::Canceled, "cancelled")
                            .with_execution(Execution::OutcomeUnknown));
                    }
                    Ok(HostBrowserOutput {
                        summary: format!("did {}", params.action),
                        text: Some("hello".to_owned()),
                        media_type: None,
                        image: None,
                    })
                }
            },
        )
    }

    fn connect(daemon: Router, client: Router) -> Rig {
        let (a, b) = tokio::io::duplex(1024 * 1024);
        let (client_sink, client_source) = Conn::new(a).split();
        let (daemon_sink, daemon_source) = Conn::new(b).split();
        let closed = CancellationToken::new();
        let daemon_peer = spawn(
            Role::Daemon,
            Box::pin(daemon_sink),
            Box::pin(daemon_source),
            Arc::new(daemon),
            closed.clone(),
        );
        let client_peer = spawn(
            Role::Client,
            Box::pin(client_sink),
            Box::pin(client_source),
            Arc::new(client),
            closed.clone(),
        );
        Rig {
            client: client_peer,
            _daemon: daemon_peer,
            closed,
        }
    }

    async fn advertise(rig: &Rig, instance: &str, epoch: u64) {
        rig.client
            .handle
            .call(
                "capability.advertise",
                1,
                json!({
                    "instance": instance,
                    "label": format!("browser on {instance}"),
                    "boot_epoch": epoch,
                    "offers": [{"id": "host.browser", "version": 1, "max_in_flight": 1}]
                }),
            )
            .await
            .unwrap();
    }

    fn drive(action: &str) -> Value {
        json!({
            "origin": {"session": "1", "task": "9", "label": "test"},
            "action": action,
            "arguments": {}
        })
    }

    #[tokio::test]
    async fn a_browser_call_travels_daemon_to_client_and_back() {
        let broker = Arc::new(Broker::new());
        let (entered, mut entered_rx) = mpsc::unbounded_channel();
        let rig = connect(
            daemon_router(broker.clone()),
            browser_client(entered, false),
        );

        advertise(&rig, "laptop", 1).await;

        let value = rig
            .client
            .handle
            .call("host.browser", 1, drive("navigate"))
            .await
            .unwrap();
        assert_eq!(entered_rx.recv().await.as_deref(), Some("navigate"));
        let output: HostBrowserOutput = serde_json::from_value(value).unwrap();
        assert_eq!(output.summary, "did navigate");
        assert_eq!(output.text.as_deref(), Some("hello"));
    }

    #[tokio::test]
    async fn without_an_advertisement_the_call_fails_before_anything_is_sent() {
        let broker = Arc::new(Broker::new());
        let (entered, _entered_rx) = mpsc::unbounded_channel();
        let rig = connect(
            daemon_router(broker.clone()),
            browser_client(entered, false),
        );

        let err = rig
            .client
            .handle
            .call("host.browser", 1, drive("navigate"))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NoHost);
        assert_eq!(err.execution, Some(Execution::NotStarted));
        assert!(err.retry_is_safe());
    }

    #[tokio::test]
    async fn withdrawing_the_advertisement_unregisters_the_provider() {
        let broker = Arc::new(Broker::new());
        let (entered, _entered_rx) = mpsc::unbounded_channel();
        let rig = connect(
            daemon_router(broker.clone()),
            browser_client(entered, false),
        );

        advertise(&rig, "laptop", 1).await;
        assert_eq!(broker.providers("host.browser").await.len(), 1);

        rig.client
            .handle
            .call(
                "capability.advertise",
                1,
                json!({"instance": "laptop", "label": "gone", "boot_epoch": 1, "offers": []}),
            )
            .await
            .unwrap();
        assert!(broker.providers("host.browser").await.is_empty());
    }

    #[tokio::test]
    async fn capability_list_reports_what_the_user_can_choose() {
        let broker = Arc::new(Broker::new());
        let (entered, _entered_rx) = mpsc::unbounded_channel();
        let rig = connect(
            daemon_router(broker.clone()),
            browser_client(entered, false),
        );
        advertise(&rig, "laptop", 1).await;

        let value = rig
            .client
            .handle
            .call("capability.list", 1, json!({"capability": "host.browser"}))
            .await
            .unwrap();
        let listed: CapabilityListOutput = serde_json::from_value(value).unwrap();
        assert_eq!(listed.providers.len(), 1);
        assert_eq!(listed.providers[0].instance, "laptop");
        assert_eq!(listed.providers[0].capability, "host.browser");
    }

    #[tokio::test]
    async fn binding_an_unknown_instance_is_refused_over_the_wire() {
        let broker = Arc::new(Broker::new());
        let (entered, _entered_rx) = mpsc::unbounded_channel();
        let rig = connect(daemon_router(broker), browser_client(entered, false));

        let err = rig
            .client
            .handle
            .call(
                "capability.bind",
                1,
                json!({"session": "1", "capability": "host.browser", "instance": "ghost"}),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NoHost);
    }

    #[tokio::test]
    async fn a_disconnect_mid_call_reports_an_unknown_outcome() {
        let broker = Arc::new(Broker::new());
        let (entered, mut entered_rx) = mpsc::unbounded_channel();
        let rig = connect(daemon_router(broker.clone()), browser_client(entered, true));
        advertise(&rig, "laptop", 1).await;

        let call = {
            let handle = rig.client.handle.clone();
            tokio::spawn(async move { handle.call("host.browser", 1, drive("click")).await })
        };
        assert_eq!(entered_rx.recv().await.as_deref(), Some("click"));

        rig.closed.cancel();
        let err = call.await.unwrap().unwrap_err();
        assert_eq!(err.execution, Some(Execution::OutcomeUnknown));
        assert!(!err.retry_is_safe());
    }

    #[tokio::test(start_paused = true)]
    async fn a_host_that_never_answers_times_out_as_an_unknown_outcome() {
        let broker = Arc::new(Broker::new());
        let (entered, mut entered_rx) = mpsc::unbounded_channel();
        let rig = connect(daemon_router(broker.clone()), browser_client(entered, true));
        advertise(&rig, "laptop", 1).await;

        let call = {
            let handle = rig.client.handle.clone();
            tokio::spawn(async move { handle.call("host.browser", 1, drive("click")).await })
        };
        assert_eq!(entered_rx.recv().await.as_deref(), Some("click"));

        tokio::time::advance(DEFAULT_CALL_DEADLINE + Duration::from_secs(1)).await;
        let err = call.await.unwrap().unwrap_err();
        assert_eq!(err.code, ErrorCode::Timeout);
        assert_eq!(err.execution, Some(Execution::OutcomeUnknown));
    }

    #[tokio::test]
    async fn a_client_that_serves_nothing_still_reaches_the_daemon() {
        let broker = Arc::new(Broker::new());
        let (a, b) = tokio::io::duplex(1024 * 1024);
        let (client_sink, client_source) = Conn::new(a).split();
        let (daemon_sink, daemon_source) = Conn::new(b).split();
        let closed = CancellationToken::new();
        let _daemon = spawn(
            Role::Daemon,
            Box::pin(daemon_sink),
            Box::pin(daemon_source),
            Arc::new(daemon_router(broker)),
            closed.clone(),
        );
        let client = spawn(
            Role::Client,
            Box::pin(client_sink),
            Box::pin(client_source),
            Arc::new(RejectAll),
            closed,
        );
        let value = client
            .handle
            .call("capability.list", 1, json!({"capability": "host.browser"}))
            .await
            .unwrap();
        let listed: CapabilityListOutput = serde_json::from_value(value).unwrap();
        assert!(listed.providers.is_empty());
        let _: Empty = serde_json::from_value(json!({})).unwrap();
    }
}
