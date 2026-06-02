use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DiscordConfig {
    pub(crate) token: String,
    #[serde(default)]
    pub(crate) intents: Vec<String>,
    #[serde(default)]
    pub(crate) allowed_user_ids: Vec<u64>,
}
