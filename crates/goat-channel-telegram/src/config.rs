use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TelegramConfig {
    pub(crate) token: String,
    #[serde(default)]
    pub(crate) allowed_user_ids: Vec<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_allowed_user_ids() {
        let cfg: TelegramConfig =
            serde_json::from_str(r#"{"token":"t","allowed_user_ids":[1,2]}"#).unwrap();
        assert_eq!(cfg.token, "t");
        assert_eq!(cfg.allowed_user_ids, vec![1, 2]);
    }

    #[test]
    fn defaults_allowed_user_ids_to_empty() {
        let cfg: TelegramConfig = serde_json::from_str(r#"{"token":"t"}"#).unwrap();
        assert!(cfg.allowed_user_ids.is_empty());
    }

    #[test]
    fn rejects_unknown_fields() {
        let err =
            serde_json::from_str::<TelegramConfig>(r#"{"token":"t","chat_id":1}"#).unwrap_err();
        assert!(err.to_string().contains("unknown field"));
    }
}
