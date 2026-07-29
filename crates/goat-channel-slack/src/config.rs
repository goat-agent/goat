use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SlackConfig {
    #[serde(default)]
    pub(crate) allowed_user_ids: Vec<String>,
}
