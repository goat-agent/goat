use serde::Deserialize;

use crate::error::BrowserError;
use crate::snapshot::{RefParts, parse_ref};

#[derive(Debug, Deserialize)]
struct Input {
    action: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default, rename = "ref")]
    reference: Option<String>,
    #[serde(default)]
    snapshot_id: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    submit: bool,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    js: Option<String>,
    #[serde(default)]
    direction: Option<String>,
    #[serde(default)]
    amount: Option<i64>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    max_chars: Option<usize>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    filter: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    op: Option<String>,
    #[serde(default)]
    index: Option<usize>,
    #[serde(default)]
    level: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserRef {
    pub snapshot_id: Option<String>,
    pub reference: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Navigate {
        url: String,
    },
    Snapshot,
    Click {
        reference: BrowserRef,
    },
    Fill {
        reference: BrowserRef,
        text: String,
        submit: bool,
    },
    Select {
        reference: BrowserRef,
        value: String,
    },
    Hover {
        reference: BrowserRef,
    },
    Drag {
        from: BrowserRef,
        to: BrowserRef,
    },
    Upload {
        reference: BrowserRef,
        path: String,
    },
    PressKey {
        key: String,
    },
    Scroll {
        direction: ScrollDirection,
        amount: Option<i64>,
    },
    GoBack,
    GoForward,
    FindText {
        query: String,
        max_chars: Option<usize>,
    },
    Inspect {
        reference: BrowserRef,
        max_chars: Option<usize>,
    },
    ReadViewport {
        max_chars: Option<usize>,
    },
    ReadContent {
        max_chars: Option<usize>,
    },
    ReadNetwork {
        filter: Option<String>,
        limit: Option<usize>,
    },
    ReadConsole {
        level: Option<String>,
        limit: Option<usize>,
    },
    Storage {
        op: StorageOp,
        name: Option<String>,
        value: Option<String>,
    },
    Tab {
        op: TabOp,
        index: Option<usize>,
        url: Option<String>,
    },
    WaitFor {
        text: Option<String>,
        state: Option<String>,
        timeout_ms: Option<u64>,
    },
    Screenshot,
    DebugEval {
        js: String,
    },
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDirection {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageOp {
    GetCookies,
    SetCookie,
    GetLocal,
    SetLocal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabOp {
    List,
    Switch,
    Close,
    New,
}

pub fn parse(input: &str) -> Result<Action, BrowserError> {
    let raw: Input = serde_json::from_str(input)
        .map_err(|err| BrowserError::Input(format!("invalid tool input: {err}")))?;
    let action = match raw.action.as_str() {
        "navigate" => Action::Navigate {
            url: require(raw.url, "navigate", "url")?,
        },
        "snapshot" => Action::Snapshot,
        "click" => Action::Click {
            reference: require_ref(raw.reference, raw.snapshot_id, "click")?,
        },
        "fill" => Action::Fill {
            reference: require_ref(raw.reference, raw.snapshot_id.clone(), "fill")?,
            text: require(raw.text, "fill", "text")?,
            submit: raw.submit,
        },
        "select" => Action::Select {
            reference: require_ref(raw.reference, raw.snapshot_id, "select")?,
            value: require(raw.value, "select", "value")?,
        },
        "hover" => Action::Hover {
            reference: require_ref(raw.reference, raw.snapshot_id, "hover")?,
        },
        "drag" => Action::Drag {
            from: require_ref(raw.from, raw.snapshot_id.clone(), "drag")?,
            to: require_ref(raw.to, raw.snapshot_id, "drag")?,
        },
        "upload" => Action::Upload {
            reference: require_ref(raw.reference, raw.snapshot_id, "upload")?,
            path: require(raw.path, "upload", "path")?,
        },
        "press_key" => Action::PressKey {
            key: require(raw.key, "press_key", "key")?,
        },
        "scroll" => Action::Scroll {
            direction: require_direction(raw.direction)?,
            amount: raw.amount,
        },
        "go_back" => Action::GoBack,
        "go_forward" => Action::GoForward,
        "find_text" => Action::FindText {
            query: require(raw.query, "find_text", "query")?,
            max_chars: raw.max_chars,
        },
        "inspect" => Action::Inspect {
            reference: require_ref(raw.reference, raw.snapshot_id, "inspect")?,
            max_chars: raw.max_chars,
        },
        "read_viewport" => Action::ReadViewport {
            max_chars: raw.max_chars,
        },
        "read_content" => Action::ReadContent {
            max_chars: raw.max_chars,
        },
        "read_network" => Action::ReadNetwork {
            filter: raw.filter,
            limit: raw.limit,
        },
        "read_console" => Action::ReadConsole {
            level: raw.level,
            limit: raw.limit,
        },
        "storage" => Action::Storage {
            op: require_storage_op(raw.op)?,
            name: raw.name,
            value: raw.value,
        },
        "tab" => Action::Tab {
            op: require_tab_op(raw.op)?,
            index: raw.index,
            url: raw.url,
        },
        "wait_for" => Action::WaitFor {
            text: raw.text,
            state: raw.state,
            timeout_ms: raw.timeout_ms,
        },
        "screenshot" => Action::Screenshot,
        "close" => Action::Close,
        "debug_eval" => Action::DebugEval {
            js: require(raw.js, "debug_eval", "js")?,
        },
        other => {
            return Err(BrowserError::Input(format!(
                "unknown action '{other}'; valid actions: navigate, snapshot, click, fill, select, hover, drag, upload, press_key, scroll, go_back, go_forward, find_text, inspect, read_viewport, read_content, read_network, read_console, storage, tab, wait_for, screenshot, close, debug_eval"
            )));
        }
    };
    Ok(action)
}

fn require(value: Option<String>, action: &str, field: &str) -> Result<String, BrowserError> {
    value
        .filter(|s| !s.is_empty())
        .ok_or_else(|| BrowserError::Input(format!("action '{action}' requires '{field}'")))
}

fn require_ref(
    value: Option<String>,
    snapshot_id: Option<String>,
    action: &str,
) -> Result<BrowserRef, BrowserError> {
    let raw = require(value, action, "ref")?;
    let RefParts {
        snapshot_id: parsed_snapshot,
        reference,
    } = parse_ref(&raw).ok_or_else(|| {
        BrowserError::Input(format!(
            "action '{action}' got an invalid ref '{raw}'; refs look like s12:e3 and come from the latest snapshot"
        ))
    })?;
    let snapshot_id = parsed_snapshot.or(snapshot_id);
    Ok(BrowserRef {
        snapshot_id,
        reference,
    })
}

fn require_direction(value: Option<String>) -> Result<ScrollDirection, BrowserError> {
    let direction = require(value, "scroll", "direction")?;
    match direction.as_str() {
        "up" => Ok(ScrollDirection::Up),
        "down" => Ok(ScrollDirection::Down),
        "left" => Ok(ScrollDirection::Left),
        "right" => Ok(ScrollDirection::Right),
        other => Err(BrowserError::Input(format!(
            "unknown scroll direction '{other}'; valid directions: up, down, left, right"
        ))),
    }
}

fn require_storage_op(value: Option<String>) -> Result<StorageOp, BrowserError> {
    let op = require(value, "storage", "op")?;
    match op.as_str() {
        "get_cookies" => Ok(StorageOp::GetCookies),
        "set_cookie" => Ok(StorageOp::SetCookie),
        "get_local" => Ok(StorageOp::GetLocal),
        "set_local" => Ok(StorageOp::SetLocal),
        other => Err(BrowserError::Input(format!(
            "unknown storage op '{other}'; valid ops: get_cookies, set_cookie, get_local, set_local"
        ))),
    }
}

fn require_tab_op(value: Option<String>) -> Result<TabOp, BrowserError> {
    let op = require(value, "tab", "op")?;
    match op.as_str() {
        "list" => Ok(TabOp::List),
        "switch" => Ok(TabOp::Switch),
        "close" => Ok(TabOp::Close),
        "new" => Ok(TabOp::New),
        other => Err(BrowserError::Input(format!(
            "unknown tab op '{other}'; valid ops: list, switch, close, new"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, BrowserRef, ScrollDirection, StorageOp, TabOp, parse};

    #[test]
    fn parses_navigate() {
        let action = parse(r#"{"action":"navigate","url":"example.com"}"#).unwrap();
        assert_eq!(
            action,
            Action::Navigate {
                url: "example.com".to_owned()
            }
        );
    }

    #[test]
    fn parses_fill_with_submit() {
        let action = parse(r#"{"action":"fill","ref":"s2:e3","text":"hi","submit":true}"#).unwrap();
        assert_eq!(
            action,
            Action::Fill {
                reference: BrowserRef {
                    snapshot_id: Some("s2".to_owned()),
                    reference: "e3".to_owned(),
                },
                text: "hi".to_owned(),
                submit: true,
            }
        );
    }

    #[test]
    fn parses_scroll() {
        let action = parse(r#"{"action":"scroll","direction":"down","amount":640}"#).unwrap();
        assert_eq!(
            action,
            Action::Scroll {
                direction: ScrollDirection::Down,
                amount: Some(640),
            }
        );
    }

    #[test]
    fn parses_hover_and_upload() {
        assert_eq!(
            parse(r#"{"action":"hover","ref":"s1:e2"}"#).unwrap(),
            Action::Hover {
                reference: BrowserRef {
                    snapshot_id: Some("s1".to_owned()),
                    reference: "e2".to_owned(),
                },
            }
        );
        assert_eq!(
            parse(r#"{"action":"upload","ref":"e5","path":"/tmp/a.png"}"#).unwrap(),
            Action::Upload {
                reference: BrowserRef {
                    snapshot_id: None,
                    reference: "e5".to_owned(),
                },
                path: "/tmp/a.png".to_owned(),
            }
        );
    }

    #[test]
    fn parses_storage_and_tab() {
        assert_eq!(
            parse(r#"{"action":"storage","op":"set_cookie","name":"sid","value":"abc"}"#).unwrap(),
            Action::Storage {
                op: StorageOp::SetCookie,
                name: Some("sid".to_owned()),
                value: Some("abc".to_owned()),
            }
        );
        assert_eq!(
            parse(r#"{"action":"tab","op":"switch","index":2}"#).unwrap(),
            Action::Tab {
                op: TabOp::Switch,
                index: Some(2),
                url: None,
            }
        );
    }

    #[test]
    fn parses_read_network() {
        assert_eq!(
            parse(r#"{"action":"read_network","filter":"api","limit":10}"#).unwrap(),
            Action::ReadNetwork {
                filter: Some("api".to_owned()),
                limit: Some(10),
            }
        );
    }

    #[test]
    fn rejects_bad_storage_op() {
        let err = parse(r#"{"action":"storage","op":"wipe"}"#).unwrap_err();
        assert!(err.to_string().contains("unknown storage op"));
    }

    #[test]
    fn rejects_legacy_type() {
        let err = parse(r#"{"action":"type","ref":"e3","text":"hi"}"#).unwrap_err();
        assert!(err.to_string().contains("unknown action"));
    }

    #[test]
    fn click_requires_ref() {
        let err = parse(r#"{"action":"click"}"#).unwrap_err();
        assert!(err.to_string().contains("requires 'ref'"));
    }

    #[test]
    fn click_rejects_bad_ref() {
        let err = parse(r#"{"action":"click","ref":"e1' or '1"}"#).unwrap_err();
        assert!(err.to_string().contains("invalid ref"));
    }

    #[test]
    fn rejects_unknown_action() {
        let err = parse(r#"{"action":"teleport"}"#).unwrap_err();
        assert!(err.to_string().contains("unknown action"));
    }

    #[test]
    fn rejects_malformed_json() {
        let err = parse("not json").unwrap_err();
        assert!(err.to_string().contains("invalid tool input"));
    }
}
