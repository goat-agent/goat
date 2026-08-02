use std::path::Path;

use goat_auth::{Credential, CredentialStore, SecretString};
use goat_channel::{ChannelSecrets, SecretSpec};
use goat_types::ChannelId;
use serde_json::Value;
use tracing::{info, warn};

pub(crate) fn resolve_for_binding(
    credentials: &CredentialStore,
    agents_dir: &Path,
    slug: &str,
    channel: &ChannelId,
    binding_name: &str,
    specs: &[SecretSpec],
    config: &mut Value,
) -> ChannelSecrets {
    let mut secrets = goat_channel::load_secrets(credentials, channel, slug, specs);
    let in_config = read_config_secrets(config, specs);
    if in_config.is_empty() {
        return secrets;
    }

    let missing = secrets.missing(specs);
    let mut drained = Vec::new();
    for (slot, value) in &in_config {
        if !missing.contains(slot) {
            drained.push(*slot);
            continue;
        }
        let key = goat_channel::secret_key(channel, slug, slot);
        match credentials.store(&key, Credential::ApiKey(SecretString::from(value.as_str()))) {
            Ok(()) => {
                secrets.insert(*slot, SecretString::from(value.as_str()));
                drained.push(*slot);
            }
            Err(e) => warn!(
                agent = %slug,
                channel = %channel,
                slot = *slot,
                error = ?e,
                "could not move a channel secret into the credential store; leaving it in config.json",
            ),
        }
    }

    if !drained.is_empty() {
        remove_slots(config, &drained);
        evict_from_disk(agents_dir, slug, channel, binding_name, &drained);
    }
    secrets
}

fn read_config_secrets(config: &Value, specs: &[SecretSpec]) -> Vec<(&'static str, String)> {
    let Some(obj) = config.as_object() else {
        return Vec::new();
    };
    specs
        .iter()
        .filter_map(|spec| {
            let raw = obj.get(spec.slot)?.as_str()?.trim();
            (!raw.is_empty()).then(|| (spec.slot, raw.to_owned()))
        })
        .collect()
}

fn remove_slots(config: &mut Value, slots: &[&str]) {
    if let Some(obj) = config.as_object_mut() {
        for slot in slots {
            obj.remove(*slot);
        }
    }
}

fn evict_from_disk(
    agents_dir: &Path,
    slug: &str,
    channel: &ChannelId,
    binding_name: &str,
    stored: &[&str],
) {
    let path = agents_dir.join(slug).join("config.json");
    match strip_stored_slots(&path, binding_name, stored) {
        Ok(()) => info!(
            agent = %slug,
            channel = %channel,
            slots = ?stored,
            "moved channel secrets out of config.json into the credential store",
        ),
        Err(e) => warn!(
            agent = %slug,
            path = %path.display(),
            error = ?e,
            "stored the channel secrets but could not rewrite config.json; delete them by hand",
        ),
    }
}

