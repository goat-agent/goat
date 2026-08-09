use serde_json::{Number, Value as Json};
use toml_edit::{
    Array, ArrayOfTables, DocumentMut, Formatted, InlineTable, Item, Table, Value as Toml,
};

pub(crate) fn to_json(item: &Item) -> Json {
    match item {
        Item::None => Json::Null,
        Item::Value(value) => value_to_json(value),
        Item::Table(table) => table_to_json(table),
        Item::ArrayOfTables(tables) => Json::Array(tables.iter().map(table_to_json).collect()),
    }
}

pub(crate) fn from_json(value: &Json) -> Option<Item> {
    match value {
        Json::Null => None,
        Json::Object(map) => {
            let mut table = Table::new();
            for (key, value) in map {
                if let Some(item) = from_json(value) {
                    table.insert(key, item);
                }
            }
            collapse(&mut table);
            Some(Item::Table(table))
        }
        Json::Array(items) if !items.is_empty() && items.iter().all(Json::is_object) => {
            let mut tables = ArrayOfTables::new();
            for item in items {
                if let Some(Item::Table(table)) = from_json(item) {
                    tables.push(table);
                }
            }
            Some(Item::ArrayOfTables(tables))
        }
        other => json_to_toml(other).map(Item::Value),
    }
}

pub(crate) fn document_from_json(value: &Json) -> Option<DocumentMut> {
    let Json::Object(map) = value else {
        return None;
    };
    let mut document = DocumentMut::new();
    for (key, value) in map {
        if let Some(item) = from_json(value) {
            document.insert(key, item);
        }
    }
    Some(document)
}

fn collapse(table: &mut Table) {
    let only_tables = !table.is_empty()
        && table
            .iter()
            .all(|(_, item)| item.is_table() || item.is_array_of_tables());
    table.set_implicit(only_tables);
}

fn table_to_json(table: &Table) -> Json {
    Json::Object(
        table
            .iter()
            .map(|(key, item)| (key.to_owned(), to_json(item)))
            .collect(),
    )
}

fn inline_to_json(table: &InlineTable) -> Json {
    Json::Object(
        table
            .iter()
            .map(|(key, value)| (key.to_owned(), value_to_json(value)))
            .collect(),
    )
}

fn value_to_json(value: &Toml) -> Json {
    match value {
        Toml::String(raw) => Json::String(raw.value().clone()),
        Toml::Integer(raw) => Json::Number(Number::from(*raw.value())),
        Toml::Float(raw) => Number::from_f64(*raw.value()).map_or(Json::Null, Json::Number),
        Toml::Boolean(raw) => Json::Bool(*raw.value()),
        Toml::Datetime(raw) => Json::String(raw.value().to_string()),
        Toml::Array(array) => Json::Array(array.iter().map(value_to_json).collect()),
        Toml::InlineTable(table) => inline_to_json(table),
    }
}

fn json_to_toml(value: &Json) -> Option<Toml> {
    match value {
        Json::Null => None,
        Json::Bool(raw) => Some(Toml::Boolean(Formatted::new(*raw))),
        Json::Number(number) => number
            .as_i64()
            .map(|raw| Toml::Integer(Formatted::new(raw)))
            .or_else(|| number.as_f64().map(|raw| Toml::Float(Formatted::new(raw)))),
        Json::String(raw) => Some(Toml::String(Formatted::new(raw.clone()))),
        Json::Array(items) => {
            let mut array = Array::new();
            for item in items.iter().filter_map(json_to_toml) {
                array.push(item);
            }
            Some(Toml::Array(array))
        }
        Json::Object(map) => {
            let mut table = InlineTable::new();
            for (key, value) in map {
                if let Some(value) = json_to_toml(value) {
                    table.insert(key, value);
                }
            }
            Some(Toml::InlineTable(table))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{document_from_json, to_json};
    use serde_json::json;

    fn render(value: &serde_json::Value) -> String {
        document_from_json(value).unwrap().to_string()
    }

    #[test]
    fn scalars_precede_tables_regardless_of_key_order() {
        let rendered = render(&json!({
            "devices": { "bind": "0.0.0.0:4317" },
            "theme": "dark",
            "default_remote": "box"
        }));
        let devices = rendered.find("[devices]").unwrap();
        assert!(rendered.find("theme =").unwrap() < devices);
        assert!(rendered.find("default_remote =").unwrap() < devices);
    }

    #[test]
    fn an_empty_object_stays_a_visible_table() {
        let rendered = render(&json!({ "channels": { "discord": {} } }));
        assert!(rendered.contains("[channels.discord]"));
        assert!(!rendered.contains("[channels]"));
    }

    #[test]
    fn a_table_holding_values_and_tables_keeps_its_own_header() {
        let rendered = render(&json!({
            "search": { "default_target": "searxng/home", "accounts": { "searxng": {} } }
        }));
        assert!(rendered.contains("[search]"));
        assert!(rendered.contains("[search.accounts.searxng]"));
    }

    #[test]
    fn a_list_of_objects_becomes_an_array_of_tables() {
        let rendered = render(&json!({
            "watch": { "inbox": [
                { "source": "linear", "query": "is:open" },
                { "source": "github", "query": "assignee:@me" }
            ] }
        }));
        assert_eq!(rendered.matches("[[watch.inbox]]").count(), 2);
        assert!(!rendered.contains('{'));
    }

    #[test]
    fn a_list_of_scalars_stays_inline() {
        let rendered = render(&json!({ "tools": ["*"], "advertised": [] }));
        assert!(rendered.contains("tools = [\"*\"]"));
        assert!(rendered.contains("advertised = []"));
    }

    #[test]
    fn null_disappears_because_toml_has_none() {
        let rendered = render(&json!({ "theme": "dark", "last_dir": null }));
        assert!(!rendered.contains("last_dir"));
    }

    #[test]
    fn toml_deserializes_straight_into_an_opaque_json_value() {
        #[derive(serde::Deserialize)]
        struct Holder {
            linear: serde_json::Value,
        }

        let holder: Holder = toml_edit::de::from_str(
            "[linear]\naccount = \"default\"\nlimit = 25\nenabled = true\ndeny = [\"a\"]\n",
        )
        .unwrap();
        assert_eq!(holder.linear["account"], "default");
        assert_eq!(holder.linear["limit"], 25);
        assert_eq!(holder.linear["enabled"], true);
        assert_eq!(holder.linear["deny"][0], "a");
    }

    #[test]
    fn an_empty_toml_table_reaches_serde_json_as_an_empty_object() {
        #[derive(serde::Deserialize)]
        struct Holder {
            channels: std::collections::BTreeMap<String, serde_json::Value>,
        }

        let holder: Holder = toml_edit::de::from_str("[channels.discord]\n").unwrap();
        assert_eq!(holder.channels["discord"], json!({}));
    }

    #[test]
    fn round_trips_an_opaque_binding_payload() {
        let payload = json!({
            "account": "default",
            "tools": { "deny": ["a", "b"] },
            "limit": 25,
            "enabled": true
        });
        let document = document_from_json(&json!({ "linear": payload.clone() })).unwrap();
        let parsed: toml_edit::DocumentMut = document.to_string().parse().unwrap();
        assert_eq!(to_json(&parsed["linear"]), payload);
    }
}
