use std::collections::{BTreeMap, BTreeSet};

use goat_auth::{Credential, CredentialKey, CredentialStore, SecretString};
use goat_mcp::{ConfigFile, ServerConfig, ValueSource};

use crate::cli::SecretPolicy;
use crate::ui;

pub struct SecretWrite {
    key: CredentialKey,
    value: String,
}

pub fn protect_sensitive(
    server_name: &str,
    account: &str,
    server: &mut ServerConfig,
    policy: SecretPolicy,
) -> color_eyre::Result<Vec<SecretWrite>> {
    let mut writes = Vec::new();
    match server {
        ServerConfig::Stdio(server) => {
            protect_map(
                server_name,
                account,
                "env",
                &mut server.env,
                policy,
                &mut writes,
            )?;
            protect_arguments(server_name, account, &mut server.args, policy, &mut writes)?;
        }
        ServerConfig::Http(server) => {
            protect_map(
                server_name,
                account,
                "header",
                &mut server.headers,
                policy,
                &mut writes,
            )?;
            protect_url(server_name, &mut server.url, policy)?;
        }
    }
    Ok(writes)
}

pub fn save_with_secrets(
    file: &mut ConfigFile,
    credentials: &CredentialStore,
    writes: &[SecretWrite],
) -> color_eyre::Result<()> {
    let backups: Vec<_> = writes
        .iter()
        .map(|write| (write.key.clone(), credentials.get(&write.key)))
        .collect();
    for write in writes {
        if let Err(error) = credentials.store(
            &write.key,
            Credential::ApiKey(SecretString::from(write.value.as_str())),
        ) {
            restore_credentials(credentials, &backups);
            return Err(ui::report(error.to_string()));
        }
    }
    if let Err(error) = file.save() {
        restore_credentials(credentials, &backups);
        return Err(ui::report(error.to_string()));
    }
    Ok(())
}

pub fn has_sensitive_literals(server: &ServerConfig) -> bool {
    let values = match server {
        ServerConfig::Stdio(server) => &server.env,
        ServerConfig::Http(server) => &server.headers,
    };
    let map_has_secrets = values
        .iter()
        .any(|(name, value)| sensitive_name(name) && matches!(value, ValueSource::Literal(_)));
    map_has_secrets
        || matches!(server, ServerConfig::Stdio(server) if server.args.windows(2).any(|pair| {
            matches!((&pair[0], &pair[1]),
                (ValueSource::Literal(flag), ValueSource::Literal(_)) if sensitive_flag(flag))
        }))
        || matches!(server, ServerConfig::Stdio(server) if server.args.iter().any(|argument| {
            matches!(argument, ValueSource::Literal(value) if embedded_secret_argument(value))
        }))
        || matches!(server, ServerConfig::Http(server) if url_has_credentials(&server.url))
}

pub fn redacted(server: &ServerConfig) -> ServerConfig {
    let mut server = server.clone();
    let values = match &mut server {
        ServerConfig::Stdio(server) => &mut server.env,
        ServerConfig::Http(server) => &mut server.headers,
    };
    for (name, value) in values {
        if sensitive_name(name) && matches!(value, ValueSource::Literal(_)) {
            *value = ValueSource::Literal("***".to_owned());
        }
    }
    if let ServerConfig::Stdio(server) = &mut server {
        for argument in &mut server.args {
            if let ValueSource::Literal(value) = argument
                && embedded_secret_argument(value)
            {
                let name = value
                    .split_once('=')
                    .map_or(value.as_str(), |(name, _)| name);
                *value = format!("{name}=***");
            }
        }
        for index in 1..server.args.len() {
            if matches!(&server.args[index - 1], ValueSource::Literal(flag) if sensitive_flag(flag))
                && matches!(server.args[index], ValueSource::Literal(_))
            {
                server.args[index] = ValueSource::Literal("***".to_owned());
            }
        }
    }
    if let ServerConfig::Http(server) = &mut server {
        server.url = redacted_url(&server.url);
    }
    server
}

