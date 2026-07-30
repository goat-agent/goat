use std::fs;

use anyhow::Result;
use thiserror::Error;

mod agent;
mod paths;
mod settings;

pub use agent::AGENT_DEFINITION_FILE;
pub use paths::{
    GoatPaths, HOME_NOT_FOUND, INSTRUCTIONS_MAX_BYTES, PROJECT_INSTRUCTIONS_FILE,
    PROJECT_INSTRUCTIONS_OVERRIDE_FILE, PROJECT_SKILLS_SUBDIR, PROJECT_SUBAGENTS_SUBDIR,
    agents_dir, auth_path, bin_dir, browser_dir, browser_profile_dir, config_path,
    global_instructions_file, log_dir, mcp_config_path, rate_limits_path, remote_dir, skills_dir,
    socket_path, subagents_dir, update_dir,
};
pub use settings::{
    Config, RemoteConfig, SearchAccountConfig, SearchConfig, SettingsError, ThemeChoice,
    WebFetchConfig,
};

use goat_agent_config::AgentConfig;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid model in agent '{slug}': {source}")]
    Model {
        slug: String,
        #[source]
        source: goat_model::ModelError,
    },
    #[error("agent '{slug}' has no agent.md")]
    MissingDefinition { slug: String },
    #[error("agents dir not found: {0}")]
    MissingAgentsDir(std::path::PathBuf),
}

#[derive(Debug)]
pub struct LoadedConfig {
    pub paths: GoatPaths,
    pub agents: Vec<AgentConfig>,
}

pub fn load() -> Result<LoadedConfig> {
    load_from(GoatPaths::default_layout()?)
}

pub fn load_from(paths: GoatPaths) -> Result<LoadedConfig> {
    fs::create_dir_all(&paths.root).ok();
    fs::create_dir_all(&paths.logs_dir).ok();
    fs::create_dir_all(&paths.agents_dir).ok();
    fs::create_dir_all(&paths.skills_dir).ok();

    let agents = agent::scan_agents(&paths.agents_dir)?;

    Ok(LoadedConfig { paths, agents })
}
