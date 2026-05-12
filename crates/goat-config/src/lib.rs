use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use goat_credentials::Credentials;
use goat_llm::Model;
use goat_persona::{PersonaBinding, PersonaConfig, PersonalityCard};
use goat_types::PersonaId;
use serde::Deserialize;
use thiserror::Error;
use tracing::warn;

const DEFAULT_HISTORY_WINDOW: usize = 20;
const LEGACY_STATE_DB: &str = "state.db";

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("yaml: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid model in persona '{slug}': {source}")]
    Model {
        slug: String,
        #[source]
        source: goat_llm::ModelError,
    },
    #[error("persona '{slug}' has no persona.md")]
    MissingPersona { slug: String },
    #[error("personas dir not found: {0}")]
    MissingPersonasDir(PathBuf),
}

#[derive(Clone, Debug)]
pub struct GoatPaths {
    pub root: PathBuf,
    pub credentials_json: PathBuf,
    pub personas_dir: PathBuf,
    pub skills_dir: PathBuf,
    pub state_db: PathBuf,
    pub logs_dir: PathBuf,
}

impl GoatPaths {
    pub fn default_layout() -> Result<Self> {
        Ok(Self::from_root(home_root()?))
    }

    pub fn from_root(root: PathBuf) -> Self {
        Self {
            credentials_json: root.join("credentials.json"),
            personas_dir: root.join("personas"),
            skills_dir: root.join("skills"),
            state_db: root.join("goat.db"),
            logs_dir: root.join("logs"),
            root,
        }
    }
}

fn home_root() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("$HOME is not set"))?;
    Ok(PathBuf::from(home).join(".goat"))
}

#[derive(Debug)]
pub struct LoadedConfig {
    pub paths: GoatPaths,
    pub credentials: Credentials,
    pub personas: Vec<PersonaConfig>,
}

pub async fn load() -> Result<LoadedConfig> {
    load_from(GoatPaths::default_layout()?).await
}

pub async fn load_from(paths: GoatPaths) -> Result<LoadedConfig> {
    fs::create_dir_all(&paths.root).ok();
    fs::create_dir_all(&paths.logs_dir).ok();
    fs::create_dir_all(&paths.personas_dir).ok();
    fs::create_dir_all(&paths.skills_dir).ok();

    migrate_legacy_db(&paths);

    let credentials = Credentials::load(&paths.credentials_json)
        .with_context(|| format!("loading {}", paths.credentials_json.display()))?;

    let personas = scan_personas(&paths.personas_dir).await?;

    Ok(LoadedConfig {
        paths,
        credentials,
        personas,
    })
}

fn migrate_legacy_db(paths: &GoatPaths) {
    let legacy = paths.root.join(LEGACY_STATE_DB);
    if legacy.exists() && !paths.state_db.exists() {
        match fs::rename(&legacy, &paths.state_db) {
            Ok(_) => warn!(
                from = %legacy.display(),
                to = %paths.state_db.display(),
                "renamed legacy database",
            ),
            Err(e) => warn!(
                error = ?e,
                "failed to rename legacy state.db; v0 history will not be visible",
            ),
        }
    }
}

async fn scan_personas(dir: &Path) -> Result<Vec<PersonaConfig>> {
    if !dir.exists() {
        return Err(ConfigError::MissingPersonasDir(dir.to_path_buf()).into());
    }
    let mut personas = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        let slug = match path.file_name().and_then(|s| s.to_str()) {
            Some(s) if !s.starts_with('.') => s.to_string(),
            _ => continue,
        };
        match load_persona(&path, &slug) {
            Ok(p) => personas.push(p),
            Err(e) => warn!(persona = %slug, error = ?e, "skipping persona"),
        }
    }
    Ok(personas)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersonaFrontMatter {
    #[serde(default)]
    display: Option<String>,
    model: String,
    #[serde(default)]
    history_window: Option<usize>,
}

fn load_persona(dir: &Path, slug: &str) -> Result<PersonaConfig> {
    let persona_md = dir.join("persona.md");
    if !persona_md.exists() {
        return Err(ConfigError::MissingPersona {
            slug: slug.to_string(),
        }
        .into());
    }
    let raw = fs::read_to_string(&persona_md)?;
    let (front_str, body) =
        split_front_matter(&raw).ok_or_else(|| anyhow!("persona.md missing YAML front-matter"))?;
    let front: PersonaFrontMatter = serde_yaml::from_str(front_str)?;

    let model = Model::parse(&front.model).map_err(|source| ConfigError::Model {
        slug: slug.to_string(),
        source,
    })?;

    let personality = PersonalityCard {
        system_prompt: body.trim().to_string(),
        traits: Vec::new(),
        source_path: persona_md,
    };

    let bindings = scan_bindings(dir)?;

    Ok(PersonaConfig {
        id: PersonaId::from_slug(slug),
        slug: slug.to_string(),
        display: front.display.unwrap_or_else(|| slug.to_string()),
        personality,
        default_model: model,
        history_window: front.history_window.unwrap_or(DEFAULT_HISTORY_WINDOW),
        bindings,
    })
}

fn scan_bindings(dir: &Path) -> Result<Vec<PersonaBinding>> {
    let mut bindings = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file() {
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let name = match path.file_stem().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let raw = fs::read_to_string(&path)?;
        let config: serde_json::Value =
            serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
        bindings.push(PersonaBinding { name, config });
    }
    bindings.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(bindings)
}

pub fn split_front_matter(s: &str) -> Option<(&str, &str)> {
    let s = s
        .strip_prefix("---\n")
        .or_else(|| s.strip_prefix("---\r\n"))?;
    let end = s.find("\n---")?;
    let (front, after) = s.split_at(end);
    let body = after
        .trim_start_matches("\n---")
        .trim_start_matches(['\n', '\r']);
    Some((front, body))
}

pub fn front_field(s: &str, key: &str) -> Option<String> {
    let (front, _) = split_front_matter(s)?;
    for line in front.lines() {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim() == key {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter_split_basic() {
        let raw = "---\nmodel: openai/gpt-4o-mini\n---\n\nbody text\n";
        let (front, body) = split_front_matter(raw).unwrap();
        assert!(front.contains("model: openai/gpt-4o-mini"));
        assert_eq!(body.trim(), "body text");
    }

    #[test]
    fn front_matter_absent_returns_none() {
        assert!(split_front_matter("no front matter").is_none());
    }

    #[test]
    fn front_matter_parses_full_fields() {
        let raw =
            "---\ndisplay: 개발가재\nmodel: openai/gpt-4o-mini\nhistory_window: 30\n---\nbody";
        let (front, _) = split_front_matter(raw).unwrap();
        let p: PersonaFrontMatter = serde_yaml::from_str(front).unwrap();
        assert_eq!(p.display.as_deref(), Some("개발가재"));
        assert_eq!(p.model, "openai/gpt-4o-mini");
        assert_eq!(p.history_window, Some(30));
    }
}
