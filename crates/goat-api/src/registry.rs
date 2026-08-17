use goat_wire::envelope::StreamClass;
use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Grant {
    Any,
    Admin,
}

impl Grant {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Admin => "admin",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    Unary,
    Stream(StreamClass),
}

impl Shape {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unary => "unary",
            Self::Stream(StreamClass::Reliable) => "stream:reliable",
            Self::Stream(StreamClass::Lossy) => "stream:lossy",
        }
    }

    pub fn is_stream(self) -> bool {
        matches!(self, Self::Stream(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    ToDaemon,
    ToClient,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ToDaemon => "to_daemon",
            Self::ToClient => "to_client",
        }
    }
}

pub trait Method {
    const NAME: &'static str;
    const VERSION: u16;
    const SHAPE: Shape;
    const GRANT: Grant;
    const DIRECTION: Direction;

    type Params: Serialize + DeserializeOwned + JsonSchema;
    type Output: Serialize + DeserializeOwned + JsonSchema;
    type Item: Serialize + DeserializeOwned + JsonSchema;
}

#[derive(Debug, Clone, PartialEq)]
pub struct MethodSchema {
    pub name: &'static str,
    pub version: u16,
    pub shape: Shape,
    pub grant: Grant,
    pub direction: Direction,
    pub params: serde_json::Value,
    pub output: serde_json::Value,
    pub item: serde_json::Value,
}

impl MethodSchema {
    pub fn qualified(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }

    pub fn contract(&self) -> String {
        serde_json::to_string(&serde_json::json!({
            "name": self.name,
            "version": self.version,
            "shape": self.shape.as_str(),
            "grant": self.grant.as_str(),
            "direction": self.direction.as_str(),
            "params": self.params,
            "output": self.output,
            "item": self.item,
        }))
        .unwrap_or_default()
    }
}

#[must_use]
pub fn schema_document() -> serde_json::Value {
    serde_json::json!({
        "envelope": goat_wire::envelope_fingerprint(),
        "methods": registry()
            .into_iter()
            .map(|schema| serde_json::json!({
                "name": schema.name,
                "version": schema.version,
                "shape": schema.shape.as_str(),
                "grant": schema.grant.as_str(),
                "direction": schema.direction.as_str(),
                "params": schema.params,
                "output": schema.output,
                "item": schema.item,
            }))
            .collect::<Vec<_>>(),
    })
}

pub fn describe<M: Method>() -> MethodSchema {
    MethodSchema {
        name: M::NAME,
        version: M::VERSION,
        shape: M::SHAPE,
        grant: M::GRANT,
        direction: M::DIRECTION,
        params: schema_of::<M::Params>(),
        output: schema_of::<M::Output>(),
        item: schema_of::<M::Item>(),
    }
}

fn schema_of<T: JsonSchema>() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(T)).unwrap_or(serde_json::Value::Null)
}

pub fn registry() -> Vec<MethodSchema> {
    use crate::methods as m;
    let mut all = vec![
        describe::<m::DaemonStatus>(),
        describe::<m::SessionList>(),
        describe::<m::SessionOpen>(),
        describe::<m::SessionSubmit>(),
        describe::<m::SessionControl>(),
        describe::<m::SessionKill>(),
        describe::<m::SessionWatch>(),
        describe::<m::ConversationList>(),
        describe::<m::AskAnswer>(),
        describe::<m::FsList>(),
        describe::<m::FsRead>(),
        describe::<m::FsWrite>(),
        describe::<m::GitDiff>(),
        describe::<m::PtyOpen>(),
        describe::<m::PtyWrite>(),
        describe::<m::PtyResize>(),
        describe::<m::CapabilityAdvertise>(),
        describe::<m::CapabilityList>(),
        describe::<m::CapabilityBind>(),
        describe::<m::AgentWatch>(),
        describe::<m::AdminAgentReload>(),
        describe::<m::AdminConfigEdit>(),
        describe::<m::AdminDaemonStop>(),
        describe::<m::AdminDevicePair>(),
        describe::<m::AdminDeviceList>(),
        describe::<m::AdminDeviceRevoke>(),
        describe::<m::HostBrowser>(),
        describe::<m::HostNotify>(),
        describe::<m::BrowserEvent>(),
    ];
    all.sort_by(|a, b| (a.name, a.version).cmp(&(b.name, b.version)));
    all
}

#[cfg(test)]
mod tests {
    use super::{Direction, Grant, Shape, registry};
    use goat_wire::envelope::StreamClass;
    use std::collections::HashSet;

