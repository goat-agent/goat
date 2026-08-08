use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DiscordConfig {
    #[serde(default)]
    pub(crate) intents: Vec<String>,
    pub(crate) allowed_user_ids: Option<Vec<u64>>,
}
