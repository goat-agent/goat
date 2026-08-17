use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use goat_api::{
    BrowserCommand, CapabilityBind, CapabilityBindParams, CapabilityList, CapabilityListParams,
    Holder, HostBrowserOutput,
};
use goat_browser_host::CAPABILITY;
use goat_browser_host::native::{Bridge, read_message, write_message};
use goat_client::{ApiSession, Link};
use goat_daemon::{BrowserRelay, CodeSessionHub, DaemonConfig};
use goat_tool_browser::Transport;
use serde_json::{Value, json};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio_util::sync::CancellationToken;

struct Chrome {
    _child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
}

async fn start_daemon(home: &Path) -> CodeSessionHub {
    let root = home.join(".goat");
    std::fs::create_dir_all(&root).expect("a fresh goat tree");
    let config = DaemonConfig {
        socket_path: root.join("daemon.sock"),
        lock_path: root.join("daemon.lock"),
        auth_path: root.join("credentials.json"),
        config_json: root.join("config.json"),
        db_path: root.join("goat.db"),
        remote: None,
    };
    let socket = config.socket_path.clone();
    let hub = CodeSessionHub::new(
        config.auth_path.clone(),
        goat_config::UserProviders::at(config.config_json.clone()),
        config.db_path.clone(),
    );
    hub.mark_ready();
    let serving = hub.clone();
    tokio::spawn(async move {
        let lock = goat_daemon::acquire(&config.lock_path, Duration::ZERO)
            .await
            .expect("the lock is free in a fresh tree");
        let _ = goat_daemon::serve_with(config, serving, CancellationToken::new(), &lock).await;
    });
    for _ in 0..100 {
        if goat_wire::transport::connect(&socket).await.is_ok() {
            return hub;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the daemon never bound {}", socket.display());
}

fn spawn_browser_host(home: &Path, instance: &str) -> Chrome {
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_goat"))
        .arg("browser-host")
        .arg("--instance")
        .arg(instance)
        .arg("--label")
        .arg("Test Chrome")
        .env("HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .expect("the goat binary runs");
    let stdin = child.stdin.take().expect("stdin is piped");
    let stdout = child.stdout.take().expect("stdout is piped");
    Chrome {
        _child: child,
        stdin,
        stdout,
    }
}

async fn await_advertisement(socket: &Path, instance: &str) -> ApiSession {
    let client = goat_client::open_api(&Link::local(socket.to_path_buf(), PathBuf::new()), "test")
        .await
        .expect("the daemon greets a client");
    for _ in 0..200 {
        let listed = client
            .api
            .call::<CapabilityList>(CapabilityListParams {
                capability: CAPABILITY.to_owned(),
            })
            .await
            .expect("capability.list answers");
        if listed
            .providers
            .iter()
            .any(|provider| provider.instance == instance)
        {
            return client;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the browser host never advertised {instance}");
}

async fn to_chrome(chrome: &mut Chrome, seq: u64, body: Value) {
    let framed = serde_json::to_value(Bridge::Message { seq, body }).expect("a bridge frame");
    write_message(&mut chrome.stdin, &framed)
        .await
        .expect("the host reads its stdin");
}

async fn answer_one(chrome: &mut Chrome, reply: Value) -> Value {
    let message = read_message(&mut chrome.stdout)
        .await
        .expect("the host writes a request");
    let bridge: Bridge = serde_json::from_value(message).expect("the host frames its requests");
    let Bridge::Message { body, .. } = bridge else {
        panic!("a small request must not be chunked")
    };
    assert_eq!(body["type"], "browser.request");
    let request_id = body["request_id"]
        .as_str()
        .expect("a request carries an id")
        .to_owned();
    let params = body["params"].clone();
    to_chrome(
        chrome,
        1,
        json!({
            "type": "browser.reply",
            "request_id": request_id,
            "result": reply,
        }),
    )
    .await;
    params
}

#[tokio::test]
async fn a_real_browser_host_carries_a_cdp_command_and_its_events() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let hub = start_daemon(home).await;

    let mut chrome = spawn_browser_host(home, "chrome-e2e");
    let client = await_advertisement(&home.join(".goat/daemon.sock"), "chrome-e2e").await;

    let holder = Holder::agent("browser-e2e");
    client
        .api
        .call::<CapabilityBind>(CapabilityBindParams {
            holder: holder.clone(),
            capability: CAPABILITY.to_owned(),
            instance: "chrome-e2e".to_owned(),
        })
        .await
        .expect("the advertised browser binds");

    let relay = BrowserRelay::new(hub.broker(), hub.browser_events(), holder);
    let mut events = relay.events();

    let (answered, dispatched) = tokio::join!(
        relay.call(BrowserCommand::Cdp {
            method: "Page.navigate".to_owned(),
            params: json!({ "url": "about:blank" }),
        }),
        answer_one(
            &mut chrome,
            json!({ "reply": "cdp", "result": { "frameId": "f1" } }),
        ),
    );

    assert_eq!(dispatched["command"], "cdp");
    assert_eq!(dispatched["method"], "Page.navigate");
    assert_eq!(dispatched["params"]["url"], "about:blank");
    assert_eq!(
        answered.expect("the browser answered"),
        HostBrowserOutput::Cdp {
            result: json!({ "frameId": "f1" }),
        }
    );

    to_chrome(
        &mut chrome,
        2,
        json!({
            "type": "browser.event",
            "event": {
                "method": "Page.loadEventFired",
                "params": { "timestamp": 12.0 },
            },
        }),
    )
    .await;

    let event = tokio::time::timeout(Duration::from_secs(10), events.recv())
        .await
        .expect("an unsolicited event reaches the holder")
        .expect("the event lane is open");
    assert_eq!(event.method, "Page.loadEventFired");
    assert_eq!(event.params["timestamp"], 12.0);
}

#[tokio::test]
async fn a_browser_that_never_started_the_work_reports_a_safe_retry() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let hub = start_daemon(home).await;

    let mut chrome = spawn_browser_host(home, "chrome-refuses");
    let _client = await_advertisement(&home.join(".goat/daemon.sock"), "chrome-refuses").await;

    let relay = BrowserRelay::new(
        hub.broker(),
        hub.browser_events(),
        Holder::agent("browser-e2e"),
    );

    let refusing = async {
        let message = read_message(&mut chrome.stdout)
            .await
            .expect("the host writes a request");
        let bridge: Bridge = serde_json::from_value(message).expect("the host frames its requests");
        let Bridge::Message { body, .. } = bridge else {
            panic!("a small request must not be chunked")
        };
        let request_id = body["request_id"]
            .as_str()
            .expect("a request carries an id")
            .to_owned();
        to_chrome(
            &mut chrome,
            1,
            json!({
                "type": "browser.reply",
                "request_id": request_id,
                "error": { "message": "no active tab", "started": false },
            }),
        )
        .await;
    };

    let (answered, ()) = tokio::join!(relay.call(BrowserCommand::TabList {}), refusing);
    let err = answered.expect_err("a browser with no tab refuses");
    assert!(
        err.to_string().contains("no active tab"),
        "the browser's own message survives the wire: {err}"
    );
}