pub fn value_names(values: &BTreeMap<String, ValueSource>) -> String {
    values
        .iter()
        .map(|(name, value)| match value {
            ValueSource::Literal(_) => name.clone(),
            ValueSource::Env { env } => format!("{name} ← ${env}"),
            ValueSource::Secret { .. } => format!("{name} ← credentials"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn server_summary(server: &ServerConfig) -> String {
    match server {
        ServerConfig::Stdio(server) => shell_words(&server.command, &server.args),
        ServerConfig::Http(server) => redacted_url(&server.url),
    }
}

pub fn redacted_url(raw: &str) -> String {
    rewrite_url(raw, true)
}

fn protect_url(
    server_name: &str,
    url: &mut String,
    policy: SecretPolicy,
) -> color_eyre::Result<()> {
    if !url_has_credentials(url) {
        return Ok(());
    }
    match policy {
        SecretPolicy::Literal => Ok(()),
        SecretPolicy::Omit => {
            *url = rewrite_url(url, false);
            Ok(())
        }
        SecretPolicy::Store => Err(ui::report_hint(
            format!("MCP server `{server_name}` has credentials in its URL"),
            "use an Authorization header, `bearerTokenEnvVar`, or an OAuth login instead",
        )),
        SecretPolicy::Error => Err(ui::report(format!(
            "MCP server `{server_name}` contains literal credentials"
        ))),
    }
}

fn protect_arguments(
    server_name: &str,
    account: &str,
    arguments: &mut Vec<ValueSource>,
    policy: SecretPolicy,
    writes: &mut Vec<SecretWrite>,
) -> color_eyre::Result<()> {
    let sensitive: Vec<_> = arguments
        .windows(2)
        .enumerate()
        .filter_map(|(index, pair)| match (&pair[0], &pair[1]) {
            (ValueSource::Literal(flag), ValueSource::Literal(value)) if sensitive_flag(flag) => {
                Some((index, index + 1, value.clone()))
            }
            _ => None,
        })
        .collect();
    let embedded: Vec<_> = arguments
        .iter()
        .enumerate()
        .filter_map(|(index, argument)| match argument {
            ValueSource::Literal(value) if embedded_secret_argument(value) => {
                Some((index, value.clone()))
            }
            _ => None,
        })
        .collect();
    let mut omitted = BTreeSet::new();
    for (flag_index, value_index, value) in sensitive {
        match policy {
            SecretPolicy::Store => {
                writes.push(SecretWrite {
                    key: CredentialKey::mcp(server_name, account, format!("arg:{value_index}")),
                    value,
                });
                arguments[value_index] = ValueSource::Secret { secret: true };
            }
            SecretPolicy::Literal => {}
            SecretPolicy::Omit => {
                omitted.insert(flag_index);
                omitted.insert(value_index);
            }
            SecretPolicy::Error => return Err(literal_credentials(server_name)),
        }
    }
    for (index, value) in embedded {
        match policy {
            SecretPolicy::Store => {
                writes.push(SecretWrite {
                    key: CredentialKey::mcp(server_name, account, format!("arg:{index}")),
                    value,
                });
                arguments[index] = ValueSource::Secret { secret: true };
            }
            SecretPolicy::Literal => {}
            SecretPolicy::Omit => {
                omitted.insert(index);
            }
            SecretPolicy::Error => return Err(literal_credentials(server_name)),
        }
    }
    if !omitted.is_empty() {
        let mut index = 0;
        arguments.retain(|_| {
            let keep = !omitted.contains(&index);
            index += 1;
            keep
        });
    }
    Ok(())
}

fn protect_map(
    server_name: &str,
    account: &str,
    kind: &str,
    values: &mut BTreeMap<String, ValueSource>,
    policy: SecretPolicy,
    writes: &mut Vec<SecretWrite>,
) -> color_eyre::Result<()> {
    let names: Vec<_> = values
        .iter()
        .filter(|(name, value)| sensitive_name(name) && matches!(value, ValueSource::Literal(_)))
        .map(|(name, _)| name.clone())
        .collect();
    for name in names {
        let Some(ValueSource::Literal(value)) = values.get(&name).cloned() else {
            continue;
        };
        match policy {
            SecretPolicy::Store => {
                writes.push(SecretWrite {
                    key: CredentialKey::mcp(server_name, account, format!("{kind}:{name}")),
                    value,
                });
                values.insert(name, ValueSource::Secret { secret: true });
            }
            SecretPolicy::Literal => {}
            SecretPolicy::Omit => {
                values.remove(&name);
            }
            SecretPolicy::Error => return Err(literal_credentials(server_name)),
        }
    }
    Ok(())
}

fn restore_credentials(
    credentials: &CredentialStore,
    backups: &[(CredentialKey, Option<Credential>)],
) {
    for (key, value) in backups {
        match value {
            Some(value) => {
                let _ = credentials.store(key, value.clone());
            }
            None => {
                let _ = credentials.remove(key);
            }
        }
    }
}

fn literal_credentials(server_name: &str) -> color_eyre::Report {
    ui::report(format!(
        "MCP server `{server_name}` contains literal credentials"
    ))
}

fn sensitive_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "token",
        "key",
        "secret",
        "password",
        "authorization",
        "cookie",
    ]
    .iter()
    .any(|needle| name.contains(needle))
}

pub fn shell_words(command: &str, args: &[ValueSource]) -> String {
    let mut words = vec![command.to_owned()];
    for (index, argument) in args.iter().enumerate() {
        let sensitive = index > 0
            && matches!(&args[index - 1], ValueSource::Literal(flag) if sensitive_flag(flag));
        words.push(if sensitive {
            "***".to_owned()
        } else {
            match argument {
                ValueSource::Literal(value) if embedded_secret_argument(value) => value
                    .split_once('=')
                    .map_or_else(|| "***".to_owned(), |(name, _)| format!("{name}=***")),
                ValueSource::Literal(value) => value.clone(),
                ValueSource::Env { env } => format!("${{{env}}}"),
                ValueSource::Secret { .. } => "<credentials>".to_owned(),
            }
        });
    }
    words.join(" ")
}

fn embedded_secret_argument(value: &str) -> bool {
    value
        .split_once('=')
        .is_some_and(|(name, value)| !value.is_empty() && sensitive_name(name))
}

fn sensitive_flag(value: &str) -> bool {
    value.starts_with('-') && sensitive_name(value)
}

fn url_has_credentials(raw: &str) -> bool {
    reqwest::Url::parse(raw).is_ok_and(|url| {
        !url.username().is_empty()
            || url.password().is_some()
            || url.query_pairs().any(|(name, _)| sensitive_name(&name))
    })
}

fn rewrite_url(raw: &str, redact: bool) -> String {
    let Ok(mut url) = reqwest::Url::parse(raw) else {
        return raw.to_owned();
    };
    if !url.username().is_empty() {
        let _ = url.set_username(if redact { "***" } else { "" });
    }
    if url.password().is_some() {
        let _ = url.set_password(redact.then_some("***"));
    }
    let pairs: Vec<_> = url
        .query_pairs()
        .filter_map(|(name, value)| {
            if sensitive_name(&name) {
                redact.then(|| (name.into_owned(), "***".to_owned()))
            } else {
                Some((name.into_owned(), value.into_owned()))
            }
        })
        .collect();
    if url.query().is_some() {
        url.query_pairs_mut().clear().extend_pairs(pairs);
    }
    url.into()
}

#[cfg(test)]
mod tests {
    use goat_mcp::StdioConfig;

    use super::*;

    #[test]
    fn sensitive_values_move_to_credentials() {
        let mut server = ServerConfig::Stdio(StdioConfig {
            command: "x".to_owned(),
            args: Vec::new(),
            env: BTreeMap::from([
                (
                    "API_TOKEN".to_owned(),
                    ValueSource::Literal("secret".to_owned()),
                ),
                ("MODE".to_owned(), ValueSource::Literal("dev".to_owned())),
            ]),
        });
        let writes = protect_sensitive("one", "user", &mut server, SecretPolicy::Store).unwrap();
        assert_eq!(writes.len(), 1);
        let ServerConfig::Stdio(server) = server else {
            panic!("stdio")
        };
        assert_eq!(
            server.env.get("API_TOKEN"),
            Some(&ValueSource::Secret { secret: true })
        );
        assert_eq!(
            server.env.get("MODE"),
            Some(&ValueSource::Literal("dev".to_owned()))
        );
    }

    #[test]
    fn sensitive_command_arguments_move_to_credentials() {
        let mut server = ServerConfig::Stdio(StdioConfig {
            command: "x".to_owned(),
            args: vec![
                ValueSource::Literal("--api-key".to_owned()),
                ValueSource::Literal("secret".to_owned()),
                ValueSource::Literal("--token=second".to_owned()),
            ],
            env: BTreeMap::new(),
        });
        let writes = protect_sensitive("one", "user", &mut server, SecretPolicy::Store).unwrap();
        assert_eq!(writes.len(), 2);
        let ServerConfig::Stdio(server) = server else {
            panic!("stdio")
        };
        assert_eq!(
            server.args,
            [
                ValueSource::Literal("--api-key".to_owned()),
                ValueSource::Secret { secret: true },
                ValueSource::Secret { secret: true }
            ]
        );
    }
}
