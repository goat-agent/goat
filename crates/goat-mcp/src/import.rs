use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use crate::{HttpConfig, McpError, ServerConfig, StdioConfig, ValueSource};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportFormat {
    Claude,
    VsCode,
    Direct,
}

impl std::fmt::Display for ImportFormat {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Claude => "Claude",
            Self::VsCode => "VS Code",
            Self::Direct => "direct",
        })
    }
}

#[derive(Debug, Clone)]
pub struct ImportCandidate {
    pub name: String,
    pub server: Option<ServerConfig>,
    pub unsupported: Vec<String>,
    pub error: Option<String>,
}

impl ImportCandidate {
    pub fn usable(&self) -> bool {
        self.server.is_some() && self.error.is_none()
    }
}

pub struct ImportSet {
    pub format: ImportFormat,
    pub candidates: Vec<ImportCandidate>,
    pub warnings: Vec<String>,
}

pub fn parse_import(raw: &[u8]) -> Result<ImportSet, McpError> {
    let value: Value = if let Ok(value) = serde_json::from_slice(raw) {
        value
    } else {
        let raw = std::str::from_utf8(raw)
            .map_err(|error| McpError::Config(format!("MCP config is not UTF-8: {error}")))?;
        serde_json::from_str(&normalize_jsonc(raw)?)?
    };
    let object = value
        .as_object()
        .ok_or_else(|| McpError::Config("imported MCP config must be a JSON object".to_owned()))?;
    let (format, servers, warnings) = if let Some(servers) = object
        .get("mcpServers")
        .filter(|servers| is_server_map(servers))
    {
        (
            ImportFormat::Claude,
            server_object(servers)?,
            wrapper_warnings(object, "mcpServers"),
        )
    } else if let Some(servers) = object
        .get("servers")
        .filter(|servers| is_server_map(servers))
    {
        (
            ImportFormat::VsCode,
            server_object(servers)?,
            wrapper_warnings(object, "servers"),
        )
    } else {
        (ImportFormat::Direct, object, Vec::new())
    };
    let mut candidates: Vec<_> = servers
        .iter()
        .map(|(name, value)| parse_candidate(name, value))
        .collect();
    candidates.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(ImportSet {
        format,
        candidates,
        warnings,
    })
}

fn is_server_map(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|servers| servers.values().all(Value::is_object))
}

fn normalize_jsonc(raw: &str) -> Result<String, McpError> {
    let mut without_comments = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(character) = chars.next() {
        if in_string {
            without_comments.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        if character == '"' {
            in_string = true;
            without_comments.push(character);
            continue;
        }
        if character == '/' && chars.peek() == Some(&'/') {
            chars.next();
            for character in chars.by_ref() {
                if character == '\n' {
                    without_comments.push('\n');
                    break;
                }
            }
            continue;
        }
        if character == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut previous = '\0';
            let mut closed = false;
            for character in chars.by_ref() {
                if character == '\n' {
                    without_comments.push('\n');
                }
                if previous == '*' && character == '/' {
                    closed = true;
                    break;
                }
                previous = character;
            }
            if !closed {
                return Err(McpError::Config(
                    "imported MCP config has an unclosed block comment".to_owned(),
                ));
            }
            continue;
        }
        without_comments.push(character);
    }
    Ok(remove_trailing_commas(&without_comments))
}

