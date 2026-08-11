use goat_auth::CredentialStore;
use goat_config::Config;
use goat_integration::IntegrationBinding;
use goat_integration_mcp::McpIntegration;
use goat_mcp_tools::ResolvedTool;

pub async fn resolve(
    config: &Config,
    credentials: &CredentialStore,
) -> (Vec<ResolvedTool>, Vec<String>) {
    if config.integrations.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let integrations = goat_integration::registry_from_inventory();
    let mut set = tokio::task::JoinSet::new();
    for (name, connection) in &config.integrations {
        let Some(integration) = integrations.get(name) else {
            continue;
        };
        let Some(mcp) = integration.as_any().downcast_ref::<McpIntegration>() else {
            continue;
        };
        let service = mcp.service().clone();
        let binding = IntegrationBinding::from_config(connection.clone());
        let credentials = credentials.clone();
        let name = name.clone();
        set.spawn(async move {
            let found = goat_integration_mcp::code_tools(&service, &credentials, &binding).await;
            (name, found)
        });
    }
    let mut tools = Vec::new();
    let mut failures = Vec::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok((_, Ok(found))) => tools.extend(found),
            Ok((name, Err(err))) => failures.push(format!("{name}: {err}")),
            Err(err) => failures.push(format!("tool discovery task failed: {err}")),
        }
    }
    (tools, failures)
}
