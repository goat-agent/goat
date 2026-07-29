use goat_integration::{IntegrationError, IntegrationResult};
use serde_json::Value;

const ENVELOPE_KEYS: [&str; 5] = ["content", "notes", "results", "items", "data"];
const MAX_NAMED_PARTICIPANTS: usize = 4;

#[derive(Clone, Debug)]
pub struct Note {
    pub key: String,
    pub title: String,
    pub updated_at: String,
    pub duration_seconds: u64,
    pub source_type: String,
    pub participants: Vec<String>,
    pub raw: Value,
}

impl Note {
    pub fn summary(&self) -> String {
        let head = if self.title.is_empty() {
            "untitled note".to_string()
        } else {
            squeeze(&self.title, 160)
        };
        let mut parts = Vec::new();
        if let Some(detail) = self.detail() {
            parts.push(detail);
        }
        if let Some(people) = self.people() {
            parts.push(people);
        }
        if parts.is_empty() {
            head
        } else {
            format!("{head} · {}", parts.join(", "))
        }
    }

    fn detail(&self) -> Option<String> {
        if self.duration_seconds > 0 {
            return Some(human_duration(self.duration_seconds));
        }
        if self.source_type.is_empty() {
            None
        } else {
            Some(self.source_type.clone())
        }
    }

    fn people(&self) -> Option<String> {
        if self.participants.is_empty() {
            return None;
        }
        let named: Vec<&str> = self
            .participants
            .iter()
            .take(MAX_NAMED_PARTICIPANTS)
            .map(String::as_str)
            .collect();
        let rest = self.participants.len() - named.len();
        let joined = named.join(", ");
        Some(if rest > 0 {
            format!("{joined} +{rest}")
        } else {
            joined
        })
    }
}

fn human_duration(seconds: u64) -> String {
    if seconds >= 3600 {
        format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60)
    } else if seconds >= 60 {
        format!("{}m", seconds / 60)
    } else {
        format!("{seconds}s")
    }
}

pub struct FetchPage {
    pub notes: Vec<Note>,
}

pub fn parse_page(data: &Value) -> IntegrationResult<FetchPage> {
    Ok(FetchPage {
        notes: parse_notes(data)?,
    })
}

pub fn parse_notes(data: &Value) -> IntegrationResult<Vec<Note>> {
    note_array(data)
        .ok_or_else(|| {
            IntegrationError::Service(format!(
                "tiro response has no note list: {}",
                squeeze(&data.to_string(), 400)
            ))
        })?
        .iter()
        .map(parse_note)
        .collect()
}

fn note_array(data: &Value) -> Option<&Vec<Value>> {
    if let Some(array) = data.as_array() {
        return Some(array);
    }
    if let Some(array) = direct_arrays(data).find(|array| !array.is_empty()) {
        return Some(array);
    }
    if let Some(array) = direct_arrays(data).next() {
        return Some(array);
    }
    ENVELOPE_KEYS
        .iter()
        .filter_map(|key| data.get(key))
        .find_map(|nested| {
            direct_arrays(nested)
                .find(|array| !array.is_empty())
                .or_else(|| direct_arrays(nested).next())
        })
}

fn direct_arrays(data: &Value) -> impl Iterator<Item = &Vec<Value>> {
    ENVELOPE_KEYS
        .iter()
        .filter_map(move |key| data.get(*key).and_then(Value::as_array))
}

fn parse_note(node: &Value) -> IntegrationResult<Note> {
    let key = string_field(node, "noteGuid")
        .or_else(|| string_field(node, "note_guid"))
        .or_else(|| string_field(node, "guid"))
        .ok_or_else(|| IntegrationError::Service("tiro note missing `noteGuid`".into()))?;
    let updated_at = string_field(node, "updatedAt")
        .or_else(|| string_field(node, "updated_at"))
        .or_else(|| string_field(node, "createdAt"))
        .or_else(|| string_field(node, "created_at"))
        .unwrap_or_default();
    Ok(Note {
        key,
        title: string_field(node, "title").unwrap_or_default(),
        updated_at,
        duration_seconds: numeric_field(node, "recordingDurationSeconds")
            .or_else(|| numeric_field(node, "recording_duration_seconds"))
            .unwrap_or_default(),
        source_type: string_field(node, "sourceType")
            .or_else(|| string_field(node, "source_type"))
            .unwrap_or_default(),
        participants: participants(node),
        raw: node.clone(),
    })
}