fn remove_trailing_commas(raw: &str) -> String {
    let mut normalized = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(character) = chars.next() {
        if in_string {
            normalized.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        if character == '"' {
            in_string = true;
            normalized.push(character);
            continue;
        }
        if character == ',' {
            let mut lookahead = chars.clone();
            if lookahead
                .find(|character| !character.is_whitespace())
                .is_some_and(|next| next == '}' || next == ']')
            {
                continue;
            }
        }
        normalized.push(character);
    }
    normalized
}

fn server_object(value: &Value) -> Result<&Map<String, Value>, McpError> {
    value
        .as_object()
        .ok_or_else(|| McpError::Config("MCP server collection must be a JSON object".to_owned()))
}

fn wrapper_warnings(object: &Map<String, Value>, wrapper: &str) -> Vec<String> {
    object
        .keys()
        .filter(|key| key.as_str() != wrapper)
        .map(|key| format!("top-level `{key}` is not imported"))
        .collect()
}

fn parse_candidate(name: &str, value: &Value) -> ImportCandidate {
    let Some(raw) = value.as_object() else {
        return failed(name, "server entry must be a JSON object");
    };
    if raw.get("disabled").and_then(Value::as_bool) == Some(true) {
        return failed(name, "server is disabled");
    }
    let kind = raw.get("type").or_else(|| raw.get("transport"));
    if let Some(kind) = kind {
        let Some(kind) = kind.as_str() else {
            return failed(name, "`type` and `transport` must be strings");
        };
        match kind {
            "stdio" | "http" | "streamable-http" | "streamableHttp" => {}
            "sse" => {
                return failed(name, "SSE transport is not supported; use Streamable HTTP");
            }
            _ => return failed(name, &format!("unsupported transport `{kind}`")),
        }
    }
    let declared = kind.and_then(Value::as_str);
    if raw.contains_key("command") {
        if declared.is_some_and(|kind| kind != "stdio") {
            return failed(name, "stdio server `type` does not match its `command`");
        }
        parse_stdio(name, raw)
    } else if raw.contains_key("url") {
        if declared == Some("stdio") {
            return failed(name, "HTTP server `type` does not match its `url`");
        }
        parse_http(name, raw)
    } else {
        failed(name, "server needs either `command` or `url`")
    }
}

fn parse_stdio(name: &str, raw: &Map<String, Value>) -> ImportCandidate {
    let Some(command) = raw.get("command").and_then(Value::as_str) else {
        return failed(name, "`command` must be a string");
    };
    let args = match value_array(raw.get("args"), "args") {
        Ok(args) => args,
        Err(error) => return failed(name, &error),
    };
    let env = match value_map(raw.get("env"), "env") {
        Ok(env) => env,
        Err(error) => return failed(name, &error),
    };
    let mut unsupported = unsupported(
        raw,
        &["command", "args", "env", "type", "transport", "disabled"],
    );
    if command.starts_with("./") || command.starts_with("../") {
        unsupported.push("relative command path".to_owned());
    }
    candidate(
        name,
        ServerConfig::Stdio(StdioConfig {
            command: command.to_owned(),
            args,
            env,
        }),
        unsupported,
    )
}

fn parse_http(name: &str, raw: &Map<String, Value>) -> ImportCandidate {
    let Some(url) = raw.get("url").and_then(Value::as_str) else {
        return failed(name, "`url` must be a string");
    };
    if url::Url::parse(url).is_err() {
        return failed(name, "`url` is not a valid absolute URL");
    }
    let (headers, inferred_bearer) = match header_values(raw.get("headers")) {
        Ok(headers) => headers,
        Err(error) => return failed(name, &error),
    };
    let explicit_bearer = match raw.get("bearerTokenEnvVar") {
        Some(Value::String(value)) if !value.is_empty() && !value.contains("${") => {
            Some(value.clone())
        }
        Some(_) => return failed(name, "`bearerTokenEnvVar` must be a string"),
        None => None,
    };
    if explicit_bearer.is_some() && inferred_bearer.is_some() && explicit_bearer != inferred_bearer
    {
        return failed(
            name,
            "HTTP server declares two different bearer token variables",
        );
    }
    let bearer_token_env_var = explicit_bearer.or(inferred_bearer);
    candidate(
        name,
        ServerConfig::Http(HttpConfig {
            url: url.to_owned(),
            headers,
            bearer_token_env_var,
        }),
        unsupported(
            raw,
            &[
                "url",
                "headers",
                "bearerTokenEnvVar",
                "type",
                "transport",
                "disabled",
            ],
        ),
    )
}

fn header_values(
    value: Option<&Value>,
) -> Result<(BTreeMap<String, ValueSource>, Option<String>), String> {
    let Some(value) = value else {
        return Ok((BTreeMap::new(), None));
    };
    let Some(values) = value.as_object() else {
        return Err("`headers` must be an object".to_owned());
    };
    let mut headers = BTreeMap::new();
    let mut bearer = None;
    for (key, value) in values {
        if key.eq_ignore_ascii_case("authorization")
            && let Some(variable) = value.as_str().and_then(bearer_env_reference)
        {
            bearer = Some(variable);
            continue;
        }
        let parsed = if let Some(value) = value.as_str() {
            imported_string(value, &format!("headers.{key}"))?
        } else {
            serde_json::from_value(value.clone())
                .map_err(|_| format!("`headers.{key}` must be a string or value reference"))?
        };
        headers.insert(key.clone(), parsed);
    }
    Ok((headers, bearer))
}

fn bearer_env_reference(value: &str) -> Option<String> {
    let expression = value.strip_prefix("Bearer ")?;
    let variable = expression
        .strip_prefix("${env:")
        .and_then(|value| value.strip_suffix('}'))
        .or_else(|| {
            expression
                .strip_prefix("${")
                .and_then(|value| value.strip_suffix('}'))
                .filter(|value| !value.contains(':'))
        })?;
    (!variable.is_empty()).then(|| variable.to_owned())
}

fn value_array(value: Option<&Value>, field: &str) -> Result<Vec<ValueSource>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let Some(values) = value.as_array() else {
        return Err(format!("`{field}` must be an array of strings"));
    };
    values
        .iter()
        .map(|value| {
            if let Some(value) = value.as_str() {
                return imported_string(value, field);
            }
            serde_json::from_value(value.clone())
                .map_err(|_| format!("`{field}` must contain strings or value references"))
        })
        .collect()
}

fn value_map(value: Option<&Value>, field: &str) -> Result<BTreeMap<String, ValueSource>, String> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let Some(values) = value.as_object() else {
        return Err(format!("`{field}` must be an object"));
    };
    values
        .iter()
        .map(|(key, value)| {
            let parsed = if let Some(value) = value.as_str() {
                imported_string(value, &format!("{field}.{key}"))?
            } else {
                serde_json::from_value(value.clone())
                    .map_err(|_| format!("`{field}.{key}` must be a string or value reference"))?
            };
            Ok((key.clone(), parsed))
        })
        .collect()
}

