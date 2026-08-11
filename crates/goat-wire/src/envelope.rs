use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

pub mod id_serde {
    use serde::de::Visitor;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &u64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&v.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = u64;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("u64 as string or integer")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<u64, E> {
                v.parse().map_err(E::custom)
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<u64, E> {
                Ok(v)
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<u64, E> {
                u64::try_from(v).map_err(E::custom)
            }
        }
        d.deserialize_any(V)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Id(pub u64);

impl Id {
    pub fn is_client_originated(self) -> bool {
        !self.0.is_multiple_of(2)
    }

    pub fn is_daemon_originated(self) -> bool {
        self.0.is_multiple_of(2)
    }

    pub fn origin(self) -> Role {
        if self.is_client_originated() {
            Role::Client
        } else {
            Role::Daemon
        }
    }
}

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Serialize for Id {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        id_serde::serialize(&self.0, s)
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        id_serde::deserialize(d).map(Self)
    }
}

impl schemars::JsonSchema for Id {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "EnvelopeId".into()
    }
    fn schema_id() -> std::borrow::Cow<'static, str> {
        concat!(module_path!(), "::Id").into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <String as schemars::JsonSchema>::json_schema(generator)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdAllocator {
    next: u64,
}

impl IdAllocator {
    pub fn for_role(role: Role) -> Self {
        Self {
            next: match role {
                Role::Client => 1,
                Role::Daemon => 2,
            },
        }
    }

    pub fn allocate(&mut self) -> Id {
        let id = Id(self.next);
        self.next = self.next.wrapping_add(2);
        id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Client,
    Daemon,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StreamClass {
    Reliable,
    Lossy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Incompatible,
    UnknownMethod,
    UnsupportedVersion,
    InvalidParams,
    NotFound,
    Conflict,
    Denied,
    Canceled,
    Timeout,
    Internal,
    NoHost,
    HostGone,
    Lagged,
    TooLarge,
    AlreadyAnswered,
    ResyncRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Execution {
    NotStarted,
    KnownFailed,
    OutcomeUnknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CallError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<Execution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl CallError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            execution: None,
            data: None,
        }
    }

    #[must_use]
    pub fn with_execution(mut self, execution: Execution) -> Self {
        self.execution = Some(execution);
        self
    }

    #[must_use]
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    pub fn retry_is_safe(&self) -> bool {
        matches!(
            self.execution,
            Some(Execution::NotStarted | Execution::KnownFailed)
        )
    }
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
    Ok {
        #[serde(default, skip_serializing_if = "Value::is_null")]
        result: Value,
    },
    Error {
        error: CallError,
    },
}

impl Outcome {
    pub fn ok(result: Value) -> Self {
        Self::Ok { result }
    }

    pub fn empty() -> Self {
        Self::Ok {
            result: Value::Null,
        }
    }

    pub fn error(error: CallError) -> Self {
        Self::Error { error }
    }

