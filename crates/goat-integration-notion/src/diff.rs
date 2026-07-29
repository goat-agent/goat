use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::parse::ViewRow;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchState {
    pub seen: BTreeMap<String, String>,
}

pub fn diff(prev: Option<&WatchState>, fetched: &[ViewRow]) -> (WatchState, Vec<ViewRow>) {
    let next = WatchState {
        seen: fetched
            .iter()
            .map(|row| (row.id.clone(), row.edited_at.clone()))
            .collect(),
    };
    let Some(prev) = prev else {
        return (next, Vec::new());
    };
    let entered = fetched
        .iter()
        .filter(|row| !prev.seen.contains_key(&row.id))
        .cloned()
        .collect();
    (next, entered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn row(id: &str, edited_at: &str) -> ViewRow {
        ViewRow {
            id: id.into(),
            title: format!("{id} title"),
            url: None,
            edited_at: edited_at.into(),
            raw: Value::Null,
        }
    }

    #[test]
    fn first_poll_baselines_without_changes() {
        let (next, entered) = diff(None, &[row("a", "t1")]);
        assert!(entered.is_empty());
        assert_eq!(next.seen.get("a").map(String::as_str), Some("t1"));
    }

    #[test]
    fn only_rows_entering_the_view_fire() {
        let prev = diff(None, &[row("a", "t1")]).0;
        let (next, entered) = diff(Some(&prev), &[row("a", "t2"), row("b", "t1")]);
        assert_eq!(entered.len(), 1);
        assert_eq!(entered[0].id, "b");
        assert_eq!(next.seen.get("a").map(String::as_str), Some("t2"));
    }

    #[test]
    fn rows_leaving_the_view_stay_silent_and_drop_out_of_state() {
        let prev = diff(None, &[row("a", "t1"), row("b", "t1")]).0;
        let (next, entered) = diff(Some(&prev), &[row("a", "t1")]);
        assert!(entered.is_empty());
        assert!(!next.seen.contains_key("b"));
    }

    #[test]
    fn re_entering_the_view_fires_again() {
        let prev = diff(None, &[row("a", "t1")]).0;
        let gone = diff(Some(&prev), &[]).0;
        let (_, entered) = diff(Some(&gone), &[row("a", "t2")]);
        assert_eq!(entered.len(), 1);
    }

    #[test]
    fn state_round_trips_through_json() {
        let state = diff(None, &[row("a", "t1")]).0;
        let raw = serde_json::to_string(&state).unwrap();
        let parsed: WatchState = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed, state);
    }
}
