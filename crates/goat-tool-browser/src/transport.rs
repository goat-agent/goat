use std::future::Future;
use std::pin::Pin;

use goat_api::{BrowserCommand, CdpEvent, HostBrowserOutput};
use tokio::sync::broadcast;

use crate::error::BrowserError;

pub type TransportFuture<'a> =
    Pin<Box<dyn Future<Output = Result<HostBrowserOutput, BrowserError>> + Send + 'a>>;

pub trait Transport: Send + Sync {
    fn call(&self, command: BrowserCommand) -> TransportFuture<'_>;
    fn events(&self) -> broadcast::Receiver<CdpEvent>;
}

#[cfg(test)]
pub mod fake {
    use std::sync::Mutex;

    use goat_api::BrowserTab;
    use serde_json::{Value, json};

    use super::{
        BrowserCommand, CdpEvent, HostBrowserOutput, Transport, TransportFuture, broadcast,
    };

    pub struct FakeBrowser {
        rules: Mutex<Vec<(String, Value)>>,
        tabs: Mutex<Vec<BrowserTab>>,
        calls: Mutex<Vec<(String, Value)>>,
        events: broadcast::Sender<CdpEvent>,
    }

    impl FakeBrowser {
        pub fn new() -> Self {
            Self {
                rules: Mutex::new(Vec::new()),
                tabs: Mutex::new(vec![tab(1, "https://start.test", true)]),
                calls: Mutex::new(Vec::new()),
                events: broadcast::channel(64).0,
            }
        }

        pub fn on(&self, needle: &str, result: Value) -> &Self {
            let mut rules = self.rules.lock().unwrap();
            match rules.iter_mut().find(|(known, _)| known == needle) {
                Some(rule) => rule.1 = result,
                None => rules.push((needle.to_owned(), result)),
            }
            self
        }

        pub fn returns(&self, needle: &str, value: &Value) -> &Self {
            self.on(
                needle,
                json!({ "result": { "type": "object", "value": value } }),
            )
        }

        pub fn handle(&self, needle: &str) -> &Self {
            self.on(
                needle,
                json!({ "result": { "type": "object", "objectId": "o1" } }),
            )
        }

        pub fn set_tabs(&self, tabs: Vec<BrowserTab>) {
            *self.tabs.lock().unwrap() = tabs;
        }

        pub fn emit(&self, method: &str, params: Value) {
            let _ = self.events.send(CdpEvent {
                method: method.to_owned(),
                params,
            });
        }

        pub fn methods(&self) -> Vec<String> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(|(method, _)| method.clone())
                .collect()
        }

        pub fn sent(&self, method: &str) -> Vec<Value> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(name, _)| name == method)
                .map(|(_, params)| params.clone())
                .collect()
        }

        fn answer(&self, method: &str, params: &Value) -> Value {
            let haystack = params.to_string();
            let rules = self.rules.lock().unwrap();
            for (needle, result) in rules.iter() {
                if haystack.contains(needle.as_str()) {
                    return result.clone();
                }
            }
            match method {
                "Runtime.evaluate" | "Runtime.callFunctionOn" => {
                    json!({ "result": { "type": "undefined" } })
                }
                _ => json!({}),
            }
        }
    }

    pub fn tab(id: i64, url: &str, selected: bool) -> BrowserTab {
        BrowserTab {
            id,
            url: url.to_owned(),
            title: "Page".to_owned(),
            selected,
        }
    }

    impl Transport for FakeBrowser {
        fn call(&self, command: BrowserCommand) -> TransportFuture<'_> {
            let output = match command {
                BrowserCommand::Cdp { method, params } => {
                    let result = self.answer(&method, &params);
                    self.calls.lock().unwrap().push((method, params));
                    HostBrowserOutput::Cdp { result }
                }
                BrowserCommand::TabList {} => HostBrowserOutput::Tabs {
                    tabs: self.tabs.lock().unwrap().clone(),
                },
                BrowserCommand::TabSelect { id } => {
                    let mut tabs = self.tabs.lock().unwrap();
                    for tab in tabs.iter_mut() {
                        tab.selected = tab.id == id;
                    }
                    HostBrowserOutput::Tabs { tabs: tabs.clone() }
                }
                BrowserCommand::TabClose { id } => {
                    let mut tabs = self.tabs.lock().unwrap();
                    tabs.retain(|tab| tab.id != id);
                    if !tabs.iter().any(|tab| tab.selected)
                        && let Some(first) = tabs.first_mut()
                    {
                        first.selected = true;
                    }
                    HostBrowserOutput::Tabs { tabs: tabs.clone() }
                }
                BrowserCommand::TabOpen { url } => {
                    let mut tabs = self.tabs.lock().unwrap();
                    let id = tabs.iter().map(|tab| tab.id).max().unwrap_or(0) + 1;
                    for tab in tabs.iter_mut() {
                        tab.selected = false;
                    }
                    tabs.push(tab(id, &url, true));
                    HostBrowserOutput::Tabs { tabs: tabs.clone() }
                }
                BrowserCommand::Detach {} => HostBrowserOutput::Detached {},
            };
            Box::pin(async move { Ok(output) })
        }

        fn events(&self) -> broadcast::Receiver<CdpEvent> {
            self.events.subscribe()
        }
    }
}
