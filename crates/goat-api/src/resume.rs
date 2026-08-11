use crate::cursor::Cursor;
use crate::methods::WatchFrom;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchStart {
    Snapshot { reset: bool },
    Replay { from_seq: u64 },
}

impl WatchStart {
    pub fn is_snapshot(self) -> bool {
        matches!(self, Self::Snapshot { .. })
    }

    pub fn resets(self) -> bool {
        matches!(self, Self::Snapshot { reset: true })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retained {
    pub oldest: Option<u64>,
    pub next_seq: u64,
}

impl Retained {
    pub fn new(oldest: Option<u64>, next_seq: u64) -> Self {
        Self { oldest, next_seq }
    }

    pub fn empty(next_seq: u64) -> Self {
        Self {
            oldest: None,
            next_seq,
        }
    }
}

pub fn decide(epoch: &str, retained: Retained, from: &WatchFrom) -> WatchStart {
    let WatchFrom::Cursor { cursor } = from else {
        return WatchStart::Snapshot { reset: false };
    };
    if cursor.epoch != epoch {
        return WatchStart::Snapshot { reset: true };
    }
    if cursor.seq > retained.next_seq {
        return WatchStart::Snapshot { reset: true };
    }
    if cursor.seq == retained.next_seq {
        return WatchStart::Replay {
            from_seq: retained.next_seq,
        };
    }
    match retained.oldest {
        Some(oldest) if cursor.seq >= oldest => WatchStart::Replay {
            from_seq: cursor.seq,
        },
        _ => WatchStart::Snapshot { reset: true },
    }
}

pub fn cursor_for(epoch: &str, seq: u64) -> Cursor {
    Cursor::new(epoch, seq)
}

#[cfg(test)]
mod tests {
    use super::{Retained, WatchStart, decide};
    use crate::cursor::Cursor;
    use crate::methods::WatchFrom;

    fn from_cursor(epoch: &str, seq: u64) -> WatchFrom {
        WatchFrom::Cursor {
            cursor: Cursor::new(epoch, seq),
        }
    }

    #[test]
    fn a_fresh_watcher_always_gets_a_snapshot_without_a_reset_marker() {
        let start = decide("e7", Retained::new(Some(10), 40), &WatchFrom::Snapshot {});
        assert_eq!(start, WatchStart::Snapshot { reset: false });
        assert!(start.is_snapshot());
        assert!(!start.resets());
    }

    #[test]
    fn a_cursor_inside_the_retained_window_replays_without_a_snapshot() {
        let start = decide("e7", Retained::new(Some(10), 40), &from_cursor("e7", 25));
        assert_eq!(start, WatchStart::Replay { from_seq: 25 });
        assert!(!start.is_snapshot());
    }

    #[test]
    fn a_cursor_exactly_at_the_oldest_retained_event_still_replays() {
        let start = decide("e7", Retained::new(Some(10), 40), &from_cursor("e7", 10));
        assert_eq!(start, WatchStart::Replay { from_seq: 10 });
    }

    #[test]
    fn a_cursor_that_fell_out_of_the_window_falls_back_to_a_reset_snapshot() {
        let start = decide("e7", Retained::new(Some(10), 40), &from_cursor("e7", 9));
        assert_eq!(start, WatchStart::Snapshot { reset: true });
        assert!(start.resets());
    }

    #[test]
    fn a_daemon_restart_is_detected_by_epoch_and_forces_a_reset_snapshot() {
        let start = decide("e8", Retained::new(Some(0), 40), &from_cursor("e7", 25));
        assert_eq!(start, WatchStart::Snapshot { reset: true });
    }

    #[test]
    fn a_caught_up_cursor_replays_nothing_and_skips_the_snapshot() {
        let start = decide("e7", Retained::new(Some(10), 40), &from_cursor("e7", 40));
        assert_eq!(start, WatchStart::Replay { from_seq: 40 });
    }

    #[test]
    fn a_cursor_ahead_of_the_daemon_is_treated_as_untrustworthy() {
        let start = decide("e7", Retained::new(Some(10), 40), &from_cursor("e7", 41));
        assert_eq!(start, WatchStart::Snapshot { reset: true });
    }

    #[test]
    fn an_empty_log_replays_only_a_caught_up_cursor() {
        let caught_up = decide("e7", Retained::empty(40), &from_cursor("e7", 40));
        assert_eq!(caught_up, WatchStart::Replay { from_seq: 40 });

        let behind = decide("e7", Retained::empty(40), &from_cursor("e7", 39));
        assert_eq!(behind, WatchStart::Snapshot { reset: true });
    }

    #[test]
    fn a_brand_new_session_accepts_a_zero_cursor() {
        let start = decide("e7", Retained::empty(0), &from_cursor("e7", 0));
        assert_eq!(start, WatchStart::Replay { from_seq: 0 });
    }

    #[test]
    fn every_reset_path_is_a_snapshot_so_a_client_never_silently_loses_state() {
        let cases = [
            decide("e8", Retained::new(Some(0), 5), &from_cursor("e7", 1)),
            decide("e7", Retained::new(Some(3), 5), &from_cursor("e7", 1)),
            decide("e7", Retained::new(Some(3), 5), &from_cursor("e7", 99)),
            decide("e7", Retained::empty(5), &from_cursor("e7", 1)),
        ];
        for case in cases {
            assert!(case.is_snapshot(), "{case:?} must not silently replay");
            assert!(case.resets(), "{case:?} must tell the client it reset");
        }
    }
}
