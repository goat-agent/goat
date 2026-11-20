use serde::Deserialize;

use crate::account;

const CATALOG: &str = include_str!("catalog.toml");

#[derive(Debug, thiserror::Error)]
#[error("read the model catalog")]
pub struct Error(#[from] toml::de::Error);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    pub id: Box<str>,
    pub kind: account::Kind,
    pub display_name: Option<Box<str>>,
    pub efforts: Option<Vec<Box<str>>>,
    pub default_effort: Option<Box<str>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    models: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    id: String,
    provider: Provider,
    name: Option<String>,
    efforts: Option<Vec<String>>,
    default_effort: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Provider {
    OpenAi,
    Anthropic,
}

impl Model {
    pub fn name(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.id)
    }

    pub fn accepts(&self, effort: &str) -> bool {
        self.efforts
            .as_ref()
            .is_none_or(|efforts| efforts.iter().any(|e| &**e == effort))
    }
}

impl From<Entry> for Model {
    fn from(entry: Entry) -> Self {
        Self {
            id: entry.id.into(),
            kind: match entry.provider {
                Provider::OpenAi => account::Kind::OpenAi,
                Provider::Anthropic => account::Kind::Anthropic,
            },
            display_name: entry.name.map(Into::into),
            efforts: entry
                .efforts
                .map(|efforts| efforts.into_iter().map(Into::into).collect()),
            default_effort: entry.default_effort.map(Into::into),
        }
    }
}

pub fn models() -> Result<Vec<Model>, Error> {
    let file: File = toml::from_str(CATALOG)?;
    Ok(file.models.into_iter().map(Model::from).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Provider as Client;

    fn model(efforts: Option<Vec<Box<str>>>) -> Model {
        Model {
            id: "m".into(),
            kind: account::Kind::OpenAi,
            display_name: None,
            efforts,
            default_effort: None,
        }
    }

    #[test]
    fn the_catalog_holds_each_default_model() {
        let models = models().unwrap();
        let missing: Vec<&str> = [account::Kind::OpenAi, account::Kind::Anthropic]
            .into_iter()
            .map(Client::default_model)
            .filter(|default| models.iter().all(|model| &*model.id != *default))
            .collect();
        assert_eq!(missing, Vec::<&str>::new());
    }

    #[test]
    fn each_default_effort_is_one_of_the_efforts() {
        let wrong: Vec<Box<str>> = models()
            .unwrap()
            .into_iter()
            .filter(|model| {
                model
                    .default_effort
                    .as_deref()
                    .is_some_and(|default| !model.accepts(default))
            })
            .map(|model| model.id)
            .collect();
        assert_eq!(wrong, Vec::<Box<str>>::new());
    }

    #[test]
    fn model_with_known_levels_rejects_other_efforts() {
        let model = model(Some(vec!["low".into(), "high".into()]));
        assert!(model.accepts("high"));
        assert!(!model.accepts("ultra"));
    }

    #[test]
    fn model_with_unknown_levels_accepts_any_effort() {
        assert!(model(None).accepts("ultra"));
    }
}