fn strip_stored_slots(path: &Path, binding_name: &str, slots: &[&str]) -> anyhow::Result<()> {
    let raw = std::fs::read_to_string(path)?;
    let mut doc: Value = serde_json::from_str(&raw)?;
    let Some(entry) = doc
        .get_mut("channels")
        .and_then(Value::as_object_mut)
        .and_then(|channels| channels.get_mut(binding_name))
        .and_then(Value::as_object_mut)
    else {
        return Ok(());
    };
    let mut changed = false;
    for slot in slots {
        changed |= entry.remove(*slot).is_some();
    }
    if !changed {
        return Ok(());
    }
    let mut serialized = serde_json::to_string_pretty(&doc)?;
    serialized.push('\n');
    std::fs::write(path, serialized)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SPECS: &[SecretSpec] = &[
        SecretSpec::new("bot_token", "bot token"),
        SecretSpec::new("app_token", "app token"),
    ];

    fn slack() -> ChannelId {
        ChannelId::from_static("slack")
    }

    fn agent_dir(root: &Path, config: &Value) -> std::path::PathBuf {
        let dir = root.join("personal");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.json"),
            serde_json::to_string_pretty(config).unwrap(),
        )
        .unwrap();
        dir
    }

    fn read_config(dir: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(dir.join("config.json")).unwrap()).unwrap()
    }

    #[test]
    fn read_config_secrets_takes_declared_slots_only() {
        let config = json!({
            "bot_token": "xoxb-1",
            "app_token": "xapp-1",
            "allowed_user_ids": ["U1"],
            "unrelated": "keep"
        });
        assert_eq!(
            read_config_secrets(&config, SPECS),
            vec![
                ("bot_token", "xoxb-1".to_owned()),
                ("app_token", "xapp-1".to_owned())
            ]
        );
    }

    #[test]
    fn read_config_secrets_skips_blank_and_non_string_values() {
        let config = json!({ "bot_token": "   ", "app_token": 7 });
        assert!(read_config_secrets(&config, SPECS).is_empty());
        assert!(read_config_secrets(&json!("nope"), SPECS).is_empty());
        assert!(read_config_secrets(&json!({}), SPECS).is_empty());
    }

    #[test]
    fn read_config_secrets_trims_surrounding_whitespace() {
        let config = json!({ "bot_token": "  xoxb-1\n" });
        assert_eq!(
            read_config_secrets(&config, SPECS),
            vec![("bot_token", "xoxb-1".to_owned())]
        );
    }

    #[test]
    fn strip_stored_slots_removes_only_the_named_binding_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = agent_dir(
            tmp.path(),
            &json!({
                "default_model": "keep",
                "channels": {
                    "slack": { "bot_token": "xoxb-1", "allowed_user_ids": ["U1"] },
                    "discord": { "token": "keep-me" }
                }
            }),
        );

        strip_stored_slots(&dir.join("config.json"), "slack", &["bot_token"]).unwrap();

        let doc = read_config(&dir);
        assert!(doc["channels"]["slack"].get("bot_token").is_none());
        assert_eq!(doc["channels"]["slack"]["allowed_user_ids"], json!(["U1"]));
        assert_eq!(doc["channels"]["discord"]["token"], "keep-me");
        assert_eq!(doc["default_model"], "keep");
    }

    #[test]
    fn strip_stored_slots_is_a_noop_when_the_binding_is_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = agent_dir(tmp.path(), &json!({ "channels": {} }));
        let before = std::fs::read_to_string(dir.join("config.json")).unwrap();

        strip_stored_slots(&dir.join("config.json"), "slack", &["bot_token"]).unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.join("config.json")).unwrap(),
            before
        );
    }

    #[test]
    fn resolve_for_binding_migrates_a_legacy_secret_end_to_end() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = agent_dir(
            tmp.path(),
            &json!({
                "channels": {
                    "slack": { "bot_token": "xoxb-1", "allowed_user_ids": ["U1"] }
                }
            }),
        );
        let store = CredentialStore::new(tmp.path().join("credentials.json"));
        let mut config = json!({ "bot_token": "xoxb-1", "allowed_user_ids": ["U1"] });

        let secrets = resolve_for_binding(
            &store,
            tmp.path(),
            "personal",
            &slack(),
            "slack",
            SPECS,
            &mut config,
        );

        assert_eq!(secrets.get("bot_token"), Some("xoxb-1"));
        assert!(config.get("bot_token").is_none());
        assert_eq!(config["allowed_user_ids"], json!(["U1"]));
        assert!(
            read_config(&dir)["channels"]["slack"]
                .get("bot_token")
                .is_none()
        );
        assert_eq!(
            read_config(&dir)["channels"]["slack"]["allowed_user_ids"],
            json!(["U1"])
        );
    }

    #[test]
    fn resolve_for_binding_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        agent_dir(
            tmp.path(),
            &json!({ "channels": { "slack": { "bot_token": "xoxb-1" } } }),
        );
        let store = CredentialStore::new(tmp.path().join("credentials.json"));

        let mut first = json!({ "bot_token": "xoxb-1" });
        resolve_for_binding(
            &store,
            tmp.path(),
            "personal",
            &slack(),
            "slack",
            SPECS,
            &mut first,
        );
        let mut second = json!({});
        let secrets = resolve_for_binding(
            &store,
            tmp.path(),
            "personal",
            &slack(),
            "slack",
            SPECS,
            &mut second,
        );

        assert_eq!(secrets.get("bot_token"), Some("xoxb-1"));
        assert_eq!(store.entries().len(), 1);
    }

    #[test]
    fn a_stored_secret_wins_over_a_stale_config_value_and_the_stale_one_is_drained() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = agent_dir(
            tmp.path(),
            &json!({ "channels": { "slack": { "bot_token": "xoxb-stale" } } }),
        );
        let store = CredentialStore::new(tmp.path().join("credentials.json"));
        let mut fresh = ChannelSecrets::new();
        fresh.insert("bot_token", SecretString::from("xoxb-fresh"));
        goat_channel::save_secrets(&store, &slack(), "personal", &fresh).unwrap();

        let mut config = json!({ "bot_token": "xoxb-stale" });
        let secrets = resolve_for_binding(
            &store,
            tmp.path(),
            "personal",
            &slack(),
            "slack",
            SPECS,
            &mut config,
        );

        assert_eq!(secrets.get("bot_token"), Some("xoxb-fresh"));
        assert!(config.get("bot_token").is_none());
        assert!(
            read_config(&dir)["channels"]["slack"]
                .get("bot_token")
                .is_none()
        );
    }

    #[test]
    fn resolve_for_binding_returns_nothing_when_there_is_nothing_to_find() {
        let tmp = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(tmp.path().join("credentials.json"));
        let mut config = json!({ "allowed_user_ids": ["U1"] });

        let secrets = resolve_for_binding(
            &store,
            tmp.path(),
            "personal",
            &slack(),
            "slack",
            SPECS,
            &mut config,
        );

        assert!(secrets.is_empty());
        assert_eq!(config["allowed_user_ids"], json!(["U1"]));
    }
}
