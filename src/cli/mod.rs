pub mod doctor;
pub mod persona;
pub mod provider;
pub mod setup;
pub mod skill;
pub mod ui;

use std::path::Path;

use anyhow::Result;

pub fn edit_credentials<F: FnOnce(&mut serde_json::Map<String, serde_json::Value>)>(
    path: &Path,
    f: F,
) -> Result<()> {
    let mut map = if path.exists() {
        let raw = std::fs::read_to_string(path)?;
        if raw.trim().is_empty() {
            serde_json::Map::new()
        } else {
            serde_json::from_str(&raw)?
        }
    } else {
        serde_json::Map::new()
    };
    f(&mut map);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let pretty = serde_json::to_string_pretty(&serde_json::Value::Object(map))?;
    std::fs::write(path, format!("{pretty}\n"))?;
    Ok(())
}

pub fn mask_key(key: &str) -> String {
    let n = key.chars().count();
    if n <= 8 {
        return "·".repeat(n);
    }
    let head: String = key.chars().take(4).collect();
    let tail: String = key.chars().skip(n - 4).collect();
    format!("{head}…{tail}")
}
