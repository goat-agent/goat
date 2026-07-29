use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::parse::Item;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchState {
    pub seen: BTreeMap<String, String>,
}

pub fn diff(prev: Option<&WatchState>, fetched: &[Item]) -> (WatchState, Vec<Item>) {
    let next = WatchState {
        seen: fetched
            .iter()
            .map(|item| (item.key.clone(), item.updated_at.clone()))
            .collect(),
    };
    let Some(prev) = prev else {
        return (next, Vec::new());
    };
    let fresh = fetched
        .iter()
        .filter(|item| !prev.seen.contains_key(&item.key))
        .cloned()
        .collect();
    (next, fresh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn item(repo: &str, number: u64, updated_at: &str) -> Item {
        Item {
            key: format!("{repo}#{number}"),
            repo: repo.into(),
            number,
            title: format!("#{number} title"),
            updated_at: updated_at.into(),
            is_pr: true,
            raw: Value::Null,
        }
    }

    #[test]
    fn first_poll_baselines_without_events() {
        let (next, fresh) = diff(None, &[item("acme/a", 1, "t1")]);
        assert!(
            fresh.is_empty(),
            "cold start must not brief every open review request",
        );
        assert_eq!(next.seen.get("acme/a#1").map(String::as_str), Some("t1"));
    }

    #[test]
    fn only_new_keys_fire_and_updates_stay_silent() {
        let prev = diff(None, &[item("acme/a", 1, "t1")]).0;
        let (next, fresh) = diff(
            Some(&prev),
            &[item("acme/a", 1, "t2"), item("acme/a", 2, "t1")],
        );
        assert_eq!(fresh.len(), 1, "a push or comment must not re-fire");
        assert_eq!(fresh[0].number, 2);
        assert_eq!(next.seen.get("acme/a#1").map(String::as_str), Some("t2"));
    }

    #[test]
    fn the_same_number_in_two_repos_is_distinct() {
        let prev = diff(None, &[item("acme/a", 1, "t1")]).0;
        let (_, fresh) = diff(Some(&prev), &[item("acme/b", 1, "t1")]);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].repo, "acme/b");
    }

    #[test]
    fn leaving_the_query_and_coming_back_fires_again() {
        let prev = diff(None, &[item("acme/a", 1, "t1")]).0;
        let gone = diff(Some(&prev), &[]).0;
        assert!(gone.seen.is_empty());
        let (_, fresh) = diff(Some(&gone), &[item("acme/a", 1, "t2")]);
        assert_eq!(fresh.len(), 1, "a re-requested review is news again");
    }

    #[test]
    fn state_round_trips_through_json() {
        let state = diff(None, &[item("acme/a", 1, "t1")]).0;
        let raw = serde_json::to_string(&state).unwrap();
        assert_eq!(serde_json::from_str::<WatchState>(&raw).unwrap(), state);
    }
}
