use std::path::Path;

use tracing::warn;

use crate::{value, write_atomic};

pub(crate) fn read_or_migrate(path: &Path) -> Option<String> {
    if let Ok(raw) = std::fs::read_to_string(path) {
        return Some(raw);
    }
    let legacy = path.with_extension("json");
    let raw = std::fs::read_to_string(&legacy).ok()?;
    let parsed = match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(parsed) => parsed,
        Err(error) => {
            warn!(path = %legacy.display(), %error, "legacy config did not parse; leaving it in place");
            return None;
        }
    };
    let Some(document) = value::document_from_json(&parsed) else {
        warn!(path = %legacy.display(), "legacy config is not an object; leaving it in place");
        return None;
    };
    let rendered = document.to_string();
    retire(path, &legacy, &rendered);
    Some(rendered)
}

fn retire(path: &Path, legacy: &Path, rendered: &str) {
    if let Err(error) = write_atomic(path, rendered.as_bytes()) {
        warn!(path = %path.display(), %error, "could not write the migrated config; retrying on the next read");
        return;
    }
    let retired = legacy.with_extension("json.migrated");
    if let Err(error) = std::fs::rename(legacy, &retired) {
        warn!(path = %legacy.display(), %error, "migrated the config but could not retire the json");
    }
}

#[cfg(test)]
mod tests {
    use super::read_or_migrate;

    #[test]
    fn a_missing_pair_reads_as_nothing() {
        let directory = tempfile::tempdir().unwrap();
        assert!(read_or_migrate(&directory.path().join("config.toml")).is_none());
    }

    #[test]
    fn an_existing_toml_wins_and_the_json_is_left_alone() {
        let directory = tempfile::tempdir().unwrap();
        let toml = directory.path().join("config.toml");
        let json = directory.path().join("config.json");
        std::fs::write(&toml, "theme = \"light\"\n").unwrap();
        std::fs::write(&json, r#"{"theme":"dark"}"#).unwrap();

        assert_eq!(read_or_migrate(&toml).unwrap(), "theme = \"light\"\n");
        assert!(json.exists());
    }

    #[test]
    fn a_legacy_json_becomes_toml_and_is_retired() {
        let directory = tempfile::tempdir().unwrap();
        let toml = directory.path().join("config.toml");
        let json = directory.path().join("config.json");
        std::fs::write(
            &json,
            r#"{"theme":"dark","remotes":{"box":{"host":"1.2.3.4:4317"}}}"#,
        )
        .unwrap();

        let migrated = read_or_migrate(&toml).unwrap();
        assert!(migrated.contains("theme = \"dark\""));
        assert!(migrated.contains("[remotes.box]"));
        assert_eq!(std::fs::read_to_string(&toml).unwrap(), migrated);
        assert!(!json.exists());
        assert!(directory.path().join("config.json.migrated").exists());
    }

    #[test]
    fn a_corrupt_legacy_json_is_never_retired() {
        let directory = tempfile::tempdir().unwrap();
        let toml = directory.path().join("config.toml");
        let json = directory.path().join("config.json");
        std::fs::write(&json, "{ oops").unwrap();

        assert!(read_or_migrate(&toml).is_none());
        assert!(json.exists());
        assert!(!toml.exists());
    }
}
