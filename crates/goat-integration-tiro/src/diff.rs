use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::parse::Note;

pub const RETENTION: usize = 500;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchState {
    pub seen: BTreeMap<String, String>,
    #[serde(default)]
    pub pending: BTreeMap<String, String>,
}

pub fn diff(prev: Option<&WatchState>, fetched: &[Note]) -> (WatchState, Vec<Note>) {
    let Some(prev) = prev else {
        let mut seen = BTreeMap::new();
        for note in fetched {
            seen.insert(note.key.clone(), note.updated_at.clone());
        }
        prune(&mut seen);
        return (
            WatchState {
                seen,
                pending: BTreeMap::new(),
            },
            Vec::new(),
        );
    };

    let mut seen = prev.seen.clone();
    let mut pending = BTreeMap::new();
    let mut fresh = Vec::new();
    for note in fetched {
        if prev.seen.contains_key(&note.key) {
            seen.insert(note.key.clone(), note.updated_at.clone());
        } else if prev.pending.get(&note.key) == Some(&note.updated_at) {
            seen.insert(note.key.clone(), note.updated_at.clone());
            fresh.push(note.clone());
        } else {
            pending.insert(note.key.clone(), note.updated_at.clone());
        }
    }
    prune(&mut seen);
    prune(&mut pending);
    (WatchState { seen, pending }, fresh)
}

pub fn hold_back(state: &mut WatchState, key: &str, updated_at: &str) {
    state.seen.remove(key);
    state
        .pending
        .insert(key.to_string(), updated_at.to_string());
}

fn prune(entries: &mut BTreeMap<String, String>) {
    if entries.len() <= RETENTION {
        return;
    }
    let mut ranked: Vec<(String, String)> = entries
        .iter()
        .map(|(key, stamp)| (key.clone(), stamp.clone()))
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));
    ranked.truncate(RETENTION);
    *entries = ranked.into_iter().collect();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn note(guid: &str, updated_at: &str) -> Note {
        Note {
            key: guid.into(),
            title: "OKR Q2 Planning".into(),
            updated_at: updated_at.into(),
            duration_seconds: 3600,
            source_type: "live-voice".into(),
            participants: vec!["Alice Kim".into()],
            raw: Value::Null,
        }
    }

    #[test]
    fn first_poll_baselines_without_events() {
        let (next, fresh) = diff(None, &[note("n-1", "t1")]);
        assert!(
            fresh.is_empty(),
            "cold start must not brief months of meetings",
        );
        assert_eq!(next.seen.get("n-1").map(String::as_str), Some("t1"));
        assert!(next.pending.is_empty());
    }

    #[test]
    fn a_note_never_fires_on_the_poll_that_first_sees_it() {
        let base = diff(None, &[]).0;
        let (next, fresh) = diff(Some(&base), &[note("n-1", "t1")]);
        assert!(
            fresh.is_empty(),
            "a note is still being written when it first appears",
        );
        assert_eq!(next.pending.get("n-1").map(String::as_str), Some("t1"));
        assert!(!next.seen.contains_key("n-1"));
    }

    #[test]
    fn a_note_fires_once_its_timestamp_stops_moving() {
        let base = diff(None, &[]).0;
        let (waiting, _) = diff(Some(&base), &[note("n-1", "t1")]);
        let (settled, fresh) = diff(Some(&waiting), &[note("n-1", "t1")]);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].key, "n-1");
        assert_eq!(settled.seen.get("n-1").map(String::as_str), Some("t1"));
        assert!(settled.pending.is_empty());
    }

    #[test]
    fn a_note_still_being_summarized_keeps_waiting() {
        let base = diff(None, &[]).0;
        let (a, _) = diff(Some(&base), &[note("n-1", "t1")]);
        let (b, fresh) = diff(Some(&a), &[note("n-1", "t2")]);
        assert!(fresh.is_empty(), "a moving timestamp means work in flight");
        assert_eq!(b.pending.get("n-1").map(String::as_str), Some("t2"));
        let (_, fresh) = diff(Some(&b), &[note("n-1", "t2")]);
        assert_eq!(fresh.len(), 1);
    }

    #[test]
    fn a_settled_note_never_fires_again_even_when_edited_later() {
        let base = diff(None, &[]).0;
        let (a, _) = diff(Some(&base), &[note("n-1", "t1")]);
        let (b, _) = diff(Some(&a), &[note("n-1", "t1")]);
        let (c, fresh) = diff(Some(&b), &[note("n-1", "t9")]);
        assert!(fresh.is_empty());
        assert_eq!(c.seen.get("n-1").map(String::as_str), Some("t9"));
        let (_, fresh) = diff(Some(&c), &[note("n-1", "t9")]);
        assert!(fresh.is_empty());
    }

    #[test]
    fn notes_that_scroll_off_the_page_stay_remembered() {
        let base = diff(None, &[note("n-1", "t1")]).0;
        let (next, _) = diff(Some(&base), &[note("n-2", "t2")]);
        assert!(next.seen.contains_key("n-1"));
        let (_, fresh) = diff(Some(&next), &[note("n-1", "t1")]);
        assert!(fresh.is_empty());
    }

    #[test]
    fn a_note_that_vanishes_before_settling_stops_waiting() {
        let base = diff(None, &[]).0;
        let (a, _) = diff(Some(&base), &[note("n-1", "t1")]);
        let (b, fresh) = diff(Some(&a), &[]);
        assert!(fresh.is_empty());
        assert!(b.pending.is_empty());
    }

    #[test]
    fn hold_back_puts_a_failed_announcement_back_in_the_queue() {
        let base = diff(None, &[]).0;
        let (waiting, _) = diff(Some(&base), &[note("n-1", "t1")]);
        let (mut settled, fresh) = diff(Some(&waiting), &[note("n-1", "t1")]);
        assert_eq!(fresh.len(), 1);

        hold_back(&mut settled, "n-1", "t1");
        assert!(!settled.seen.contains_key("n-1"));
        let (_, fresh) = diff(Some(&settled), &[note("n-1", "t1")]);
        assert_eq!(fresh.len(), 1, "a dropped note must be retried");
    }

    #[test]
    fn retention_caps_growth_and_keeps_the_newest() {
        let mut entries = BTreeMap::new();
        for n in 0..(RETENTION + 50) {
            let stamp = format!("{n:016}");
            entries.insert(format!("n-{stamp}"), stamp);
        }
        prune(&mut entries);
        assert_eq!(entries.len(), RETENTION);
        let newest = RETENTION + 49;
        assert!(entries.contains_key(&format!("n-{newest:016}")));
        assert!(!entries.contains_key(&format!("n-{:016}", 0_usize)));
    }

    #[test]
    fn state_round_trips_through_json() {
        let base = diff(None, &[]).0;
        let (state, _) = diff(Some(&base), &[note("n-1", "t1")]);
        let raw = serde_json::to_string(&state).unwrap();
        let parsed: WatchState = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed, state);
    }

    #[test]
    fn state_written_before_the_settle_gate_still_loads() {
        let parsed: WatchState = serde_json::from_str(r#"{"seen":{"n-1":"t1"}}"#).unwrap();
        assert_eq!(parsed.seen.get("n-1").map(String::as_str), Some("t1"));
        assert!(parsed.pending.is_empty());
    }
}