    #[test]
    fn every_method_is_uniquely_named_and_versioned() {
        let mut seen = HashSet::new();
        for schema in registry() {
            assert!(
                seen.insert(schema.qualified()),
                "duplicate method {}",
                schema.qualified()
            );
        }
    }

    #[test]
    fn method_names_are_namespaced_lowercase() {
        for schema in registry() {
            assert!(
                schema.name.contains('.'),
                "{} is missing a namespace",
                schema.name
            );
            assert_eq!(schema.name, schema.name.to_lowercase());
            assert!(schema.version >= 1);
        }
    }

    #[test]
    fn admin_namespace_and_admin_grant_agree() {
        for schema in registry() {
            let named_admin = schema.name.starts_with("admin.");
            let granted_admin = schema.grant == Grant::Admin;
            assert_eq!(
                named_admin, granted_admin,
                "{} disagrees about admin authority",
                schema.name
            );
        }
    }

    #[test]
    fn host_namespace_is_exactly_the_reverse_direction() {
        for schema in registry() {
            let named_host = schema.name.starts_with("host.");
            let reverse = schema.direction == Direction::ToClient;
            assert_eq!(
                named_host, reverse,
                "{} disagrees about direction",
                schema.name
            );
        }
    }

    #[test]
    fn reverse_methods_are_never_admin() {
        for schema in registry() {
            if schema.direction == Direction::ToClient {
                assert_eq!(schema.grant, Grant::Any, "{} is reverse admin", schema.name);
            }
        }
    }

    #[test]
    fn only_streams_declare_an_item_type() {
        for schema in registry() {
            let has_item = schema.item != serde_json::Value::Null
                && schema.item.get("type") != Some(&serde_json::json!("null"));
            assert_eq!(
                schema.shape.is_stream(),
                has_item,
                "{} item type does not match its shape",
                schema.name
            );
        }
    }

    #[test]
    fn shape_labels_are_stable() {
        assert_eq!(Shape::Unary.as_str(), "unary");
        assert_eq!(
            Shape::Stream(StreamClass::Reliable).as_str(),
            "stream:reliable"
        );
        assert_eq!(Shape::Stream(StreamClass::Lossy).as_str(), "stream:lossy");
    }

    #[test]
    fn session_state_streams_are_reliable_and_terminal_output_is_lossy() {
        let by_name = |name: &str| {
            registry()
                .into_iter()
                .find(|schema| schema.name == name)
                .unwrap_or_else(|| panic!("{name} is not registered"))
        };
        assert_eq!(
            by_name("session.watch").shape,
            Shape::Stream(StreamClass::Reliable)
        );
        assert_eq!(
            by_name("agent.watch").shape,
            Shape::Stream(StreamClass::Reliable)
        );
        assert_eq!(by_name("pty.open").shape, Shape::Stream(StreamClass::Lossy));
    }
}

#[cfg(test)]
mod frozen {
    use std::fmt::Write as _;

    use sha2::{Digest, Sha256};

    use super::registry;

    fn render() -> String {
        let mut out = String::new();
        for schema in registry() {
            let hash = Sha256::digest(schema.contract().as_bytes());
            let mut short = String::with_capacity(16);
            for byte in hash.iter().take(8) {
                let _ = write!(short, "{byte:02x}");
            }
            let _ = writeln!(
                out,
                "{} {} {} {} {short}",
                schema.qualified(),
                schema.shape.as_str(),
                schema.grant.as_str(),
                schema.direction.as_str()
            );
        }
        out
    }

    fn fixture() -> &'static str {
        include_str!("methods_fingerprint.txt")
    }

    fn schema_fixture() -> &'static str {
        include_str!("methods_schema.json")
    }

    fn rendered_schema() -> String {
        let mut text = serde_json::to_string_pretty(&super::schema_document())
            .expect("the method table serializes");
        text.push('\n');
        text
    }

    #[test]
    fn the_published_schema_matches_the_method_table() {
        assert_eq!(
            rendered_schema(),
            schema_fixture(),
            "methods_schema.json is the contract other clients generate from; regenerate it with \
             `cargo run -p goat-api --bin methods_schema crates/goat-api/src/methods_schema.json`"
        );
    }

    #[test]
    fn method_contracts_are_frozen() {
        assert_eq!(
            render(),
            fixture(),
            "a method contract changed without a version bump; bump the method version or run the regenerate test deliberately"
        );
    }

    #[test]
    #[ignore = "rewrites methods_fingerprint.txt; run only after a deliberate method version bump"]
    fn regenerate() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/methods_fingerprint.txt");
        std::fs::write(path, render()).unwrap();
    }
}