    pub fn into_result(self) -> Result<Value, CallError> {
        match self {
            Self::Ok { result } => Ok(result),
            Self::Error { error } => Err(error),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Hello {
    pub role: Role,
    pub envelope: String,
    pub agent: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub methods: BTreeMap<String, Vec<u16>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grants: Vec<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub info: Value,
}

impl Hello {
    pub fn new(role: Role, agent: impl Into<String>) -> Self {
        Self {
            role,
            envelope: envelope_fingerprint().to_owned(),
            agent: agent.into(),
            methods: BTreeMap::new(),
            grants: Vec::new(),
            info: Value::Null,
        }
    }

    #[must_use]
    pub fn with_method(mut self, name: impl Into<String>, versions: Vec<u16>) -> Self {
        self.methods.insert(name.into(), versions);
        self
    }

    #[must_use]
    pub fn with_grants(mut self, grants: Vec<String>) -> Self {
        self.grants = grants;
        self
    }

    #[must_use]
    pub fn with_info(mut self, info: Value) -> Self {
        self.info = info;
        self
    }

    pub fn speaks(&self, method: &str, version: u16) -> bool {
        self.methods
            .get(method)
            .is_some_and(|versions| versions.contains(&version))
    }

    pub fn best_version(&self, method: &str) -> Option<u16> {
        self.methods
            .get(method)
            .and_then(|v| v.iter().copied().max())
    }

    pub fn compatible(&self) -> bool {
        self.envelope == envelope_fingerprint()
    }
}

fn default_version() -> u16 {
    1
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Frame {
    Hello(Hello),
    Req {
        id: Id,
        method: String,
        #[serde(default = "default_version")]
        version: u16,
        #[serde(default, skip_serializing_if = "Value::is_null")]
        params: Value,
    },
    Res {
        id: Id,
        #[serde(flatten)]
        outcome: Outcome,
    },
    Data {
        id: Id,
        item: Value,
        #[serde(default, skip_serializing_if = "is_zero")]
        dropped: u64,
    },
    End {
        id: Id,
        #[serde(flatten)]
        outcome: Outcome,
    },
    Cancel {
        id: Id,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

impl Frame {
    pub fn req(id: Id, method: impl Into<String>, version: u16, params: Value) -> Self {
        Self::Req {
            id,
            method: method.into(),
            version,
            params,
        }
    }

    pub fn res(id: Id, outcome: Outcome) -> Self {
        Self::Res { id, outcome }
    }

    pub fn data(id: Id, item: Value) -> Self {
        Self::Data {
            id,
            item,
            dropped: 0,
        }
    }

    pub fn data_after_drop(id: Id, item: Value, dropped: u64) -> Self {
        Self::Data { id, item, dropped }
    }

    pub fn end(id: Id, outcome: Outcome) -> Self {
        Self::End { id, outcome }
    }

    pub fn cancel(id: Id, reason: Option<String>) -> Self {
        Self::Cancel { id, reason }
    }

    pub fn id(&self) -> Option<Id> {
        match self {
            Self::Hello(_) => None,
            Self::Req { id, .. }
            | Self::Res { id, .. }
            | Self::Data { id, .. }
            | Self::End { id, .. }
            | Self::Cancel { id, .. } => Some(*id),
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Res { .. } | Self::End { .. })
    }
}

pub fn envelope_fingerprint() -> &'static str {
    include_str!("envelope_fingerprint.txt").trim_ascii_end()
}

#[cfg(test)]
mod fingerprint {
    use std::fmt::Write as _;

    use sha2::{Digest, Sha256};

    use super::{Frame, envelope_fingerprint};

    fn render() -> String {
        let schema = serde_json::to_value(schemars::schema_for!(Frame)).unwrap();
        let mut out = serde_json::to_string_pretty(&schema).unwrap();
        out.push('\n');
        out
    }

    fn digest() -> String {
        let hash = Sha256::digest(render().as_bytes());
        let mut out = String::with_capacity(16);
        for byte in hash.iter().take(8) {
            let _ = write!(out, "{byte:02x}");
        }
        format!("env1:{out}")
    }

    #[test]
    fn matches_fixture() {
        assert_eq!(digest(), envelope_fingerprint());
    }

    #[test]
    fn does_not_depend_on_engine_vocabulary() {
        let rendered = render();
        for engine_type in [
            "TranscriptEntry",
            "ModelEntry",
            "ToolCall",
            "RateLimitSnapshot",
        ] {
            assert!(
                !rendered.contains(engine_type),
                "envelope schema leaked {engine_type}; adding an engine variant would move the envelope fingerprint"
            );
        }
    }

    #[test]
    #[ignore = "rewrites envelope_fingerprint.txt; run after a deliberate envelope change"]
    fn regenerate() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/envelope_fingerprint.txt");
        std::fs::write(path, format!("{}\n", digest())).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CallError, ErrorCode, Execution, Frame, Hello, Id, IdAllocator, Outcome, Role,
        envelope_fingerprint,
    };
    use serde_json::{Value, json};

    fn round_trip(frame: &Frame) -> Frame {
        let text = serde_json::to_string(frame).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn req_serializes_flat_with_kind() {
        let frame = Frame::req(Id(7), "fs.list", 1, json!({"path": "/w"}));
        let text = serde_json::to_string(&frame).unwrap();
        assert_eq!(
            text,
            r#"{"kind":"req","id":"7","method":"fs.list","version":1,"params":{"path":"/w"}}"#
        );
        assert_eq!(round_trip(&frame), frame);
    }

    #[test]
    fn res_ok_flattens_status_and_result() {
        let frame = Frame::res(Id(7), Outcome::ok(json!({"entries": []})));
        let text = serde_json::to_string(&frame).unwrap();
        assert_eq!(
            text,
            r#"{"kind":"res","id":"7","status":"ok","result":{"entries":[]}}"#
        );
        assert_eq!(round_trip(&frame), frame);
    }

    #[test]
    fn res_empty_ok_omits_result() {
        let frame = Frame::res(Id(3), Outcome::empty());
        let text = serde_json::to_string(&frame).unwrap();
        assert_eq!(text, r#"{"kind":"res","id":"3","status":"ok"}"#);
        assert_eq!(round_trip(&frame), frame);
    }

    #[test]
    fn res_error_carries_code_and_execution() {
        let frame = Frame::res(
            Id(9),
            Outcome::error(
                CallError::new(ErrorCode::NoHost, "no browser host is connected")
                    .with_execution(Execution::NotStarted)
                    .with_data(json!({"capability": "host.browser"})),
            ),
        );
        let back = round_trip(&frame);
        assert_eq!(back, frame);
        let Frame::Res { outcome, .. } = back else {
            panic!("expected res")
        };
        let err = outcome.into_result().unwrap_err();
        assert_eq!(err.code, ErrorCode::NoHost);
        assert!(err.retry_is_safe());
    }

    #[test]
    fn outcome_unknown_is_never_retry_safe() {
        let err = CallError::new(ErrorCode::HostGone, "client vanished")
            .with_execution(Execution::OutcomeUnknown);
        assert!(!err.retry_is_safe());
    }

    #[test]
    fn missing_execution_is_not_retry_safe() {
        let err = CallError::new(ErrorCode::Internal, "boom");
        assert!(!err.retry_is_safe());
    }

    #[test]
    fn data_omits_dropped_when_zero_and_keeps_it_otherwise() {
        let clean = Frame::data(Id(5), json!({"t": "output"}));
        assert_eq!(
            serde_json::to_string(&clean).unwrap(),
            r#"{"kind":"data","id":"5","item":{"t":"output"}}"#
        );
        let lossy = Frame::data_after_drop(Id(5), json!({"t": "output"}), 128);
        assert!(
            serde_json::to_string(&lossy)
                .unwrap()
                .contains(r#""dropped":128"#)
        );
        assert_eq!(round_trip(&lossy), lossy);
    }

    #[test]
    fn end_and_cancel_round_trip() {
        let end = Frame::end(
            Id(5),
            Outcome::error(CallError::new(ErrorCode::Canceled, "closed by client")),
        );
        assert_eq!(round_trip(&end), end);
        let cancel = Frame::cancel(Id(5), Some("pane closed".to_owned()));
        assert_eq!(round_trip(&cancel), cancel);
        let bare = Frame::cancel(Id(5), None);
        assert_eq!(
            serde_json::to_string(&bare).unwrap(),
            r#"{"kind":"cancel","id":"5"}"#
        );
    }

    #[test]
    fn hello_round_trips_and_reports_compatibility() {
        let hello = Hello::new(Role::Client, "goat-tui/0.1.0")
            .with_method("host.browser", vec![1])
            .with_info(json!({"pid": 41221}));
        let frame = Frame::Hello(hello.clone());
        assert_eq!(round_trip(&frame), frame);
        assert!(hello.compatible());
        assert!(hello.speaks("host.browser", 1));
        assert!(!hello.speaks("host.browser", 2));
        assert_eq!(hello.best_version("host.browser"), Some(1));
        assert_eq!(hello.best_version("host.simulator"), None);
    }

    #[test]
    fn hello_mismatch_is_detected() {
        let mut hello = Hello::new(Role::Daemon, "goat-daemon/0.1.0");
        hello.envelope = "env0:deadbeef".to_owned();
        assert!(!hello.compatible());
    }

    #[test]
    fn hello_tolerates_unknown_fields_and_absent_optionals() {
        let hello: Hello = serde_json::from_str(
            r#"{"role":"daemon","envelope":"x","agent":"a","future_field":{"nested":true}}"#,
        )
        .unwrap();
        assert_eq!(hello.role, Role::Daemon);
        assert!(hello.methods.is_empty());
        assert!(hello.grants.is_empty());
        assert_eq!(hello.info, Value::Null);
    }

    #[test]
    fn req_version_defaults_to_one_when_absent() {
        let frame: Frame =
            serde_json::from_str(r#"{"kind":"req","id":"1","method":"session.list"}"#).unwrap();
        assert_eq!(frame, Frame::req(Id(1), "session.list", 1, Value::Null));
    }

    #[test]
    fn ids_accept_string_or_number_and_survive_js_unsafe_range() {
        let big = Id(9_007_199_254_740_993);
        assert_eq!(
            serde_json::to_string(&big).unwrap(),
            r#""9007199254740993""#
        );
        let from_str: Id = serde_json::from_str(r#""42""#).unwrap();
        let from_num: Id = serde_json::from_str("42").unwrap();
        assert_eq!(from_str, Id(42));
        assert_eq!(from_num, Id(42));
    }

    #[test]
    fn allocator_parity_separates_the_two_directions() {
        let mut client = IdAllocator::for_role(Role::Client);
        let mut daemon = IdAllocator::for_role(Role::Daemon);
        let cs: Vec<Id> = (0..3).map(|_| client.allocate()).collect();
        let ds: Vec<Id> = (0..3).map(|_| daemon.allocate()).collect();
        assert_eq!(cs, vec![Id(1), Id(3), Id(5)]);
        assert_eq!(ds, vec![Id(2), Id(4), Id(6)]);
        assert!(cs.iter().all(|id| id.is_client_originated()));
        assert!(ds.iter().all(|id| id.is_daemon_originated()));
        assert_eq!(cs[0].origin(), Role::Client);
        assert_eq!(ds[0].origin(), Role::Daemon);
    }

    #[test]
    fn frame_id_and_terminality() {
        assert_eq!(Frame::req(Id(1), "m", 1, Value::Null).id(), Some(Id(1)));
        assert_eq!(Frame::Hello(Hello::new(Role::Client, "a")).id(), None);
        assert!(Frame::res(Id(1), Outcome::empty()).is_terminal());
        assert!(Frame::end(Id(1), Outcome::empty()).is_terminal());
        assert!(!Frame::data(Id(1), Value::Null).is_terminal());
        assert!(!Frame::cancel(Id(1), None).is_terminal());
    }

    #[test]
    fn envelope_fingerprint_is_stable_text() {
        let fingerprint = envelope_fingerprint();
        assert!(!fingerprint.is_empty());
        assert!(!fingerprint.ends_with('\n'));
    }
}
