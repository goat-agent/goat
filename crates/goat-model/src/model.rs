use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{ModelError, ProviderId};

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Model {
    pub provider: ProviderId,
    pub account: Option<String>,
    pub id: String,
}

impl Model {
    pub fn new(provider: ProviderId, id: impl Into<String>) -> Self {
        Self {
            provider,
            account: None,
            id: id.into(),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn account(&self) -> Option<&str> {
        self.account.as_deref()
    }

    pub fn parse(s: &str) -> Result<Self, ModelError> {
        let (prov, id) = s
            .split_once('/')
            .ok_or_else(|| ModelError::BadFormat(s.to_string()))?;
        let (provider, account) = match prov.split_once(':') {
            Some((provider, account)) => (provider, Some(account.to_string())),
            None => (prov, None),
        };
        if provider.is_empty() || id.is_empty() || account.as_deref() == Some("") {
            return Err(ModelError::BadFormat(s.to_string()));
        }
        Ok(Self {
            provider: ProviderId::from(provider),
            account,
            id: id.to_string(),
        })
    }
}

impl fmt::Display for Model {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.account {
            Some(account) => write!(f, "{}:{}/{}", self.provider, account, self.id),
            None => write!(f, "{}/{}", self.provider, self.id),
        }
    }
}

impl Serialize for Model {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Model {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Model::parse(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_provider_and_id_without_account() {
        let model = Model::parse("anthropic/claude-opus-4-8").unwrap();
        assert_eq!(model.provider.0, "anthropic");
        assert_eq!(model.account(), None);
        assert_eq!(model.id(), "claude-opus-4-8");
        assert_eq!(model.to_string(), "anthropic/claude-opus-4-8");
    }

    #[test]
    fn parses_pinned_account_without_gluing_it_into_the_provider() {
        let model = Model::parse("openai-codex:runbear/gpt-5.6-terra").unwrap();
        assert_eq!(model.provider.0, "openai-codex");
        assert_eq!(model.account(), Some("runbear"));
        assert_eq!(model.id(), "gpt-5.6-terra");
        assert_eq!(model.to_string(), "openai-codex:runbear/gpt-5.6-terra");
    }

    #[test]
    fn keeps_colons_in_the_model_id() {
        let model = Model::parse("anthropic/claude-3:beta").unwrap();
        assert_eq!(model.provider.0, "anthropic");
        assert_eq!(model.account(), None);
        assert_eq!(model.id(), "claude-3:beta");
    }

    #[test]
    fn rejects_empty_components() {
        assert!(Model::parse("anthropic").is_err());
        assert!(Model::parse("/claude").is_err());
        assert!(Model::parse("anthropic/").is_err());
        assert!(Model::parse("anthropic:/claude").is_err());
    }
}
