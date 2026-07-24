use std::collections::VecDeque;
use std::sync::Arc;

use chromiumoxide::Page;
use chromiumoxide::cdp::browser_protocol::page::{
    DialogType, EventJavascriptDialogOpening, HandleJavaScriptDialogParams,
};
use futures::StreamExt as _;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

const DIALOG_LOG_MAX: usize = 8;
const DIALOG_MESSAGE_MAX: usize = 120;

type DialogLog = Arc<Mutex<VecDeque<String>>>;

pub struct DialogGuard {
    task: JoinHandle<()>,
    log: DialogLog,
}

impl DialogGuard {
    pub async fn spawn(page: &Page) -> Self {
        let log: DialogLog = Arc::new(Mutex::new(VecDeque::new()));
        let task = match page.event_listener::<EventJavascriptDialogOpening>().await {
            Ok(mut events) => {
                let page = page.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    while let Some(event) = events.next().await {
                        let accept = matches!(event.r#type, DialogType::Beforeunload);
                        let _ = page
                            .execute(HandleJavaScriptDialogParams::new(accept))
                            .await;
                        record(&log, &event, accept).await;
                    }
                })
            }
            Err(_) => tokio::spawn(std::future::ready(())),
        };
        Self { task, log }
    }

    pub async fn drain(&self) -> Vec<String> {
        let mut guard = self.log.lock().await;
        guard.drain(..).collect()
    }

    pub fn abort(&self) {
        self.task.abort();
    }
}

async fn record(log: &DialogLog, event: &EventJavascriptDialogOpening, accepted: bool) {
    let disposition = if accepted { "accepted" } else { "dismissed" };
    let kind = dialog_kind(&event.r#type);
    let message: String = event.message.chars().take(DIALOG_MESSAGE_MAX).collect();
    let entry = format!("{kind} auto-{disposition}: \"{message}\"");
    let mut guard = log.lock().await;
    if guard.len() >= DIALOG_LOG_MAX {
        guard.pop_front();
    }
    guard.push_back(entry);
}

fn dialog_kind(kind: &DialogType) -> &'static str {
    match kind {
        DialogType::Alert => "alert",
        DialogType::Confirm => "confirm",
        DialogType::Prompt => "prompt",
        DialogType::Beforeunload => "beforeunload",
    }
}
