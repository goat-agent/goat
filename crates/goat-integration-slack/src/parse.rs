use goat_integration::{IntegrationError, IntegrationResult};
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct Mention {
    pub key: String,
    pub channel: String,
    pub channel_name: String,
    pub ts: String,
    pub user: String,
    pub text: String,
    pub raw: Value,
}

impl Mention {
    pub fn is_authored_by(&self, user_id: &str) -> bool {
        !self.user.is_empty() && self.user == user_id
    }

    pub fn summary(&self) -> String {
        let place = if self.channel_name.is_empty() {
            self.channel.clone()
        } else {
            format!("#{}", self.channel_name)
        };
        let who = if self.user.is_empty() {
            "someone".to_string()
        } else {
            format!("<@{}>", self.user)
        };
        format!("{place} — {who}: {}", squeeze(&self.text, 160))
    }
}

pub fn parse_mentions(data: &Value) -> IntegrationResult<Vec<Mention>> {
    match_array(data)
        .ok_or_else(|| {
            IntegrationError::Service(format!("slack response has no message list: {data}"))
        })?
        .iter()
        .map(parse_match)
        .collect()
}

fn match_array(data: &Value) -> Option<&Vec<Value>> {
    if let Some(array) = data.as_array() {
        return Some(array);
    }
    if let Some(array) = data
        .get("messages")
        .and_then(|messages| messages.get("matches"))
        .and_then(Value::as_array)
    {
        return Some(array);
    }
    ["matches", "messages", "results", "items", "nodes"]
        .iter()
        .find_map(|key| data.get(key).and_then(Value::as_array))
}

fn parse_match(node: &Value) -> IntegrationResult<Mention> {
    let ts = string_field(node, "ts")
        .or_else(|| string_field(node, "timestamp"))
        .ok_or_else(|| IntegrationError::Service("slack message match missing `ts`".into()))?;
    let (channel, channel_name) = parse_channel(node);
    Ok(Mention {
        key: format!("{channel}:{ts}"),
        channel,
        channel_name,
        ts,
        user: string_field(node, "user")
            .or_else(|| string_field(node, "user_id"))
            .unwrap_or_default(),
        text: string_field(node, "text").unwrap_or_default(),
        raw: node.clone(),
    })
}

fn parse_channel(node: &Value) -> (String, String) {
    let Some(channel) = node.get("channel") else {
        return (
            string_field(node, "channel_id").unwrap_or_else(|| "?".into()),
            String::new(),
        );
    };
    if let Some(id) = channel.as_str() {
        return (id.to_string(), String::new());
    }
    (
        string_field(channel, "id").unwrap_or_else(|| "?".into()),
        string_field(channel, "name").unwrap_or_default(),
    )
}

fn string_field(node: &Value, key: &str) -> Option<String> {
    node.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn squeeze(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let kept: String = flat.chars().take(max).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn match_node(channel: &str, ts: &str, user: &str) -> Value {
        json!({
            "ts": ts,
            "user": user,
            "username": "someone",
            "text": "hey <@U0OWNER> can you look at this",
            "channel": { "id": channel, "name": "eng" },
            "permalink": "https://acme.slack.com/archives/C1/p1"
        })
    }

    #[test]
    fn parses_native_search_envelope() {
        let data = json!({
            "messages": {
                "matches": [match_node("C1", "1700000000.000100", "U1")],
                "pagination": { "page_count": 3 }
            }
        });
        let mentions = parse_mentions(&data).unwrap();
        assert_eq!(mentions.len(), 1);
        assert_eq!(mentions[0].key, "C1:1700000000.000100");
        assert_eq!(mentions[0].channel_name, "eng");
        assert_eq!(mentions[0].user, "U1");
        assert_eq!(
            mentions[0].raw["permalink"],
            "https://acme.slack.com/archives/C1/p1"
        );
    }

    #[test]
    fn parses_bare_array_and_alternate_wrappers() {
        let bare = json!([match_node("C1", "1.1", "U1")]);
        assert_eq!(parse_mentions(&bare).unwrap().len(), 1);
        for key in ["matches", "messages", "results", "items", "nodes"] {
            let wrapped = json!({ key: [match_node("C2", "2.2", "U2")] });
            assert_eq!(parse_mentions(&wrapped).unwrap()[0].channel, "C2");
        }
    }

    #[test]
    fn channel_may_be_a_bare_id_or_absent() {
        let flat = json!([{ "ts": "3.3", "channel": "C9", "text": "hi" }]);
        let mention = &parse_mentions(&flat).unwrap()[0];
        assert_eq!(mention.channel, "C9");
        assert_eq!(mention.key, "C9:3.3");
        assert!(mention.channel_name.is_empty());

        let none = json!([{ "ts": "4.4", "text": "hi" }]);
        assert_eq!(parse_mentions(&none).unwrap()[0].channel, "?");
    }

    #[test]
    fn missing_ts_is_rejected_and_unknown_shapes_error() {
        assert!(parse_mentions(&json!([{ "channel": "C1" }])).is_err());
        assert!(parse_mentions(&json!({ "unexpected": true })).is_err());
        assert!(parse_mentions(&json!("text blob")).is_err());
    }

    #[test]
    fn self_authorship_needs_a_non_empty_user() {
        let mine = parse_mentions(&json!([match_node("C1", "1.1", "U0OWNER")])).unwrap();
        assert!(mine[0].is_authored_by("U0OWNER"));
        assert!(!mine[0].is_authored_by("U1"));

        let anon = parse_mentions(&json!([{ "ts": "1.1", "text": "x" }])).unwrap();
        assert!(!anon[0].is_authored_by(""));
    }

    #[test]
    fn summary_is_flattened_and_clamped() {
        let mention = &parse_mentions(&json!([match_node("C1", "1.1", "U1")])).unwrap()[0];
        assert_eq!(
            mention.summary(),
            "#eng — <@U1>: hey <@U0OWNER> can you look at this"
        );

        let long = json!([{ "ts": "1.1", "text": "a ".repeat(200) }]);
        let squeezed = parse_mentions(&long).unwrap()[0].summary();
        assert!(squeezed.ends_with('…'));
    }
}
