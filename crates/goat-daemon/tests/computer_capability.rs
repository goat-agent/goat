use std::{sync::Arc, time::Duration};

use goat_api::{
    CapabilityList, CapabilityListParams, ComputerCommand, Holder, HostComputerOutput, SessionId,
};
use goat_computer_host::{
    CAPABILITY, CAPABILITY_VERSION, ComputerHost, advertise, advertisement, fake::FakeDesktop,
    withdrawal,
};
use goat_wire::{
    WireConn,
    envelope::{Frame, Hello, Role},
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn computer_capability_roundtrip_and_withdrawal() {
    let dir = tempfile::tempdir().unwrap();
    let manager = goat_daemon::CodeSessionHub::new(
        dir.path().join("auth.json"),
        goat_config::UserProviders::at(dir.path().join("config.json")),
        dir.path().join("store.sqlite"),
    );
    manager.mark_ready();
    let broker = manager.broker();
    let shutdown = CancellationToken::new();
    let host = goat_daemon::EnvelopeHost::new(
        manager,
        shutdown.clone(),
        "computer-test".into(),
        dir.path().join("store.sqlite"),
    );
    let socket = dir.path().join("computer.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let serving = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (sink, source) = WireConn::<_, Frame, Frame>::new(stream).split();
        goat_daemon::serve_envelope(
            host,
            goat_daemon::ClientOrigin::Local,
            Box::pin(sink),
            Box::pin(source),
            shutdown,
        )
        .await;
    });
    let desktop = Arc::new(FakeDesktop::default());
    desktop.push(Ok(HostComputerOutput::Point { x: 123.0, y: 456.0 }));
    let provider = Arc::new(ComputerHost::new(desktop.clone()));
    let link = goat_client::Link::local(socket, std::path::PathBuf::new());
    let connection = goat_client::open_serving(
        &link,
        "computer-test",
        provider.clone(),
        Hello::new(Role::Client, "computer-test").with_method(CAPABILITY, vec![CAPABILITY_VERSION]),
    )
    .await
    .unwrap();
    advertise(
        &connection.api,
        advertisement("test-mac/desktop", "Test Mac", 7),
    )
    .await
    .unwrap();
    let listed = connection
        .api
        .call::<CapabilityList>(CapabilityListParams {
            capability: CAPABILITY.into(),
        })
        .await
        .unwrap();
    assert_eq!(listed.providers.len(), 1);
    assert_eq!(listed.providers[0].instance, "test-mac/desktop");
    let output = broker
        .invoke(
            &Holder::session(SessionId(1)),
            CAPABILITY,
            serde_json::json!({"command":"cursor_position"}),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(
        output,
        serde_json::json!({"reply":"point", "x":123.0, "y":456.0})
    );
    assert_eq!(desktop.commands(), vec![ComputerCommand::CursorPosition {}]);
    provider.halt();
    let error = broker
        .invoke(
            &Holder::session(SessionId(1)),
            CAPABILITY,
            serde_json::json!({"command":"cursor_position"}),
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.execution,
        Some(goat_wire::envelope::Execution::NotStarted)
    );
    assert_eq!(desktop.commands(), vec![ComputerCommand::CursorPosition {}]);
    advertise(&connection.api, withdrawal("test-mac/desktop", 7))
        .await
        .unwrap();
    let listed = connection
        .api
        .call::<CapabilityList>(CapabilityListParams {
            capability: CAPABILITY.into(),
        })
        .await
        .unwrap();
    assert!(listed.providers.is_empty());
    drop(connection);
    serving.abort();
}
