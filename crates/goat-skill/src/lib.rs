mod args;
mod manifest;
mod render;
mod scan;

use std::path::PathBuf;

use thiserror::Error;

pub use args::{Call, Resolved, resolve};
pub use manifest::{Argument, ArgumentValue, Choice};
pub use render::render;
pub use scan::{Diagnostic, Resource, Scope, Scopes, Skill, SkillSet, Survey, survey};

pub const PROJECT_SUBDIR: &str = ".goat/skills";

#[derive(Debug, Error)]
pub enum SkillError {
    #[error("failed to read a skill: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid YAML in {path}: {source}")]
    Yaml {
        path: PathBuf,
        source: serde_yaml::Error,
    },
    #[error("{0} has no `---` front matter")]
    MissingFrontMatter(PathBuf),
    #[error("skill `{name}` must live in a directory of the same name, not `{dir}`")]
    NameMismatch { name: String, dir: String },
    #[error("{path}: {field} {reason}")]
    Validation {
        path: PathBuf,
        field: &'static str,
        reason: String,
    },
    #[error("no skill named `{0}`")]
    NotFound(String),
    #[error("{0}")]
    InvalidArguments(String),
}
