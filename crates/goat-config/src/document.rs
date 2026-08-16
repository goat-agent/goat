use std::path::{Path, PathBuf};

use serde_json::{Map, Value as Json};
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};
use tracing::warn;

use crate::paths::config_path;
use crate::settings::{Config, RemoteEntry, SearchAccountConfig, SettingsError, ThemeChoice};
use crate::{migrate, write_atomic};

pub struct ConfigDocument {
    path: PathBuf,
    document: DocumentMut,
}

impl ConfigDocument {
    pub fn load() -> Result<Self, SettingsError> {
        let path = config_path().ok_or(SettingsError::NoHome)?;
        Ok(Self::load_path(&path))
    }

    pub fn load_path(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            document: read_document(path),
        }
    }

    #[must_use]
    pub fn config(&self) -> Config {
        toml_edit::de::from_document(self.document.clone()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), SettingsError> {
        write_atomic(&self.path, self.document.to_string().as_bytes())
    }

    pub fn set_theme(&mut self, theme: ThemeChoice) {
        let name = match theme {
            ThemeChoice::Dark => "dark",
            ThemeChoice::Light => "light",
        };
        section(&mut self.document, &["ui"])["theme"] = value(name);
    }

    pub fn set_mouse_capture(&mut self, enabled: bool) {
        section(&mut self.document, &["ui"])["mouse_capture"] = value(enabled);
    }

    pub fn set_browser(&mut self, enabled: bool) {
        section(&mut self.document, &["tools"])["browser"] = value(enabled);
    }

    pub fn set_default_remote(&mut self, name: Option<&str>) {
        match name {
            Some(name) => self.document["default_remote"] = value(name),
            None => {
                self.document.remove("default_remote");
            }
        }
    }

    pub fn upsert_remote(&mut self, name: &str, entry: &RemoteEntry) {
        let table = section(&mut self.document, &["remotes", name]);
        table["host"] = value(entry.host.as_str());
        table["fingerprint"] = value(entry.fingerprint.as_str());
        match &entry.last_dir {
            Some(directory) => table["last_dir"] = value(directory.as_str()),
            None => {
                table.remove("last_dir");
            }
        }
    }

    pub fn set_remote_last_dir(&mut self, name: &str, directory: &str) {
        section(&mut self.document, &["remotes", name])["last_dir"] = value(directory);
    }

    pub fn remove_remote(&mut self, name: &str) {
        container(&mut self.document, &["remotes"]).remove(name);
    }

    pub fn upsert_provider(&mut self, name: &str, endpoint: &str) {
        section(&mut self.document, &["providers", name])["endpoint"] = value(endpoint);
    }

    pub fn remove_provider(&mut self, name: &str) {
        container(&mut self.document, &["providers"]).remove(name);
    }

    pub fn set_search_default(&mut self, target: Option<&str>) {
        let table = section(&mut self.document, &["search"]);
        match target {
            Some(target) => table["default"] = value(target),
            None => {
                table.remove("default");
            }
        }
    }

    pub fn upsert_search_account(&mut self, account: &SearchAccountConfig) {
        let Some(entry) = flatten(account) else {
            warn!(
                target = account.target(),
                "could not encode the search account"
            );
            return;
        };
        let target = account.target();
        let accounts = accounts_mut(&mut self.document);
        let existing = accounts
            .iter()
            .position(|table| account_target(table) == target);
        match existing {
            Some(index) => *accounts.get_mut(index).expect("index just found") = entry,
            None => accounts.push(entry),
        }
    }

    pub fn remove_search_account(&mut self, target: &str) {
        accounts_mut(&mut self.document).retain(|table| account_target(table) != target);
    }

    pub fn merge_integration(&mut self, kind: &str, patch: &Map<String, Json>) {
        let table = section(&mut self.document, &["integrations", kind]);
        for (key, value) in patch {
            if let Some(item) = crate::value::from_json(value) {
                table.insert(key, item);
            }
        }
    }

    pub fn remove_integration(&mut self, kind: &str) {
        container(&mut self.document, &["integrations"]).remove(kind);
    }
}

pub(crate) fn read_document(path: &Path) -> DocumentMut {
    let Some(raw) = migrate::read_or_migrate(path) else {
        return DocumentMut::new();
    };
    match raw.parse::<DocumentMut>() {
        Ok(document) => document,
        Err(error) => {
            warn!(path = %path.display(), %error, "config did not parse; setting it aside");
            let _ = std::fs::rename(path, path.with_extension("toml.corrupt"));
            DocumentMut::new()
        }
    }
}

pub(crate) fn section<'a>(document: &'a mut DocumentMut, path: &[&str]) -> &'a mut Table {
    walk(document, path, false)
}

pub(crate) fn container<'a>(document: &'a mut DocumentMut, path: &[&str]) -> &'a mut Table {
    walk(document, path, true)
}

fn walk<'a>(document: &'a mut DocumentMut, path: &[&str], hide_leaf: bool) -> &'a mut Table {
    let mut table = document.as_table_mut();
    let leaf = path.len() - 1;
    for (index, key) in path.iter().enumerate() {
        let hidden = index < leaf || hide_leaf;
        let entry = table.entry(key).or_insert_with(|| {
            let mut created = Table::new();
            created.set_implicit(hidden);
            Item::Table(created)
        });
        if !entry.is_table() {
            *entry = Item::Table(Table::new());
        }
        table = entry.as_table_mut().expect("replaced when not a table");
    }
    table
}

