use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use goat_api::{
    CapabilityBind, CapabilityBindParams, CapabilityList, CapabilityListParams, Holder, ResumeMode,
    SessionId, SessionOpen, SessionOpenParams,
};
use goat_browser_host::{
    BrowserHost, BrowserPort, CAPABILITY, advertise, advertisement, withdrawal,
};
use goat_client::Link;
use goat_wire::envelope::{Hello, Role};
use goat_wire::transport;
use serde_json::{Value, json};
use tokio::sync::mpsc;

async fn start_daemon(dir: &std::path::Path) -> PathBuf {
    let socket = dir.join("d.sock");
    let cfg = goat_daemon::DaemonConfig {
        socket_path: socket.clone(),
        lock_path: dir.join("daemon.lock"),
        auth_path: dir.join("auth.json"),
        config_json: dir.join("config.json"),
        db_path: dir.join("store.sqlite"),
        remote: None,
    };
    tokio::spawn(async move {
        let _ = goat_daemon::serve(cfg).await;
    });
    for _ in 0..50 {
        if transport::connect(&socket).await.is_ok() {
            return socket;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("daemon did not start");
}

fn link(socket: &std::path::Path) -> Link {
    Link::local(socket.to_path_buf(), PathBuf::new())
}

type Conn = goat_wire::WireConn<
    tokio::io::DuplexStream,
    goat_wire::envelope::Frame,
    goat_wire::envelope::Frame,
>;

struct RecordingPort {
    sent: mpsc::UnboundedSender<(u64, Value)>,
}

#[async_trait::async_trait]
impl BrowserPort for RecordingPort {
    async fn dispatch(&self, request_id: u64, params: Value) -> Result<(), String> {
        self.sent
            .send((request_id, params))
            .map_err(|_| "the browser closed".to_owned())
    }
}

async fn attach_browser(
    socket: &std::path::Path,
    instance: &str,
    boot_epoch: u64,
) -> (
    goat_client::ApiSession,
    Arc<BrowserHost>,
    mpsc::UnboundedReceiver<(u64, Value)>,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    let host = Arc::new(BrowserHost::new(Arc::new(RecordingPort { sent: tx })));
    let hello = Hello::new(Role::Client, "goat-browser-host/test")
        .with_method(CAPABILITY, vec![goat_browser_host::CAPABILITY_VERSION]);
    let session = goat_client::open_serving(&link(socket), "browser-host", host.clone(), hello)
        .await
        .expect("the daemon greets a capability provider");
    advertise(
        &session.api,
        advertisement(instance.to_owned(), "Chrome".to_owned(), boot_epoch),
    )
    .await
    .expect("the advertisement reaches the broker");
    (session, host, rx)
}

#[tokio::test]
async fn an_advertised_browser_is_visible_to_another_client() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;

    let (_provider, _host, _rx) = attach_browser(&socket, "chrome-default", 7).await;

    let watcher = goat_client::open_api(&link(&socket), "watcher")
        .await
        .unwrap();
    let listed = watcher
        .api
        .call::<CapabilityList>(CapabilityListParams {
            capability: CAPABILITY.to_owned(),
        })
        .await
        .expect("capability.list answers");

    assert_eq!(listed.providers.len(), 1, "one browser is attached");
    assert_eq!(listed.providers[0].instance, "chrome-default");
    assert_eq!(listed.providers[0].label, "Chrome");
}

#[tokio::test]
async fn the_broker_reaches_a_real_provider_over_a_real_connection() {
    let dir = tempfile::tempdir().unwrap();
    let manager = goat_daemon::CodeSessionHub::new(
        dir.path().join("auth.json"),
        goat_config::UserProviders::at(dir.path().join("config.json")),
        dir.path().join("store.sqlite"),
    );
    manager.mark_ready();
    let broker = manager.broker();
    let shutdown = tokio_util::sync::CancellationToken::new();
    let host = goat_daemon::EnvelopeHost::new(
        manager,
        shutdown.clone(),
        "e1".to_owned(),
        dir.path().join("store.sqlite"),
    );

    let (a, b) = tokio::io::duplex(1024 * 1024);
    let (daemon_sink, daemon_source) = Conn::new(a).split();
    let closed = tokio_util::sync::CancellationToken::new();
    tokio::spawn(goat_daemon::serve_envelope(
        host,
        goat_daemon::ClientOrigin::Local,
        Box::pin(daemon_sink),
        Box::pin(daemon_source),
        closed.clone(),
    ));

    let (tx, mut rx) = mpsc::unbounded_channel();
    let browser = Arc::new(BrowserHost::new(Arc::new(RecordingPort { sent: tx })));
    let (client_sink, client_source) = Conn::new(b).split();
    let mut peer = goat_wire::peer::spawn(
        Role::Client,
        Box::pin(client_sink),
        Box::pin(client_source),
        browser.clone(),
        closed.clone(),
    );
    let greeting = peer.hello.recv().await.expect("the daemon greets first");
    let api = goat_api::Api::negotiated(peer.handle.clone(), &greeting);
    peer.handle
        .send_hello(
            Hello::new(Role::Client, "goat-browser-host/test")
                .with_method(CAPABILITY, vec![goat_browser_host::CAPABILITY_VERSION]),
        )
        .await
        .expect("the provider greets back");
    advertise(
        &api,
        advertisement("chrome-default".to_owned(), "Chrome".to_owned(), 7),
    )
    .await
    .expect("the advertisement reaches the broker");

    let calling = tokio::spawn(async move {
        broker
            .invoke(
                &Holder::session(SessionId(1)),
                CAPABILITY,
                json!({ "command": "cdp", "method": "Page.navigate", "params": {} }),
                Duration::from_secs(5),
            )
            .await
    });

    let (request_id, params) = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("the reverse call reaches the browser port")
        .expect("the port is open");
    assert_eq!(params["method"], "Page.navigate");

    browser
        .settle(
            request_id,
            Ok(json!({ "reply": "cdp", "result": { "frameId": "f1" } })),
        )
        .await;

    let answered = tokio::time::timeout(Duration::from_secs(5), calling)
        .await
        .expect("the answer comes back")
        .expect("the call task finishes")
        .expect("the browser answered");
    assert_eq!(answered["result"]["frameId"], "f1");
    std::mem::forget(peer);
}

#[tokio::test]
async fn a_withdrawn_browser_leaves_the_list() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;

    let (provider, _host, _rx) = attach_browser(&socket, "chrome-default", 7).await;
    advertise(&provider.api, withdrawal("chrome-default".to_owned(), 7))
        .await
        .expect("the withdrawal reaches the broker");

    let watcher = goat_client::open_api(&link(&socket), "watcher")
        .await
        .unwrap();
    let listed = watcher
        .api
        .call::<CapabilityList>(CapabilityListParams {
            capability: CAPABILITY.to_owned(),
        })
        .await
        .expect("capability.list answers");
    assert!(
        listed.providers.is_empty(),
        "a browser that offers nothing is no longer a provider"
    );
}

#[tokio::test]
async fn binding_an_unknown_instance_is_refused_without_touching_a_browser() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;

    let (_provider, _host, mut rx) = attach_browser(&socket, "chrome-default", 7).await;

    let caller = goat_client::open_api(&link(&socket), "caller")
        .await
        .unwrap();
    let session = caller
        .api
        .call::<SessionOpen>(SessionOpenParams {
            cwd: dir.path().display().to_string(),
            resume: ResumeMode::New {},
        })
        .await
        .unwrap()
        .session;

    let refused = caller
        .api
        .call::<CapabilityBind>(CapabilityBindParams {
            holder: Holder::session(session),
            capability: CAPABILITY.to_owned(),
            instance: "a-browser-that-never-attached".to_owned(),
        })
        .await;
    assert!(refused.is_err(), "an unknown instance cannot be bound");
    assert!(
        rx.try_recv().is_err(),
        "a refused bind must not reach any browser"
    );
}
