use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::parse::Mention;

pub const RETENTION: usize = 500;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchState {
    pub seen: BTreeMap<String, String>,
}

pub fn diff(prev: Option<&WatchState>, fetched: &[Mention]) -> (WatchState, Vec<Mention>) {
    let mut seen = prev.map(|state| state.seen.clone()).unwrap_or_default();
    let fresh = match prev {
        None => Vec::new(),
        Some(prev) => fetched
            .iter()
            .filter(|mention| !prev.seen.contains_key(&mention.key))
            .cloned()
            .collect(),
    };
    for mention in fetched {
        seen.insert(mention.key.clone(), mention.ts.clone());
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
        .map(|(key, ts)| (key.clone(), ts.clone()))
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));
    ranked.truncate(RETENTION);
    *seen = ranked.into_iter().collect();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn mention(channel: &str, ts: &str) -> Mention {
        Mention {
            key: format!("{channel}:{ts}"),
            channel: channel.into(),
            channel_name: "eng".into(),
            ts: ts.into(),
            user: "U1".into(),
            text: "ping".into(),
            raw: Value::Null,
        }
    }

    #[test]
    fn first_poll_baselines_without_events() {
        let fetched = [mention("C1", "1.1")];
        let (next, fresh) = diff(None, &fetched);
        assert!(
            fresh.is_empty(),
            "cold start must not brief months of history",
        );
        assert_eq!(next.seen.get("C1:1.1").map(String::as_str), Some("1.1"));
    }

    #[test]
    fn only_unseen_keys_fire() {
        let prev = diff(None, &[mention("C1", "1.1")]).0;
        let (_, fresh) = diff(Some(&prev), &[mention("C1", "1.1"), mention("C1", "2.2")]);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].ts, "2.2");
    }

    #[test]
    fn keys_that_scroll_off_the_page_stay_remembered() {
        let prev = diff(None, &[mention("C1", "1.1")]).0;
        let (next, _) = diff(Some(&prev), &[mention("C1", "2.2")]);
        assert!(
            next.seen.contains_key("C1:1.1"),
            "a mention leaving the search page must not be able to re-fire",
        );
        let (_, fresh) = diff(Some(&next), &[mention("C1", "1.1")]);
        assert!(fresh.is_empty());
    }

    #[test]
    fn same_ts_in_two_channels_are_distinct() {
        let prev = diff(None, &[mention("C1", "1.1")]).0;
        let (_, fresh) = diff(Some(&prev), &[mention("C2", "1.1")]);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].channel, "C2");
    }

    #[test]
    fn retention_caps_growth_and_keeps_the_newest() {
        let mut state = WatchState::default();
        for n in 0..(RETENTION + 50) {
            let ts = format!("{n:016}");
            state.seen.insert(format!("C1:{ts}"), ts);
        }
        prune(&mut state.seen);
        assert_eq!(state.seen.len(), RETENTION);
        let newest = RETENTION + 49;
        assert!(state.seen.contains_key(&format!("C1:{newest:016}")));
        assert!(!state.seen.contains_key(&format!("C1:{:016}", 0_usize)));
    }

    #[test]
    fn state_round_trips_through_json() {
        let state = diff(None, &[mention("C1", "1.1")]).0;
        let raw = serde_json::to_string(&state).unwrap();
        let parsed: WatchState = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed, state);
    }
}
