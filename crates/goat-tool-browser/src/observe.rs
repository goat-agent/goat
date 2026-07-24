use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::Arc;

use chromiumoxide::Page;
use chromiumoxide::cdp::browser_protocol::network::{
    self, EventLoadingFailed, EventResponseReceived,
};
use chromiumoxide::cdp::js_protocol::runtime::{
    EventConsoleApiCalled, EventExceptionThrown, RemoteObject,
};
use futures::StreamExt as _;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

const RING_MAX: usize = 60;
const URL_MAX: usize = 120;
const CONSOLE_TEXT_MAX: usize = 200;

struct NetworkEntry {
    status: i64,
    kind: String,
    mime: String,
    url: String,
    failure: Option<String>,
}

struct ConsoleEntry {
    level: String,
    text: String,
}

type NetworkRing = Arc<Mutex<VecDeque<NetworkEntry>>>;
type ConsoleRing = Arc<Mutex<VecDeque<ConsoleEntry>>>;

pub struct SessionObservers {
    network: NetworkRing,
    console: ConsoleRing,
    tasks: Vec<JoinHandle<()>>,
}

impl SessionObservers {
    pub async fn spawn(page: &Page) -> Self {
        let network: NetworkRing = Arc::new(Mutex::new(VecDeque::new()));
        let console: ConsoleRing = Arc::new(Mutex::new(VecDeque::new()));
        let _ = page.execute(network::EnableParams::default()).await;
        let mut tasks = Vec::new();
        if let Ok(mut events) = page.event_listener::<EventResponseReceived>().await {
            let ring = network.clone();
            tasks.push(tokio::spawn(async move {
                while let Some(event) = events.next().await {
                    push(
                        &ring,
                        NetworkEntry {
                            status: event.response.status,
                            kind: format!("{:?}", event.r#type).to_lowercase(),
                            mime: event.response.mime_type.clone(),
                            url: cap(&event.response.url, URL_MAX),
                            failure: None,
                        },
                    )
                    .await;
                }
            }));
        }
        if let Ok(mut events) = page.event_listener::<EventLoadingFailed>().await {
            let ring = network.clone();
            tasks.push(tokio::spawn(async move {
                while let Some(event) = events.next().await {
                    if event.canceled == Some(true) {
                        continue;
                    }
                    push(
                        &ring,
                        NetworkEntry {
                            status: 0,
                            kind: format!("{:?}", event.r#type).to_lowercase(),
                            mime: String::new(),
                            url: String::new(),
                            failure: Some(event.error_text.clone()),
                        },
                    )
                    .await;
                }
            }));
        }
        if let Ok(mut events) = page.event_listener::<EventConsoleApiCalled>().await {
            let ring = console.clone();
            tasks.push(tokio::spawn(async move {
                while let Some(event) = events.next().await {
                    let level = format!("{:?}", event.r#type).to_lowercase();
                    let text = render_args(&event.args);
                    push_console(&ring, ConsoleEntry { level, text }).await;
                }
            }));
        }
        if let Ok(mut events) = page.event_listener::<EventExceptionThrown>().await {
            let ring = console.clone();
            tasks.push(tokio::spawn(async move {
                while let Some(event) = events.next().await {
                    let text = cap(&event.exception_details.text, CONSOLE_TEXT_MAX);
                    push_console(
                        &ring,
                        ConsoleEntry {
                            level: "exception".to_owned(),
                            text,
                        },
                    )
                    .await;
                }
            }));
        }
        Self {
            network,
            console,
            tasks,
        }
    }

    pub fn abort(&self) {
        for task in &self.tasks {
            task.abort();
        }
    }

    pub async fn read_network(&self, filter: Option<&str>, limit: usize) -> String {
        let ring = self.network.lock().await;
        let mut out = String::from("network (recent):\n");
        let mut shown = 0usize;
        for entry in ring.iter().rev() {
            if let Some(needle) = filter
                && !entry.url.contains(needle)
                && !entry.failure.as_deref().is_some_and(|f| f.contains(needle))
            {
                continue;
            }
            if let Some(failure) = &entry.failure {
                let _ = writeln!(out, "- [FAILED {}] {} {}", failure, entry.kind, entry.url);
            } else {
                let _ = writeln!(
                    out,
                    "- [{}] {} {} {}",
                    entry.status, entry.kind, entry.mime, entry.url
                );
            }
            shown += 1;
            if shown >= limit {
                break;
            }
        }
        if shown == 0 {
            out.push_str("- none\n");
        }
        out
    }

    pub async fn read_console(&self, level: Option<&str>, limit: usize) -> String {
        let ring = self.console.lock().await;
        let mut out = String::from("console (recent):\n");
        let mut shown = 0usize;
        for entry in ring.iter().rev() {
            if let Some(want) = level
                && entry.level != want
            {
                continue;
            }
            let _ = writeln!(out, "- [{}] {}", entry.level, entry.text);
            shown += 1;
            if shown >= limit {
                break;
            }
        }
        if shown == 0 {
            out.push_str("- none\n");
        }
        out
    }

    pub async fn last_error_hint(&self) -> Option<String> {
        let ring = self.console.lock().await;
        ring.iter().rev().find_map(|entry| {
            (entry.level == "error" || entry.level == "exception")
                .then(|| format!("page_error: [{}] {}", entry.level, entry.text))
        })
    }
}

async fn push(ring: &NetworkRing, entry: NetworkEntry) {
    let mut guard = ring.lock().await;
    if guard.len() >= RING_MAX {
        guard.pop_front();
    }
    guard.push_back(entry);
}

async fn push_console(ring: &ConsoleRing, entry: ConsoleEntry) {
    let mut guard = ring.lock().await;
    if guard.len() >= RING_MAX {
        guard.pop_front();
    }
    guard.push_back(entry);
}

fn render_args(args: &[RemoteObject]) -> String {
    let rendered: Vec<String> = args
        .iter()
        .map(|arg| {
            arg.value
                .as_ref()
                .map(|value| match value.as_str() {
                    Some(text) => text.to_owned(),
                    None => value.to_string(),
                })
                .or_else(|| arg.description.clone())
                .unwrap_or_default()
        })
        .collect();
    cap(&rendered.join(" "), CONSOLE_TEXT_MAX)
}

fn cap(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}
