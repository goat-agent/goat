use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{ModelError, ProviderId};

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Model {
    pub provider: ProviderId,
    pub id: String,
}

impl Model {
    pub fn new(provider: ProviderId, id: impl Into<String>) -> Self {
        Self {
            provider,
            id: id.into(),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn parse(s: &str) -> Result<Self, ModelError> {
        let (prov, id) = s
            .split_once('/')
            .ok_or_else(|| ModelError::BadFormat(s.to_string()))?;
        if prov.is_empty() || id.is_empty() {
            return Err(ModelError::BadFormat(s.to_string()));
        }
        Ok(Self {
            provider: ProviderId::new(prov),
            id: id.to_string(),
        })
    }
}

impl fmt::Display for Model {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.provider, self.id)
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

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ModelInfo {
    pub provider: ProviderId,
    pub id: &'static str,
    pub context: u32,
}

inventory::collect!(ModelInfo);