fn accounts_mut(document: &mut DocumentMut) -> &mut ArrayOfTables {
    let search = container(document, &["search"]);
    let entry = search
        .entry("accounts")
        .or_insert_with(|| Item::ArrayOfTables(ArrayOfTables::new()));
    if !entry.is_array_of_tables() {
        *entry = Item::ArrayOfTables(ArrayOfTables::new());
    }
    entry
        .as_array_of_tables_mut()
        .expect("replaced when not an array of tables")
}

fn account_target(table: &Table) -> String {
    let field = |key: &str| {
        table
            .get(key)
            .and_then(Item::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    format!("{}/{}", field("provider"), field("account"))
}

fn flatten(account: &SearchAccountConfig) -> Option<Table> {
    let document = toml_edit::ser::to_document(account).ok()?;
    Some(document.as_table().clone())
}

#[cfg(test)]
mod tests {
    use super::ConfigDocument;
    use crate::settings::{RemoteEntry, SearchAccountConfig, ThemeChoice};

    fn at(directory: &std::path::Path, raw: &str) -> ConfigDocument {
        let path = directory.join("config.toml");
        std::fs::write(&path, raw).unwrap();
        ConfigDocument::load_path(&path)
    }

    #[test]
    fn comments_and_unknown_keys_survive_a_mutation() {
        let directory = tempfile::tempdir().unwrap();
        let mut document = at(
            directory.path(),
            "future_extension = true\n\n# my listener\n[ui]\ntheme = \"dark\"\n\n[search]\nunknown_nested = 7\n",
        );
        document.set_theme(ThemeChoice::Light);
        document.save().unwrap();

        let saved = std::fs::read_to_string(directory.path().join("config.toml")).unwrap();
        assert!(saved.contains("# my listener"));
        assert!(saved.contains("future_extension = true"));
        assert!(saved.contains("unknown_nested = 7"));
        assert!(saved.contains("theme = \"light\""));
    }

    #[test]
    fn a_new_entry_lands_beside_its_siblings_not_at_the_end() {
        let directory = tempfile::tempdir().unwrap();
        let mut document = at(
            directory.path(),
            "[providers.alpha]\nendpoint = \"https://a\"\n\n[proxy]\nenabled = true\n",
        );
        document.upsert_provider("beta", "https://b");
        document.save().unwrap();

        let saved = std::fs::read_to_string(directory.path().join("config.toml")).unwrap();
        let beta = saved.find("[providers.beta]").unwrap();
        assert!(saved.find("[providers.alpha]").unwrap() < beta);
        assert!(beta < saved.find("[proxy]").unwrap());
    }

    #[test]
    fn reads_its_own_document_as_typed_config() {
        let directory = tempfile::tempdir().unwrap();
        let mut document = at(directory.path(), "");
        assert!(document.config().default_remote.is_none());

        document.upsert_remote(
            "box",
            &RemoteEntry {
                host: "1.2.3.4:4317".to_owned(),
                fingerprint: "ab12".to_owned(),
                last_dir: None,
            },
        );
        document.set_default_remote(Some("box"));

        let config = document.config();
        assert_eq!(config.default_remote.as_deref(), Some("box"));
        assert_eq!(config.remotes["box"].host, "1.2.3.4:4317");
        assert!(config.remotes["box"].last_dir.is_none());
    }

    #[test]
    fn search_accounts_are_replaced_by_target_not_appended() {
        let directory = tempfile::tempdir().unwrap();
        let mut document = at(directory.path(), "");
        document.upsert_search_account(&SearchAccountConfig::Searxng {
            account: "home".to_owned(),
            endpoint: "https://one".to_owned(),
        });
        document.upsert_search_account(&SearchAccountConfig::Searxng {
            account: "home".to_owned(),
            endpoint: "https://two".to_owned(),
        });
        document.upsert_search_account(&SearchAccountConfig::Brave {
            account: "work".to_owned(),
        });

        let accounts = document.config().search.accounts;
        assert_eq!(accounts.len(), 2);
        assert_eq!(
            accounts[0],
            SearchAccountConfig::Searxng {
                account: "home".to_owned(),
                endpoint: "https://two".to_owned(),
            }
        );

        document.remove_search_account("searxng/home");
        assert_eq!(document.config().search.accounts.len(), 1);
    }

    #[test]
    fn an_integration_binding_with_no_keys_stays_a_binding() {
        let directory = tempfile::tempdir().unwrap();
        let mut document = at(directory.path(), "");
        document.merge_integration("github", &serde_json::Map::new());
        document.merge_integration(
            "linear",
            serde_json::json!({ "account": "default" })
                .as_object()
                .unwrap(),
        );
        document.save().unwrap();

        let saved = std::fs::read_to_string(directory.path().join("config.toml")).unwrap();
        assert!(saved.contains("[integrations.github]"));
        assert!(!saved.contains("[integrations]"));

        let config = document.config();
        assert_eq!(config.integrations["github"], serde_json::json!({}));
        assert_eq!(config.integrations["linear"]["account"], "default");
    }

    #[test]
    fn emptying_a_map_leaves_no_header_behind() {
        let directory = tempfile::tempdir().unwrap();
        let mut document = at(directory.path(), "");
        document.merge_integration("github", &serde_json::Map::new());
        document.remove_integration("github");
        document.remove_remote("never-there");
        document.remove_provider("never-there");
        document.save().unwrap();

        let saved = std::fs::read_to_string(directory.path().join("config.toml")).unwrap();
        assert_eq!(saved.trim(), "");
    }

    #[test]
    fn a_corrupt_document_is_set_aside_rather_than_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let document = at(directory.path(), "[ui\ntheme =");
        assert!(document.config().default_remote.is_none());
        assert!(directory.path().join("config.toml.corrupt").exists());
    }
}
