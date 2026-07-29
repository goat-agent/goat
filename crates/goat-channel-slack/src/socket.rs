use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
struct Frame {
    #[serde(rename = "type")]
    kind: Option<String>,
    envelope_id: Option<String>,
    payload: Option<Value>,
    reason: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Incoming {
    Hello,
    Disconnect {
        reason: String,
    },
    Event {
        envelope_id: String,
        event: Value,
    },
    Ignored {
        envelope_id: Option<String>,
        kind: String,
    },
    Unparsable,
}

pub(crate) fn classify(raw: &str) -> Incoming {
    let Ok(frame) = serde_json::from_str::<Frame>(raw) else {
        return Incoming::Unparsable;
    };
    let kind = frame.kind.unwrap_or_default();
    match kind.as_str() {
        "hello" => Incoming::Hello,
        "disconnect" => Incoming::Disconnect {
            reason: frame.reason.unwrap_or_else(|| "unspecified".to_string()),
        },
        "events_api" => {
            let event = frame
                .payload
                .as_ref()
                .and_then(|payload| payload.get("event"))
                .cloned();
            match (frame.envelope_id, event) {
                (Some(envelope_id), Some(event)) => Incoming::Event { envelope_id, event },
                (envelope_id, _) => Incoming::Ignored { envelope_id, kind },
            }
        }
        _ => Incoming::Ignored {
            envelope_id: frame.envelope_id,
            kind,
        },
    }
}

pub(crate) fn ack(envelope_id: &str) -> String {
    json!({ "envelope_id": envelope_id }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_is_recognised() {
        let raw = r#"{"type":"hello","num_connections":1,"connection_info":{"app_id":"A1"}}"#;
        assert_eq!(classify(raw), Incoming::Hello);
    }

    #[test]
    fn disconnect_carries_its_reason() {
        let raw = r#"{"type":"disconnect","reason":"refresh_requested"}"#;
        assert_eq!(
            classify(raw),
            Incoming::Disconnect {
                reason: "refresh_requested".to_string()
            }
        );
    }

    #[test]
    fn a_disconnect_without_a_reason_still_classifies() {
        assert_eq!(
            classify(r#"{"type":"disconnect"}"#),
            Incoming::Disconnect {
                reason: "unspecified".to_string()
            }
        );
    }

    #[test]
    fn an_events_api_frame_yields_the_inner_event() {
        let raw = r#"{
            "envelope_id":"env-1",
            "type":"events_api",
            "payload":{
                "team_id":"T1",
                "type":"event_callback",
                "event":{"type":"message","channel":"C1","user":"U1","text":"hi","ts":"1.1"}
            },
            "accepts_response_payload":false
        }"#;
        let Incoming::Event { envelope_id, event } = classify(raw) else {
            panic!("expected an event");
        };
        assert_eq!(envelope_id, "env-1");
        assert_eq!(event["type"], "message");
        assert_eq!(event["channel"], "C1");
    }

    #[test]
    fn an_events_api_frame_without_an_inner_event_is_ignored_but_keeps_its_envelope() {
        let raw = r#"{"envelope_id":"env-2","type":"events_api","payload":{"type":"x"}}"#;
        assert_eq!(
            classify(raw),
            Incoming::Ignored {
                envelope_id: Some("env-2".to_string()),
                kind: "events_api".to_string()
            }
        );
    }

    #[test]
    fn other_frame_types_are_ignored_but_still_ackable() {
        let raw = r#"{"envelope_id":"env-3","type":"slash_commands","payload":{"command":"/x"}}"#;
        assert_eq!(
            classify(raw),
            Incoming::Ignored {
                envelope_id: Some("env-3".to_string()),
                kind: "slash_commands".to_string()
            }
        );
    }

    #[test]
    fn an_unknown_future_frame_type_does_not_break_the_loop() {
        let raw = r#"{"type":"something_new","envelope_id":"env-4"}"#;
        assert_eq!(
            classify(raw),
            Incoming::Ignored {
                envelope_id: Some("env-4".to_string()),
                kind: "something_new".to_string()
            }
        );
    }

    #[test]
    fn garbage_is_reported_rather_than_panicking() {
        assert_eq!(classify("not json"), Incoming::Unparsable);
        assert_eq!(
            classify("{}"),
            Incoming::Ignored {
                envelope_id: None,
                kind: String::new()
            }
        );
    }

    #[test]
    fn the_ack_payload_is_exactly_the_envelope_id() {
        assert_eq!(ack("env-1"), r#"{"envelope_id":"env-1"}"#);
    }
}
