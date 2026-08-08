use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SlackConfig {
    pub(crate) allowed_user_ids: Option<Vec<String>>,
}
