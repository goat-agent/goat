use std::collections::BTreeMap;

use goat_auth::{AuthError, Credential, CredentialKey, CredentialStore, SecretString};
use goat_types::ChannelId;

use crate::{ChannelError, ChannelResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SecretSpec {
    pub slot: &'static str,
    pub label: &'static str,
}

impl SecretSpec {
    #[must_use]
    pub const fn new(slot: &'static str, label: &'static str) -> Self {
        Self { slot, label }
    }
}

#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct ChannelMetadata {
    pub display: &'static str,
    pub setup: &'static str,
    pub secrets: &'static [SecretSpec],
}

impl ChannelMetadata {
    #[must_use]
    pub const fn new(
        display: &'static str,
        setup: &'static str,
        secrets: &'static [SecretSpec],
    ) -> Self {
        Self {
            display,
            setup,
            secrets,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ChannelSecrets(BTreeMap<String, SecretString>);

impl ChannelSecrets {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, slot: impl Into<String>, secret: impl Into<SecretString>) {
        self.0.insert(slot.into(), secret.into());
    }

    #[must_use]
    pub fn get(&self, slot: &str) -> Option<&str> {
        self.0.get(slot).map(SecretString::expose)
    }

    pub fn require(&self, slot: &str) -> ChannelResult<&str> {
        self.get(slot).ok_or_else(|| {
            ChannelError::Auth(format!(
                "missing secret `{slot}`; re-run `goat agent channel add`"
            ))
        })
    }

    #[must_use]
    pub fn slots(&self) -> Vec<&str> {
        self.0.keys().map(String::as_str).collect()
    }

    #[must_use]
    pub fn missing<'a>(&self, specs: &'a [SecretSpec]) -> Vec<&'a str> {
        specs
            .iter()
            .filter(|spec| self.get(spec.slot).is_none())
            .map(|spec| spec.slot)
            .collect()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl FromIterator<(String, SecretString)> for ChannelSecrets {
    fn from_iter<T: IntoIterator<Item = (String, SecretString)>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

#[must_use]
pub fn secret_key(channel: &ChannelId, account: &str, slot: &str) -> CredentialKey {
    CredentialKey::channel(channel.as_str(), account, slot)
}

#[must_use]
pub fn load(
    credentials: &CredentialStore,
    channel: &ChannelId,
    account: &str,
    specs: &[SecretSpec],
) -> ChannelSecrets {
    let mut out = ChannelSecrets::new();
    for spec in specs {
        if let Some(credential) = credentials.get(&secret_key(channel, account, spec.slot)) {
            out.insert(spec.slot, SecretString::from(credential.bearer()));
        }
    }
    out
}

pub fn save(
    credentials: &CredentialStore,
    channel: &ChannelId,
    account: &str,
    secrets: &ChannelSecrets,
) -> Result<(), AuthError> {
    for slot in secrets.slots() {
        let Some(secret) = secrets.get(slot) else {
            continue;
        };
        credentials.store(
            &secret_key(channel, account, slot),
            Credential::ApiKey(SecretString::from(secret)),
        )?;
    }
    Ok(())
}

pub fn forget(
    credentials: &CredentialStore,
    channel: &ChannelId,
    account: &str,
    specs: &[SecretSpec],
) -> Result<(), AuthError> {
    for spec in specs {
        credentials.remove(&secret_key(channel, account, spec.slot))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPECS: &[SecretSpec] = &[
        SecretSpec::new("bot_token", "bot token"),
        SecretSpec::new("app_token", "app token"),
    ];

    fn store() -> (tempfile::TempDir, CredentialStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path().join("credentials.json"));
        (dir, store)
    }

    fn slack() -> ChannelId {
        ChannelId::from_static("slack")
    }

    fn two_tokens() -> ChannelSecrets {
        let mut secrets = ChannelSecrets::new();
        secrets.insert("bot_token", SecretString::from("xoxb-1"));
        secrets.insert("app_token", SecretString::from("xapp-1"));
        secrets
    }

    #[test]
    fn require_names_the_missing_slot_and_points_at_the_fix() {
        assert_eq!(two_tokens().require("bot_token").unwrap(), "xoxb-1");
        let err = ChannelSecrets::new().require("app_token").unwrap_err();
        let ChannelError::Auth(message) = err else {
            panic!("expected an auth error");
        };
        assert!(message.contains("app_token"));
        assert!(message.contains("goat agent channel add"));
    }

    #[test]
    fn missing_reports_every_absent_slot_in_spec_order() {
        assert!(two_tokens().missing(SPECS).is_empty());
        assert_eq!(
            ChannelSecrets::new().missing(SPECS),
            vec!["bot_token", "app_token"]
        );
        let mut partial = ChannelSecrets::new();
        partial.insert("bot_token", SecretString::from("xoxb-1"));
        assert_eq!(partial.missing(SPECS), vec!["app_token"]);
    }

    #[test]
    fn each_slot_round_trips_as_its_own_credential_entry() {
        let (_dir, store) = store();
        save(&store, &slack(), "personal", &two_tokens()).unwrap();

        assert_eq!(store.entries().len(), 2);
        let loaded = load(&store, &slack(), "personal", SPECS);
        assert_eq!(loaded.get("bot_token"), Some("xoxb-1"));
        assert_eq!(loaded.get("app_token"), Some("xapp-1"));
    }

    #[test]
    fn secrets_are_scoped_per_agent() {
        let (_dir, store) = store();
        save(&store, &slack(), "personal", &two_tokens()).unwrap();
        let mut work = ChannelSecrets::new();
        work.insert("bot_token", SecretString::from("xoxb-work"));
        save(&store, &slack(), "work", &work).unwrap();

        assert_eq!(
            load(&store, &slack(), "personal", SPECS).get("bot_token"),
            Some("xoxb-1")
        );
        assert_eq!(
            load(&store, &slack(), "work", SPECS).get("bot_token"),
            Some("xoxb-work")
        );
    }

    #[test]
    fn secrets_are_scoped_per_channel() {
        let (_dir, store) = store();
        save(&store, &slack(), "personal", &two_tokens()).unwrap();

        let discord = ChannelId::from_static("discord");
        let specs = &[SecretSpec::new("bot_token", "bot token")];
        assert!(load(&store, &discord, "personal", specs).is_empty());
    }

    #[test]
    fn forget_clears_every_declared_slot_and_leaves_other_agents_alone() {
        let (_dir, store) = store();
        save(&store, &slack(), "personal", &two_tokens()).unwrap();
        save(&store, &slack(), "work", &two_tokens()).unwrap();

        forget(&store, &slack(), "personal", SPECS).unwrap();
        assert!(load(&store, &slack(), "personal", SPECS).is_empty());
        assert_eq!(
            load(&store, &slack(), "work", SPECS).get("bot_token"),
            Some("xoxb-1")
        );
    }

    #[test]
    fn load_returns_a_partial_map_so_require_reports_the_gap() {
        let (_dir, store) = store();
        let mut only_bot = ChannelSecrets::new();
        only_bot.insert("bot_token", SecretString::from("xoxb-1"));
        save(&store, &slack(), "personal", &only_bot).unwrap();

        let loaded = load(&store, &slack(), "personal", SPECS);
        assert_eq!(loaded.get("bot_token"), Some("xoxb-1"));
        assert!(loaded.require("app_token").is_err());
    }

    #[test]
    fn load_ignores_slots_the_channel_does_not_declare() {
        let (_dir, store) = store();
        let mut extra = ChannelSecrets::new();
        extra.insert("legacy_token", SecretString::from("stale"));
        save(&store, &slack(), "personal", &extra).unwrap();

        assert!(load(&store, &slack(), "personal", SPECS).is_empty());
    }

    #[test]
    fn debug_output_never_leaks_a_secret() {
        let rendered = format!("{:?}", two_tokens());
        assert!(!rendered.contains("xoxb-1"));
        assert!(!rendered.contains("xapp-1"));
        assert!(rendered.contains("bot_token"));
    }
}
