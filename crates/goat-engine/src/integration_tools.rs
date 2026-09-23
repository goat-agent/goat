use std::collections::BTreeMap;
use std::path::Path;

use goat_auth::CredentialStore;
use goat_config::Config;
use goat_integration::{Connection, Connections};
use goat_integration_mcp::McpIntegration;
use goat_mcp_tools::ResolvedTool;
use serde_json::Value;

pub async fn resolve(
    config: &Config,
    credentials: &CredentialStore,
    project_root: &Path,
) -> (Vec<ResolvedTool>, Vec<String>) {
    let connections = Connections::parse(&config.integrations);
    let mut failures: Vec<String> = connections
        .invalid
        .iter()
        .map(|(name, reason)| format!("{name}: {reason}"))
        .collect();
    let usage = match goat_integration::load_project_usage(project_root) {
        Ok(usage) => usage,
        Err(e) => {
            failures.push(e.to_string());
            Some(BTreeMap::new())
        }
    };
    let selected = select(&connections, usage.as_ref(), &mut failures);
    let shared_kinds = shared_kinds(&selected);

    let mut set = tokio::task::JoinSet::new();
    for (connection, usage) in selected {
        let Some(factory) = goat_integration::factory_for(&connection.kind) else {
            continue;
        };
        let integration = (factory.ctor)();
        let metadata = integration.metadata();
        if !goat_integration::connection_state(&metadata, connection, credentials).is_ready() {
            continue;
        }
        let Some(mcp) = integration.as_any().downcast_ref::<McpIntegration>() else {
            continue;
        };
        let service = mcp.service().clone();
        let binding = connection.binding(&usage);
        let label = shared_kinds
            .contains(&connection.kind.as_str())
            .then(|| format!("{} · {}", metadata.display, connection.name));
        let credentials = credentials.clone();
        let name = connection.name.clone();
        set.spawn(async move {
            let found = goat_integration_mcp::code_tools(&service, &credentials, &binding).await;
            (name, label, found)
        });
    }
    let mut tools = Vec::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok((_, label, Ok(found))) => tools.extend(found.into_iter().map(|mut tool| {
                if let Some(label) = &label {
                    tool.description = format!("[{label}] {}", tool.description);
                }
                tool
            })),
            Ok((name, _, Err(err))) => failures.push(format!("{name}: {err}")),
            Err(err) => failures.push(format!("tool discovery task failed: {err}")),
        }
    }
    (tools, failures)
}

fn select<'a>(
    connections: &'a Connections,
    usage: Option<&BTreeMap<String, Value>>,
    failures: &mut Vec<String>,
) -> Vec<(&'a Connection, Value)> {
    let Some(usage) = usage else {
        return connections
            .valid
            .iter()
            .map(|connection| (connection, Value::Object(serde_json::Map::new())))
            .collect();
    };
    usage
        .iter()
        .filter_map(|(name, usage)| {
            let Some(connection) = connections.get(name) else {
                failures.push(format!("{name}: no such integration connection"));
                return None;
            };
            if let Err(e) = goat_integration::reject_connection_keys(usage) {
                failures.push(format!("{name}: {e}"));
                return None;
            }
            Some((connection, usage.clone()))
        })
        .collect()
}

fn shared_kinds<'a>(selected: &[(&'a Connection, Value)]) -> Vec<&'a str> {
    let mut kinds: Vec<&str> = selected
        .iter()
        .map(|(connection, _)| connection.kind.as_str())
        .collect();
    kinds.sort_unstable();
    kinds
        .windows(2)
        .filter(|pair| pair[0] == pair[1])
        .map(|pair| pair[0])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn connections() -> Connections {
        Connections {
            valid: vec![
                Connection::new("linear", "linear", json!({})),
                Connection::new("linear-work", "linear", json!({})),
                Connection::new("sentry", "sentry", json!({})),
            ],
            invalid: Vec::new(),
        }
    }

    #[test]
    fn without_a_project_file_every_connection_is_selected() {
        let connections = connections();
        let mut failures = Vec::new();
        let selected = select(&connections, None, &mut failures);
        assert_eq!(selected.len(), 3);
        assert_eq!(shared_kinds(&selected), vec!["linear"]);
    }

    #[test]
    fn a_project_file_selects_only_its_connections_and_never_their_host() {
        let connections = connections();
        let usage = BTreeMap::from([
            ("linear-work".to_owned(), json!({})),
            ("sentry".to_owned(), json!({ "host": "https://evil.test" })),
            ("gone".to_owned(), json!({})),
        ]);
        let mut failures = Vec::new();
        let selected = select(&connections, Some(&usage), &mut failures);
        let names: Vec<&str> = selected.iter().map(|(c, _)| c.name.as_str()).collect();
        assert_eq!(names, vec!["linear-work"]);
        assert_eq!(failures.len(), 2);
        assert!(shared_kinds(&selected).is_empty());
    }
}
