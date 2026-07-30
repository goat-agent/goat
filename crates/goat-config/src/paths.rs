use std::path::PathBuf;

use anyhow::{Result, anyhow};

#[derive(Clone, Debug)]
pub struct GoatPaths {
    pub root: PathBuf,
    pub credentials_json: PathBuf,
    pub config_json: PathBuf,
    pub mcp_json: PathBuf,
    pub rate_limits_json: PathBuf,
    pub agents_dir: PathBuf,
    pub subagents_dir: PathBuf,
    pub memory_dir: PathBuf,
    pub skills_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub remote_dir: PathBuf,
    pub browser_dir: PathBuf,
    pub update_dir: PathBuf,
    pub bin_dir: PathBuf,
    pub socket_path: PathBuf,
    pub state_db: PathBuf,
}

impl GoatPaths {
    pub fn default_layout() -> Result<Self> {
        Ok(Self::from_root(home_root()?))
    }

    pub fn from_root(root: PathBuf) -> Self {
        Self {
            credentials_json: root.join("credentials.json"),
            config_json: root.join("config.json"),
            mcp_json: root.join("mcp.json"),
            rate_limits_json: root.join("rate_limits.json"),
            agents_dir: root.join("agents"),
            subagents_dir: root.join("subagents"),
            memory_dir: root.join("memory"),
            skills_dir: root.join("skills"),
            logs_dir: root.join("logs"),
            remote_dir: root.join("remote"),
            browser_dir: root.join("browser"),
            update_dir: root.join("update"),
            bin_dir: root.join("bin"),
            socket_path: root.join("daemon.sock"),
            state_db: root.join("goat.db"),
            root,
        }
    }

    pub fn agent_dir(&self, slug: &str) -> PathBuf {
        self.agents_dir.join(slug)
    }

    pub fn browser_profile_dir(&self) -> PathBuf {
        self.browser_dir.join("profile")
    }
}

fn home_root() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("$HOME is not set"))?;
    Ok(PathBuf::from(home).join(".goat"))
}

pub const HOME_NOT_FOUND: &str = "could not resolve ~/.goat";

fn resolved() -> Option<GoatPaths> {
    GoatPaths::default_layout().ok()
}

pub fn config_path() -> Option<PathBuf> {
    resolved().map(|p| p.config_json)
}

pub fn mcp_config_path() -> Option<PathBuf> {
    resolved().map(|p| p.mcp_json)
}

pub fn auth_path() -> Option<PathBuf> {
    resolved().map(|p| p.credentials_json)
}

pub fn log_dir() -> Option<PathBuf> {
    resolved().map(|p| p.logs_dir)
}

pub fn skills_dir() -> Option<PathBuf> {
    resolved().map(|p| p.skills_dir)
}

pub fn browser_dir() -> Option<PathBuf> {
    resolved().map(|p| p.browser_dir)
}

pub fn browser_profile_dir() -> Option<PathBuf> {
    resolved().map(|p| p.browser_profile_dir())
}

pub fn socket_path() -> Option<PathBuf> {
    resolved().map(|p| p.socket_path)
}

pub fn remote_dir() -> Option<PathBuf> {
    resolved().map(|p| p.remote_dir)
}

pub fn update_dir() -> Option<PathBuf> {
    resolved().map(|p| p.update_dir)
}

pub fn bin_dir() -> Option<PathBuf> {
    resolved().map(|p| p.bin_dir)
}

pub fn agents_dir() -> Option<PathBuf> {
    resolved().map(|p| p.agents_dir)
}

pub fn subagents_dir() -> Option<PathBuf> {
    resolved().map(|p| p.subagents_dir)
}

pub fn rate_limits_path() -> Option<PathBuf> {
    resolved().map(|p| p.rate_limits_json)
}

pub fn global_instructions_file() -> Option<PathBuf> {
    resolved().map(|p| p.root.join(PROJECT_INSTRUCTIONS_FILE))
}

pub const PROJECT_SKILLS_SUBDIR: &str = ".goat/skills";
pub const PROJECT_SUBAGENTS_SUBDIR: &str = ".goat/subagents";
pub const PROJECT_INSTRUCTIONS_FILE: &str = "AGENTS.md";
pub const PROJECT_INSTRUCTIONS_OVERRIDE_FILE: &str = "AGENTS.override.md";
pub const INSTRUCTIONS_MAX_BYTES: usize = 32 * 1024;
