use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::watch::Observed;

pub const RETENTION: usize = 500;
pub const STATE_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchState {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub seen: BTreeMap<String, String>,
    #[serde(default)]
    pub pending: BTreeMap<String, String>,
}

impl WatchState {
    fn sealed(seen: BTreeMap<String, String>, pending: BTreeMap<String, String>) -> Self {
        Self {
            version: STATE_VERSION,
            seen,
            pending,
        }
    }
}

pub type DiffFn = fn(Option<&WatchState>, &[Observed]) -> (WatchState, Vec<Observed>);
pub type HoldBackFn = fn(&mut WatchState, Option<&WatchState>, &Observed);

#[derive(Clone, Copy)]
pub struct DiffOps {
    pub diff: DiffFn,
    pub hold_back: HoldBackFn,
}

pub const REBUILD: DiffOps = DiffOps {
    diff: rebuild,
    hold_back: revert_seen,
};

pub const RETAIN: DiffOps = DiffOps {
    diff: retain,
    hold_back: revert_seen,
};

pub const SETTLE: DiffOps = DiffOps {
    diff: settle,
    hold_back: demote_to_pending,
};

fn rebuild(prev: Option<&WatchState>, fetched: &[Observed]) -> (WatchState, Vec<Observed>) {
    let seen = fetched
        .iter()
        .map(|item| (item.key.clone(), item.stamp.clone()))
        .collect();
    let next = WatchState::sealed(seen, BTreeMap::new());
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

fn retain(prev: Option<&WatchState>, fetched: &[Observed]) -> (WatchState, Vec<Observed>) {
    let mut seen = prev.map(|state| state.seen.clone()).unwrap_or_default();
    let fresh = match prev {
        None => Vec::new(),
        Some(prev) => fetched
            .iter()
            .filter(|item| !prev.seen.contains_key(&item.key))
            .cloned()
            .collect(),
    };
    for item in fetched {
        seen.insert(item.key.clone(), item.stamp.clone());
    }
    prune(&mut seen);
    (WatchState::sealed(seen, BTreeMap::new()), fresh)
}

fn settle(prev: Option<&WatchState>, fetched: &[Observed]) -> (WatchState, Vec<Observed>) {
    let Some(prev) = prev else {
        let mut seen = BTreeMap::new();
        for item in fetched {
            seen.insert(item.key.clone(), item.stamp.clone());
        }
        prune(&mut seen);
        return (WatchState::sealed(seen, BTreeMap::new()), Vec::new());
    };

    let mut seen = prev.seen.clone();
    let mut pending = BTreeMap::new();
    let mut fresh = Vec::new();
    for item in fetched {
        if prev.seen.contains_key(&item.key) {
            seen.insert(item.key.clone(), item.stamp.clone());
        } else if prev.pending.get(&item.key) == Some(&item.stamp) {
            seen.insert(item.key.clone(), item.stamp.clone());
            fresh.push(item.clone());
        } else {
            pending.insert(item.key.clone(), item.stamp.clone());
        }
    }
    prune(&mut seen);
    prune(&mut pending);
    (WatchState::sealed(seen, pending), fresh)
}

fn revert_seen(next: &mut WatchState, prev: Option<&WatchState>, item: &Observed) {
    match prev.and_then(|state| state.seen.get(&item.key)) {
        Some(stamp) => {
            next.seen.insert(item.key.clone(), stamp.clone());
        }
        None => {
            next.seen.remove(&item.key);
        }
    }
}

fn demote_to_pending(next: &mut WatchState, _prev: Option<&WatchState>, item: &Observed) {
    next.seen.remove(&item.key);
    next.pending.insert(item.key.clone(), item.stamp.clone());
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

    fn item(key: &str, stamp: &str) -> Observed {
        Observed {
            key: key.to_owned(),
            stamp: stamp.to_owned(),
            summary: format!("{key} summary"),
            payload: Value::Null,
        }
    }

    fn keys(items: &[Observed]) -> Vec<&str> {
        items.iter().map(|i| i.key.as_str()).collect()
    }

    #[test]
    fn every_policy_stays_silent_on_a_cold_start() {
        let page = [item("a", "1"), item("b", "2")];
        for ops in [REBUILD, RETAIN, SETTLE] {
            let (state, fresh) = (ops.diff)(None, &page);
            assert!(fresh.is_empty());
            assert_eq!(state.seen.len(), 2);
        }
    }

    #[test]
    fn rebuild_forgets_what_scrolled_off_so_it_can_fire_again() {
        let (first, _) = rebuild(None, &[item("a", "1")]);
        let (second, fresh) = rebuild(Some(&first), &[item("b", "2")]);
        assert_eq!(keys(&fresh), ["b"]);
        assert!(!second.seen.contains_key("a"));
        let (_, again) = rebuild(Some(&second), &[item("a", "1")]);
        assert_eq!(keys(&again), ["a"]);
    }

    #[test]
    fn retain_remembers_what_scrolled_off_so_it_cannot_fire_again() {
        let (first, _) = retain(None, &[item("a", "1")]);
        let (second, fresh) = retain(Some(&first), &[item("b", "2")]);
        assert_eq!(keys(&fresh), ["b"]);
        assert!(second.seen.contains_key("a"));
        let (_, again) = retain(Some(&second), &[item("a", "1")]);
        assert!(again.is_empty());
    }

    #[test]
    fn settle_waits_for_a_stable_second_sighting() {
        let (first, _) = settle(None, &[]);
        let (second, fresh) = settle(Some(&first), &[item("a", "1")]);
        assert!(fresh.is_empty());
        assert_eq!(second.pending.get("a").map(String::as_str), Some("1"));

        let (third, fresh) = settle(Some(&second), &[item("a", "1")]);
        assert_eq!(keys(&fresh), ["a"]);
        assert!(third.seen.contains_key("a"));
    }

    #[test]
    fn settle_restarts_the_gate_when_the_item_keeps_changing() {
        let (first, _) = settle(None, &[]);
        let (second, _) = settle(Some(&first), &[item("a", "1")]);
        let (third, fresh) = settle(Some(&second), &[item("a", "2")]);
        assert!(fresh.is_empty());
        assert_eq!(third.pending.get("a").map(String::as_str), Some("2"));
    }

    #[test]
    fn holding_back_lets_the_next_poll_retry() {
        let (first, _) = retain(None, &[item("a", "1")]);
        let (mut second, fresh) = retain(Some(&first), &[item("b", "2")]);
        assert_eq!(keys(&fresh), ["b"]);
        (RETAIN.hold_back)(&mut second, Some(&first), &fresh[0]);
        let (_, again) = retain(Some(&second), &[item("b", "2")]);
        assert_eq!(keys(&again), ["b"]);
    }

    #[test]
    fn settle_holds_back_by_demoting_to_pending() {
        let mut state = WatchState::sealed(
            [("a".to_owned(), "1".to_owned())].into_iter().collect(),
            BTreeMap::new(),
        );
        (SETTLE.hold_back)(&mut state, None, &item("a", "1"));
        assert!(!state.seen.contains_key("a"));
        assert_eq!(state.pending.get("a").map(String::as_str), Some("1"));
    }

    #[test]
    fn pruning_keeps_the_newest_entries_at_the_retention_bound() {
        let page: Vec<Observed> = (0..RETENTION + 10)
            .map(|n| item(&format!("k{n:04}"), &format!("2026-01-{n:04}")))
            .collect();
        let (state, _) = retain(None, &page);
        assert_eq!(state.seen.len(), RETENTION);
        assert!(state.seen.contains_key(&format!("k{:04}", RETENTION + 9)));
        assert!(!state.seen.contains_key("k0000"));
    }

    #[test]
    fn state_written_before_this_scaffold_still_loads() {
        let legacy: WatchState = serde_json::from_str(r#"{"seen":{"a":"1"}}"#).unwrap();
        assert_eq!(legacy.version, 0);
        assert_eq!(legacy.seen.get("a").map(String::as_str), Some("1"));
        assert!(legacy.pending.is_empty());
    }

    #[test]
    fn fresh_state_carries_the_current_version() {
        let (state, _) = rebuild(None, &[item("a", "1")]);
        assert_eq!(state.version, STATE_VERSION);
    }
}