fn imported_string(value: &str, field: &str) -> Result<ValueSource, String> {
    if let Some(source) = env_reference(value) {
        return Ok(source);
    }
    if value.contains("${") {
        return Err(format!(
            "`{field}` uses an input expression Goat cannot resolve"
        ));
    }
    Ok(ValueSource::Literal(value.to_owned()))
}

fn env_reference(value: &str) -> Option<ValueSource> {
    value
        .strip_prefix("${env:")
        .and_then(|value| value.strip_suffix('}'))
        .filter(|value| !value.is_empty())
        .map(|env| ValueSource::Env {
            env: env.to_owned(),
        })
}

fn unsupported(raw: &Map<String, Value>, known: &[&str]) -> Vec<String> {
    let known: BTreeSet<_> = known.iter().copied().collect();
    raw.keys()
        .filter(|key| !known.contains(key.as_str()))
        .cloned()
        .collect()
}

fn candidate(name: &str, server: ServerConfig, unsupported: Vec<String>) -> ImportCandidate {
    if has_stored_references(&server) {
        return failed(
            name,
            "stored credential references cannot be copied from another config",
        );
    }
    ImportCandidate {
        name: name.to_owned(),
        server: Some(server),
        unsupported,
        error: None,
    }
}

fn has_stored_references(server: &ServerConfig) -> bool {
    match server {
        ServerConfig::Stdio(server) => server
            .args
            .iter()
            .chain(server.env.values())
            .any(|value| matches!(value, ValueSource::Secret { .. })),
        ServerConfig::Http(server) => server
            .headers
            .values()
            .any(|value| matches!(value, ValueSource::Secret { .. })),
    }
}

fn failed(name: &str, error: &str) -> ImportCandidate {
    ImportCandidate {
        name: name.to_owned(),
        server: None,
        unsupported: Vec::new(),
        error: Some(error.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_claude_and_vscode_wrappers() {
        let claude = parse_import(br#"{"mcpServers":{"a":{"command":"x"}}}"#).unwrap();
        assert_eq!(claude.format, ImportFormat::Claude);
        assert!(claude.candidates[0].usable());
        let vscode =
            parse_import(br#"{"servers":{"a":{"type":"http","url":"https://example.test/mcp"}}}"#)
                .unwrap();
        assert_eq!(vscode.format, ImportFormat::VsCode);
        assert!(vscode.candidates[0].usable());
    }

    #[test]
    fn one_bad_server_does_not_hide_a_good_server() {
        let set =
            parse_import(br#"{"mcpServers":{"bad":{"args":[]},"good":{"command":"x"}}}"#).unwrap();
        assert_eq!(set.candidates.len(), 2);
        assert!(!set.candidates[0].usable());
        assert!(set.candidates[1].usable());
    }

    #[test]
    fn env_references_are_not_imported_as_literals() {
        let set =
            parse_import(br#"{"servers":{"a":{"command":"x","env":{"TOKEN":"${env:TOKEN}"}}}}"#)
                .unwrap();
        let ServerConfig::Stdio(stdio) = set.candidates[0].server.as_ref().unwrap() else {
            panic!("stdio")
        };
        assert_eq!(
            stdio.env.get("TOKEN"),
            Some(&ValueSource::Env {
                env: "TOKEN".to_owned()
            })
        );
    }

    #[test]
    fn unsupported_fields_are_reported_per_server() {
        let set = parse_import(
            br#"{"servers":{"a":{"command":"x","timeout":1000},"b":{"command":"y"}}}"#,
        )
        .unwrap();
        assert_eq!(set.candidates[0].unsupported, ["timeout"]);
        assert!(set.candidates[1].unsupported.is_empty());
    }

    #[test]
    fn vscode_input_references_are_not_silently_copied() {
        let set =
            parse_import(br#"{"servers":{"a":{"command":"x","env":{"TOKEN":"${input:token}"}}}}"#)
                .unwrap();
        assert!(!set.candidates[0].usable());
    }

    #[test]
    fn bearer_header_environment_references_use_the_native_field() {
        let set = parse_import(
            br#"{"servers":{"a":{"url":"https://example.test/mcp","headers":{"Authorization":"Bearer ${env:TOKEN}"}}}}"#,
        )
        .unwrap();
        let ServerConfig::Http(http) = set.candidates[0].server.as_ref().unwrap() else {
            panic!("http")
        };
        assert_eq!(http.bearer_token_env_var.as_deref(), Some("TOKEN"));
        assert!(http.headers.is_empty());
    }

    #[test]
    fn imports_jsonc_comments_and_trailing_commas() {
        let set = parse_import(
            br#"{
                // shared project config
                "servers": {
                    "a": {
                        "command": "https://example.test//not-a-comment",
                    },
                },
            }"#,
        )
        .unwrap();
        assert!(set.candidates[0].usable());
    }

    #[test]
    fn rejects_unclosed_jsonc_comments() {
        assert!(parse_import(br#"{"servers": {}} /*"#).is_err());
    }
}
