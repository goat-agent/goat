use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::parse::Issue;

pub const RETENTION: usize = 500;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchState {
    pub seen: BTreeMap<String, String>,
}

pub fn diff(prev: Option<&WatchState>, fetched: &[Issue]) -> (WatchState, Vec<Issue>) {
    let mut seen = prev.map(|state| state.seen.clone()).unwrap_or_default();
    let fresh = match prev {
        None => Vec::new(),
        Some(prev) => fetched
            .iter()
            .filter(|issue| !prev.seen.contains_key(&issue.key))
            .cloned()
            .collect(),
    };
    for issue in fetched {
        seen.insert(issue.key.clone(), issue.last_seen.clone());
    }
    prune(&mut seen);
    (WatchState { seen }, fresh)
}

fn prune(seen: &mut BTreeMap<String, String>) {
    if seen.len() <= RETENTION {
        return;
    }
    let mut ranked: Vec<(String, String)> = seen
        .iter()
        .map(|(key, last_seen)| (key.clone(), last_seen.clone()))
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));
    ranked.truncate(RETENTION);
    *seen = ranked.into_iter().collect();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn issue(key: &str, last_seen: &str) -> Issue {
        Issue {
            key: key.into(),
            short_id: key.into(),
            title: "boom".into(),
            culprit: String::new(),
            count: "1".into(),
            user_count: "1".into(),
            last_seen: last_seen.into(),
            raw: Value::Null,
        }
    }

    #[test]
    fn first_poll_baselines_without_events() {
        let fetched = [issue("BACKEND-1A", "2026-07-28T00:00:00Z")];
        let (next, fresh) = diff(None, &fetched);
        assert!(
            fresh.is_empty(),
            "cold start must not brief the whole backlog",
        );
        assert_eq!(
            next.seen.get("BACKEND-1A").map(String::as_str),
            Some("2026-07-28T00:00:00Z")
        );
    }

    #[test]
    fn only_unseen_issues_fire() {
        let prev = diff(None, &[issue("BACKEND-1A", "2026-07-28T00:00:00Z")]).0;
        let (_, fresh) = diff(
            Some(&prev),
            &[
                issue("BACKEND-1A", "2026-07-28T01:00:00Z"),
                issue("BACKEND-2B", "2026-07-28T01:00:00Z"),
            ],
        );
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].key, "BACKEND-2B");
    }

    #[test]
    fn issues_that_scroll_off_the_page_stay_remembered() {
        let prev = diff(None, &[issue("BACKEND-1A", "2026-07-28T00:00:00Z")]).0;
        let (next, _) = diff(Some(&prev), &[issue("BACKEND-2B", "2026-07-28T01:00:00Z")]);
        assert!(
            next.seen.contains_key("BACKEND-1A"),
            "an issue leaving the query page must not be able to re-fire",
        );
        let (_, fresh) = diff(Some(&next), &[issue("BACKEND-1A", "2026-07-28T02:00:00Z")]);
        assert!(fresh.is_empty());
    }

    #[test]
    fn retention_caps_growth_and_keeps_the_newest() {
        let mut state = WatchState::default();
        for n in 0..(RETENTION + 50) {
            let last_seen = format!("{n:016}");
            state.seen.insert(format!("ISSUE-{last_seen}"), last_seen);
        }
        prune(&mut state.seen);
        assert_eq!(state.seen.len(), RETENTION);
        let newest = RETENTION + 49;
        assert!(state.seen.contains_key(&format!("ISSUE-{newest:016}")));
        assert!(!state.seen.contains_key(&format!("ISSUE-{:016}", 0_usize)));
    }

    #[test]
    fn state_round_trips_through_json() {
        let state = diff(None, &[issue("BACKEND-1A", "2026-07-28T00:00:00Z")]).0;
        let raw = serde_json::to_string(&state).unwrap();
        let parsed: WatchState = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed, state);
    }
}