fn participants(node: &Value) -> Vec<String> {
    node.get("participants")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| match value {
                    Value::String(text) => Some(text.clone()).filter(|text| !text.is_empty()),
                    other => string_field(other, "name").or_else(|| string_field(other, "email")),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn string_field(node: &Value, key: &str) -> Option<String> {
    node.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn numeric_field(node: &Value, key: &str) -> Option<u64> {
    match node.get(key) {
        Some(Value::Number(value)) => value.as_f64().filter(|n| *n > 0.0).map(|n| n as u64),
        Some(Value::String(value)) => value.parse().ok(),
        _ => None,
    }
}

fn squeeze(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let kept: String = flat.chars().take(max).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn note_node(guid: &str, updated_at: &str) -> Value {
        json!({
            "noteGuid": guid,
            "title": "OKR Q2 Planning",
            "webUrl": format!("https://platform.tiro.ooo/notes/{guid}"),
            "createdAt": "2026-04-15T10:00:00Z",
            "updatedAt": updated_at,
            "recordingDurationSeconds": 3625,
            "sourceType": "live-voice",
            "participants": [
                { "name": "Alice Kim", "email": "alice@example.com" },
                { "name": "Bob Park", "email": null }
            ]
        })
    }

    #[test]
    fn parses_the_documented_envelope() {
        let page = json!({
            "content": [note_node("abc-123-def", "2026-04-15T11:30:00Z")],
            "notes": [],
            "total": 137,
            "totalSize": 1,
            "nextCursor": null
        });
        let notes = parse_notes(&page).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].key, "abc-123-def");
        assert_eq!(notes[0].updated_at, "2026-04-15T11:30:00Z");
        assert_eq!(notes[0].duration_seconds, 3625);
    }

    #[test]
    fn an_empty_preferred_key_falls_through_to_the_legacy_alias() {
        let page = json!({
            "content": [],
            "notes": [note_node("legacy-1", "2026-04-15T11:30:00Z")]
        });
        assert_eq!(parse_notes(&page).unwrap()[0].key, "legacy-1");
    }

    #[test]
    fn parses_bare_arrays_and_nested_envelopes() {
        let bare = json!([note_node("bare-1", "2026-04-15T11:30:00Z")]);
        assert_eq!(parse_notes(&bare).unwrap()[0].key, "bare-1");

        let nested = json!({ "data": { "content": [note_node("deep-1", "t")] } });
        assert_eq!(parse_notes(&nested).unwrap()[0].key, "deep-1");
    }

    #[test]
    fn an_empty_page_parses_to_no_notes() {
        let page = json!({ "content": [], "notes": [], "totalSize": 0 });
        assert!(parse_notes(&page).unwrap().is_empty());
    }

    #[test]
    fn updated_at_falls_back_to_created_at_so_the_settle_gate_still_converges() {
        let node = json!([{ "noteGuid": "n-1", "createdAt": "2026-04-15T10:00:00Z" }]);
        let notes = parse_notes(&node).unwrap();
        assert_eq!(notes[0].updated_at, "2026-04-15T10:00:00Z");
    }

    #[test]
    fn unknown_shapes_and_guidless_notes_error() {
        assert!(parse_notes(&json!({ "unexpected": true })).is_err());
        assert!(parse_notes(&json!("text blob")).is_err());
        assert!(parse_notes(&json!([{ "title": "no identity" }])).is_err());
    }

    #[test]
    fn summary_carries_title_duration_and_participants() {
        let note = &parse_notes(&json!([note_node("abc", "t")])).unwrap()[0];
        assert_eq!(
            note.summary(),
            "OKR Q2 Planning · 1h 0m, Alice Kim, Bob Park"
        );
    }

    #[test]
    fn summary_uses_the_source_type_when_nothing_was_recorded() {
        let node = json!([{ "noteGuid": "n-1", "title": "Pasted brief", "sourceType": "text" }]);
        assert_eq!(
            parse_notes(&node).unwrap()[0].summary(),
            "Pasted brief · text"
        );
    }

    #[test]
    fn summary_degrades_and_clamps() {
        let bare = json!([{ "noteGuid": "n-1" }]);
        assert_eq!(parse_notes(&bare).unwrap()[0].summary(), "untitled note");

        let long = json!([{ "noteGuid": "n-1", "title": "a ".repeat(200) }]);
        assert!(parse_notes(&long).unwrap()[0].summary().ends_with('…'));
    }

    #[test]
    fn long_attendee_lists_are_capped() {
        let people: Vec<Value> = (1..=7)
            .map(|n| json!({ "name": format!("P{n}") }))
            .collect();
        let node = json!([{ "noteGuid": "n-1", "title": "Standup", "participants": people }]);
        assert_eq!(
            parse_notes(&node).unwrap()[0].summary(),
            "Standup · P1, P2, P3, P4 +3"
        );
    }

    #[test]
    fn participants_accept_plain_strings_and_fall_back_to_email() {
        let node = json!([{
            "noteGuid": "n-1",
            "participants": ["Carol", { "name": null, "email": "dave@example.com" }]
        }]);
        assert_eq!(
            parse_notes(&node).unwrap()[0].participants,
            vec!["Carol".to_string(), "dave@example.com".to_string()]
        );
    }

    #[test]
    fn durations_render_at_every_scale() {
        assert_eq!(human_duration(3625), "1h 0m");
        assert_eq!(human_duration(3600 * 2 + 61), "2h 1m");
        assert_eq!(human_duration(125), "2m");
        assert_eq!(human_duration(45), "45s");
    }
}
