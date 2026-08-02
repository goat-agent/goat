use goat_channel::{ChannelError, ChannelResult};
use goat_types::Surface;

const CHANNEL_PREFIX: &str = "c:";
const THREAD_MARKER: &str = ":t:";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Coords {
    pub(crate) channel: String,
    pub(crate) thread_ts: Option<String>,
}

pub(crate) fn external(channel: &str, thread_ts: Option<&str>) -> String {
    match thread_ts {
        Some(ts) => format!("{CHANNEL_PREFIX}{channel}{THREAD_MARKER}{ts}"),
        None => format!("{CHANNEL_PREFIX}{channel}"),
    }
}

pub(crate) fn parse(external: &str) -> ChannelResult<Coords> {
    let body = external.strip_prefix(CHANNEL_PREFIX).ok_or_else(|| {
        ChannelError::BadRequest(format!("slack: unrecognised thread key `{external}`"))
    })?;
    let (channel, thread_ts) = match body.split_once(THREAD_MARKER) {
        Some((channel, ts)) => (channel, Some(ts)),
        None => (body, None),
    };
    if channel.is_empty() || thread_ts.is_some_and(str::is_empty) {
        return Err(ChannelError::BadRequest(format!(
            "slack: incomplete thread key `{external}`"
        )));
    }
    Ok(Coords {
        channel: channel.to_string(),
        thread_ts: thread_ts.map(str::to_string),
    })
}

pub(crate) fn is_thread_reply(thread_ts: Option<&str>, ts: &str) -> bool {
    thread_ts.is_some_and(|parent| parent != ts)
}

pub(crate) fn surface_of(channel_type: Option<&str>, channel: &str, in_thread: bool) -> Surface {
    if in_thread {
        return Surface::Thread;
    }
    if is_direct(channel_type, channel) {
        Surface::Dm
    } else {
        Surface::Channel
    }
}

fn is_direct(channel_type: Option<&str>, channel: &str) -> bool {
    match channel_type {
        Some("im") => true,
        Some(_) => false,
        None => channel.starts_with('D'),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_channel_key_round_trips() {
        let key = external("C123", None);
        assert_eq!(key, "c:C123");
        assert_eq!(
            parse(&key).unwrap(),
            Coords {
                channel: "C123".into(),
                thread_ts: None
            }
        );
    }

    #[test]
    fn a_thread_key_round_trips() {
        let key = external("C123", Some("1712345678.000100"));
        assert_eq!(key, "c:C123:t:1712345678.000100");
        assert_eq!(
            parse(&key).unwrap(),
            Coords {
                channel: "C123".into(),
                thread_ts: Some("1712345678.000100".into())
            }
        );
    }

    #[test]
    fn a_dm_key_round_trips_through_the_same_shape() {
        let key = external("D999", None);
        assert_eq!(key, "c:D999");
        assert_eq!(parse(&key).unwrap().channel, "D999");
    }

    #[test]
    fn parse_rejects_foreign_and_incomplete_keys() {
        assert!(parse("g:1:c:2").is_err());
        assert!(parse("").is_err());
        assert!(parse("c:").is_err());
        assert!(parse("c:C123:t:").is_err());
    }

    #[test]
    fn a_thread_parent_is_not_itself_a_thread_reply() {
        assert!(!is_thread_reply(None, "1.1"));
        assert!(!is_thread_reply(Some("1.1"), "1.1"));
        assert!(is_thread_reply(Some("1.1"), "2.2"));
    }

    #[test]
    fn channel_type_decides_the_surface_when_present() {
        assert_eq!(surface_of(Some("im"), "D1", false), Surface::Dm);
        assert_eq!(surface_of(Some("channel"), "C1", false), Surface::Channel);
        assert_eq!(surface_of(Some("group"), "G1", false), Surface::Channel);
        assert_eq!(surface_of(Some("mpim"), "G1", false), Surface::Channel);
    }

    #[test]
    fn the_id_prefix_is_the_fallback_when_channel_type_is_missing() {
        assert_eq!(surface_of(None, "D1", false), Surface::Dm);
        assert_eq!(surface_of(None, "C1", false), Surface::Channel);
        assert_eq!(surface_of(None, "G1", false), Surface::Channel);
    }

    #[test]
    fn a_thread_reply_is_a_thread_even_in_a_dm() {
        assert_eq!(surface_of(Some("im"), "D1", true), Surface::Thread);
        assert_eq!(surface_of(Some("channel"), "C1", true), Surface::Thread);
    }
}
