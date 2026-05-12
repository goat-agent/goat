use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TelegramConfig {
    pub(crate) token: String,
    #[serde(default)]
    pub(crate) allowed_user_ids: Vec<i64>,
}
