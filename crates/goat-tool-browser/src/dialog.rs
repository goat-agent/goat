use std::collections::VecDeque;
use std::sync::Arc;

use chromiumoxide_cdp::cdp::browser_protocol::page::{
    DialogType, EventJavascriptDialogOpening, HandleJavaScriptDialogParams,
};
use goat_api::CdpEvent;
use tokio::sync::{Mutex, broadcast};
use tokio::task::JoinHandle;

use crate::cdp::{Cdp, decode, next_event};

const DIALOG_LOG_MAX: usize = 8;
const DIALOG_MESSAGE_MAX: usize = 120;

type DialogLog = Arc<Mutex<VecDeque<String>>>;

pub struct DialogGuard {
    task: JoinHandle<()>,
    log: DialogLog,
}

impl DialogGuard {
    pub fn spawn(cdp: &Cdp) -> Self {
        let log: DialogLog = Arc::new(Mutex::new(VecDeque::new()));
        let task = tokio::spawn(watch(cdp.clone(), cdp.events(), log.clone()));
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

async fn watch(cdp: Cdp, mut events: broadcast::Receiver<CdpEvent>, log: DialogLog) {
    while let Some(event) = next_event(&mut events).await {
        let Some(opening) = decode::<EventJavascriptDialogOpening>(&event) else {
            continue;
        };
        let accept = matches!(opening.r#type, DialogType::Beforeunload);
        let _ = cdp.send(HandleJavaScriptDialogParams::new(accept)).await;
        record(&log, &opening, accept).await;
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
