use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Cursor {
    pub epoch: String,
    pub seq: u64,
}

impl Cursor {
    pub fn new(epoch: impl Into<String>, seq: u64) -> Self {
        Self {
            epoch: epoch.into(),
            seq,
        }
    }

    pub fn same_epoch(&self, other: &Self) -> bool {
        self.epoch == other.epoch
    }

    #[must_use]
    pub fn next(&self) -> Self {
        Self {
            epoch: self.epoch.clone(),
            seq: self.seq.saturating_add(1),
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CursorError {
    #[error("a cursor is `<epoch>:<seq>`, got `{0}`")]
    Malformed(String),
}

impl FromStr for Cursor {
    type Err = CursorError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let Some((epoch, seq)) = text.rsplit_once(':') else {
            return Err(CursorError::Malformed(text.to_owned()));
        };
        if epoch.is_empty() {
            return Err(CursorError::Malformed(text.to_owned()));
        }
        let seq: u64 = seq
            .parse()
            .map_err(|_| CursorError::Malformed(text.to_owned()))?;
        Ok(Self {
            epoch: epoch.to_owned(),
            seq,
        })
    }
}

impl fmt::Display for Cursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.epoch, self.seq)
    }
}

impl Serialize for Cursor {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Cursor {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

impl schemars::JsonSchema for Cursor {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Cursor".into()
    }
    fn schema_id() -> std::borrow::Cow<'static, str> {
        concat!(module_path!(), "::Cursor").into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <String as schemars::JsonSchema>::json_schema(generator)
    }
}

#[cfg(test)]
mod tests {
    use super::{Cursor, CursorError};

    #[test]
    fn round_trips_through_text() {
        let cursor = Cursor::new("e7", 1042);
        assert_eq!(cursor.to_string(), "e7:1042");
        assert_eq!("e7:1042".parse::<Cursor>().unwrap(), cursor);
    }

    #[test]
    fn round_trips_through_json_as_a_string() {
        let cursor = Cursor::new("e7", 1042);
        let json = serde_json::to_string(&cursor).unwrap();
        assert_eq!(json, r#""e7:1042""#);
        assert_eq!(serde_json::from_str::<Cursor>(&json).unwrap(), cursor);
    }

    #[test]
    fn rejects_malformed_text() {
        for bad in ["", "e7", ":4", "e7:x", "e7:"] {
            assert!(matches!(
                bad.parse::<Cursor>(),
                Err(CursorError::Malformed(_))
            ));
        }
    }

    #[test]
    fn a_daemon_restart_is_detectable_by_epoch() {
        let before = Cursor::new("e7", 10);
        let after = Cursor::new("e8", 0);
        assert!(!before.same_epoch(&after));
        assert!(before.same_epoch(&Cursor::new("e7", 99)));
    }

    #[test]
    fn next_advances_within_the_epoch() {
        assert_eq!(Cursor::new("e7", 10).next(), Cursor::new("e7", 11));
        assert_eq!(Cursor::new("e7", u64::MAX).next().seq, u64::MAX);
    }

    #[test]
    fn ordering_is_by_epoch_then_sequence() {
        let mut all = vec![
            Cursor::new("e8", 1),
            Cursor::new("e7", 20),
            Cursor::new("e7", 3),
        ];
        all.sort();
        assert_eq!(
            all,
            vec![
                Cursor::new("e7", 3),
                Cursor::new("e7", 20),
                Cursor::new("e8", 1),
            ]
        );
    }
}
