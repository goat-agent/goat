use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::parse::AssignedIssue;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchState {
    pub seen: BTreeMap<String, String>,
}

pub fn diff(
    prev: Option<&WatchState>,
    fetched: &[AssignedIssue],
) -> (WatchState, Vec<AssignedIssue>) {
    let next = WatchState {
        seen: fetched
            .iter()
            .map(|i| (i.id.clone(), i.updated_at.clone()))
            .collect(),
    };
    let Some(prev) = prev else {
        return (next, Vec::new());
    };
    let assigned = fetched
        .iter()
        .filter(|issue| !prev.seen.contains_key(&issue.id))
        .cloned()
        .collect();
    (next, assigned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn issue(id: &str, identifier: &str, updated_at: &str) -> AssignedIssue {
        AssignedIssue {
            id: id.into(),
            identifier: identifier.into(),
            title: format!("{identifier} title"),
            updated_at: updated_at.into(),
            status_type: "unstarted".into(),
            raw: Value::Null,
        }
    }

    #[test]
    fn first_poll_baselines_without_changes() {
        let fetched = [issue("a", "GOA-1", "t1")];
        let (next, assigned) = diff(None, &fetched);
        assert!(assigned.is_empty());
        assert_eq!(next.seen.get("a").map(String::as_str), Some("t1"));
    }

    #[test]
    fn only_new_ids_fire_and_updates_stay_silent() {
        let prev = diff(None, &[issue("a", "GOA-1", "t1")]).0;
        let fetched = [issue("a", "GOA-1", "t2"), issue("b", "GOA-2", "t1")];
        let (next, assigned) = diff(Some(&prev), &fetched);
        assert_eq!(assigned.len(), 1);
        assert_eq!(assigned[0].identifier, "GOA-2");
        assert_eq!(next.seen.get("a").map(String::as_str), Some("t2"));
    }

    #[test]
    fn unchanged_and_disappeared_issues_stay_silent() {
        let prev = diff(
            None,
            &[issue("a", "GOA-1", "t1"), issue("b", "GOA-2", "t1")],
        )
        .0;
        let (next, assigned) = diff(Some(&prev), &[issue("a", "GOA-1", "t1")]);
        assert!(assigned.is_empty());
        assert!(!next.seen.contains_key("b"));
    }

    #[test]
    fn reassignment_after_disappearing_fires_again() {
        let prev = diff(None, &[issue("a", "GOA-1", "t1")]).0;
        let gone = diff(Some(&prev), &[]).0;
        let (_, assigned) = diff(Some(&gone), &[issue("a", "GOA-1", "t2")]);
        assert_eq!(assigned.len(), 1);
    }

    #[test]
    fn state_round_trips_through_json() {
        let state = diff(None, &[issue("a", "GOA-1", "t1")]).0;
        let raw = serde_json::to_string(&state).unwrap();
        let parsed: WatchState = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed, state);
    }
}
