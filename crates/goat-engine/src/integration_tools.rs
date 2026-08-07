use std::collections::HashSet;
use std::sync::Arc;

use goat_auth::CredentialStore;
use goat_config::Config;
use goat_integration::IntegrationBinding;
use goat_integration_mcp::{CachedTool, DenyRules, McpIntegration, McpService, ToolDisposition};
use goat_mcp::McpToolResult;
use goat_protocol::ToolDisplay;
use goat_tool::{Tool, ToolContext, ToolError, ToolFuture, ToolImage, ToolOutput};
use serde_json::Value;

pub async fn load(
    config: &Config,
    credentials: &CredentialStore,
) -> (Vec<Box<dyn Tool>>, Vec<String>) {
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
        set.spawn(async move {
            let discovered = discover(&service, &credentials, &binding).await;
            (name.clone(), service, discovered)
        });
    }
    let mut adapters = Vec::new();
    let mut failures = Vec::new();
    let mut used = HashSet::new();
    while let Some(joined) = set.join_next().await {
        let (name, service, discovered) = match joined {
            Ok(outcome) => outcome,
            Err(err) => {
                failures.push(format!("tool discovery task failed: {err}"));
                continue;
            }
        };
        let tools = match discovered {
            Ok(tools) => tools,
            Err(err) => {
                failures.push(format!("{name}: {err}"));
                continue;
            }
        };
        adapters.extend(adapt(&service, tools, &mut used));
    }
    adapters.sort_by(|a, b| a.name.cmp(b.name));
    (
        adapters
            .into_iter()
            .map(|tool| Box::new(tool) as Box<dyn Tool>)
            .collect(),
        failures,
    )
}

async fn discover(
    service: &Arc<McpService>,
    credentials: &CredentialStore,
    binding: &IntegrationBinding,
) -> goat_integration::IntegrationResult<Vec<CachedTool>> {
    let session = service.connect(credentials, binding).await?;
    let tools = session.list_tools().await;
    session.close().await;
    Ok(tools
        .map_err(|e| service.wire_error(&e))?
        .into_iter()
        .map(CachedTool::from)
        .collect())
}

fn adapt(
    service: &Arc<McpService>,
    discovered: Vec<CachedTool>,
    used: &mut HashSet<String>,
) -> Vec<IntegrationToolAdapter> {
    let mut adapters = Vec::new();
    for tool in discovered {
        let disposition = goat_integration_mcp::disposition(
            service,
            &service.tools,
            &DenyRules::effective(&service.tools, &Value::Null),
            &tool,
        );
        if disposition == ToolDisposition::Skip {
            continue;
        }
        let Some(name) = goat_integration_mcp::usable_name(service, &tool) else {
            continue;
        };
        let name = name.as_str().to_owned();
        if !used.insert(name.clone()) {
            continue;
        }
        adapters.push(IntegrationToolAdapter::new(name, service.clone(), tool));
    }
    adapters
}

#[derive(Clone)]
struct IntegrationToolAdapter {
    name: &'static str,
    description: &'static str,
    parameters: Value,
    original_name: String,
    service: Arc<McpService>,
}

impl IntegrationToolAdapter {
    fn new(exposed_name: String, service: Arc<McpService>, tool: CachedTool) -> Self {
        Self {
            name: leak(exposed_name),
            description: leak(tool.description),
            parameters: tool.input_schema,
            original_name: tool.name,
            service,
        }
    }
}

impl Tool for IntegrationToolAdapter {
    fn name(&self) -> &'static str {
        self.name
    }

    fn description(&self) -> &'static str {
        self.description
    }

    fn parameters(&self) -> Value {
        self.parameters.clone()
    }

    fn run<'a>(&'a self, input: &'a str, _ctx: &'a ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let arguments = arguments_from(input)?;
            let binding = IntegrationBinding::from_config(self.connection.clone());
            let session = self
                .service
                .connect(&self.credentials, &binding)
                .await
                .map_err(|err| ToolError::Execution {
                    message: err.to_string(),
                })?;
            let result = self
                .service
                .call(
                    &session,
                    &self.original_name,
                    goat_integration::drop_placeholder_args(arguments),
                )
                .await;
            session.close().await;
            let value = result.map_err(|err| ToolError::Execution {
                message: err.to_string(),
            })?;
            output_from(&self.original_name, value)
        })
    }

    fn display_input(&self, input: &str) -> ToolDisplay {
        ToolDisplay::with_detail(
            format!("{} on {}", self.original_name, self.service.id.as_str()),
            input.to_owned(),
        )
    }
}

fn arguments_from(input: &str) -> Result<Value, ToolError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(Value::Null);
    }
    Ok(serde_json::from_str(trimmed)?)
}

fn output_from(tool_name: &str, value: Value) -> Result<ToolOutput, ToolError> {
    let text = match &value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    Ok(ToolOutput::text(text).with_summary(summary(&text)))
}

fn summary(text: &str) -> String {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .map_or_else(String::new, |line| line.chars().take(80).collect())
}

fn leak(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}
