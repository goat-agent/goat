use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use goat_protocol::{
    SkillBranchInfo, SkillChoiceInfo, SkillCommandShape, SkillInfo, SkillParameterInfo,
    SkillParameterValue,
};
use serde::Deserialize;

pub struct Skill {
    pub name: String,
    pub description: String,
    pub command: Option<SkillCommandShape>,
    pub dir: PathBuf,
    pub body: String,
}

pub struct SkillSet {
    skills: BTreeMap<String, Skill>,
}

impl SkillSet {
    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Skill> {
        self.skills.values()
    }

    pub fn len(&self) -> usize {
        self.skills.len()
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }
}

pub fn load(cwd: &Path) -> SkillSet {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(global) = goat_config::skills_dir() {
        dirs.push(global);
    }
    dirs.push(cwd.join(goat_config::PROJECT_SKILLS_SUBDIR));
    load_from_dirs(&dirs)
}

pub fn load_from_dirs(dirs: &[PathBuf]) -> SkillSet {
    let mut skills: BTreeMap<String, Skill> = BTreeMap::new();
    for dir in dirs {
        load_dir(dir, &mut skills);
    }
    SkillSet { skills }
}

impl From<&Skill> for SkillInfo {
    fn from(skill: &Skill) -> Self {
        Self {
            name: skill.name.clone(),
            description: skill.description.clone(),
            command: skill.command.clone(),
        }
    }
}

fn load_dir(dir: &Path, out: &mut BTreeMap<String, Skill>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest = path.join("SKILL.md");
        let Ok(content) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        let dir_name = entry.file_name().to_string_lossy().into_owned();
        match parse(&content, &dir_name) {
            Ok(parsed) => {
                out.insert(
                    parsed.name.clone(),
                    Skill {
                        name: parsed.name,
                        description: parsed.description,
                        command: parsed.command,
                        dir: path,
                        body: parsed.body,
                    },
                );
            }
            Err(reason) => {
                tracing::warn!(path = %manifest.display(), reason, "skipping skill");
            }
        }
    }
}

struct Parsed {
    name: String,
    description: String,
    command: Option<SkillCommandShape>,
    body: String,
}

#[derive(Deserialize)]
struct Manifest {
    name: Option<String>,
    description: String,
    #[serde(default)]
    arguments: Option<Vec<ManifestParameter>>,
    #[serde(default)]
    subcommands: Option<Vec<ManifestBranch>>,
}

#[derive(Deserialize)]
struct ManifestBranch {
    name: String,
    description: String,
    #[serde(default)]
    arguments: Vec<ManifestParameter>,
}

#[derive(Deserialize)]
struct ManifestParameter {
    name: String,
    description: String,
    #[serde(default)]
    required: bool,
    value: ManifestValue,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ManifestValue {
    Kind(ManifestValueKind),
    Choice { choice: Vec<ManifestChoice> },
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ManifestValueKind {
    Word,
    Integer,
    TextTail,
}

#[derive(Deserialize)]
struct ManifestChoice {
    value: String,
    #[serde(default)]
    description: Option<String>,
}

fn parse(content: &str, dir_name: &str) -> Result<Parsed, String> {
    let content = content.trim_start_matches('\u{feff}');
    let Some(rest) = content.strip_prefix("---") else {
        return Err("missing frontmatter".to_owned());
    };
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let Some((frontmatter, body)) = rest.split_once("\n---") else {
        return Err("unterminated frontmatter".to_owned());
    };
    let manifest: Manifest = serde_yaml::from_str(frontmatter).map_err(|err| err.to_string())?;
    let name = manifest
        .name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| dir_name.to_owned());
    let description = (!manifest.description.trim().is_empty())
        .then_some(manifest.description)
        .ok_or_else(|| "missing description".to_owned())?;
    let command = command_shape(manifest.arguments, manifest.subcommands)?;
    validate_skill_name(&name, "skill")?;
    if let Some(command) = &command {
        validate_command(command)?;
    }
    let body = body.strip_prefix("\n").unwrap_or(body).trim().to_owned();
    Ok(Parsed {
        name,
        description,
        command,
        body,
    })
}

fn command_shape(
    arguments: Option<Vec<ManifestParameter>>,
    subcommands: Option<Vec<ManifestBranch>>,
) -> Result<Option<SkillCommandShape>, String> {
    match (arguments, subcommands) {
        (Some(_), Some(_)) => Err("arguments and subcommands cannot both be present".to_owned()),
        (Some(arguments), None) => Ok(Some(SkillCommandShape::Arguments {
            items: arguments.into_iter().map(parameter_info).collect(),
        })),
        (None, Some(subcommands)) => Ok(Some(SkillCommandShape::Subcommands {
            items: subcommands.into_iter().map(branch_info).collect(),
        })),
        (None, None) => Ok(None),
    }
}

fn branch_info(branch: ManifestBranch) -> SkillBranchInfo {
    SkillBranchInfo {
        name: branch.name,
        description: branch.description,
        arguments: branch.arguments.into_iter().map(parameter_info).collect(),
    }
}

fn parameter_info(parameter: ManifestParameter) -> SkillParameterInfo {
    SkillParameterInfo {
        name: parameter.name,
        description: parameter.description,
        required: parameter.required,
        value: value_info(parameter.value),
    }
}

fn value_info(value: ManifestValue) -> SkillParameterValue {
    match value {
        ManifestValue::Kind(ManifestValueKind::Word) => SkillParameterValue::Word {},
        ManifestValue::Kind(ManifestValueKind::Integer) => SkillParameterValue::Integer {},
        ManifestValue::Kind(ManifestValueKind::TextTail) => SkillParameterValue::TextTail {},
        ManifestValue::Choice { choice } => SkillParameterValue::Choice {
            options: choice
                .into_iter()
                .map(|choice| SkillChoiceInfo {
                    value: choice.value,
                    description: choice.description,
                })
                .collect(),
        },
    }
}

fn validate_command(command: &SkillCommandShape) -> Result<(), String> {
    match command {
        SkillCommandShape::Arguments { items: arguments } => validate_parameters(arguments),
        SkillCommandShape::Subcommands { items: subcommands } => {
            if subcommands.is_empty() {
                return Err("subcommands cannot be empty".to_owned());
            }
            let mut names = std::collections::BTreeSet::new();
            for subcommand in subcommands {
                validate_skill_name(&subcommand.name, "subcommand")?;
                if !names.insert(subcommand.name.as_str()) {
                    return Err(format!("duplicate subcommand: {}", subcommand.name));
                }
                if subcommand.description.trim().is_empty() {
                    return Err(format!(
                        "subcommand {} description cannot be empty",
                        subcommand.name
                    ));
                }
                validate_parameters(&subcommand.arguments)?;
            }
            Ok(())
        }
    }
}

fn validate_parameters(arguments: &[SkillParameterInfo]) -> Result<(), String> {
    let mut names = std::collections::BTreeSet::new();
    let mut optional_seen = false;
    let mut text_tail_seen = false;
    for (index, argument) in arguments.iter().enumerate() {
        validate_skill_name(&argument.name, "argument")?;
        if !names.insert(argument.name.as_str()) {
            return Err(format!("duplicate argument: {}", argument.name));
        }
        if argument.description.trim().is_empty() {
            return Err(format!(
                "argument {} description cannot be empty",
                argument.name
            ));
        }
        if !argument.required {
            optional_seen = true;
        } else if optional_seen {
            return Err("required argument cannot follow optional argument".to_owned());
        }
        if matches!(argument.value, SkillParameterValue::TextTail {}) {
            if text_tail_seen {
                return Err("text_tail cannot appear more than once".to_owned());
            }
            text_tail_seen = true;
            if index + 1 != arguments.len() {
                return Err("text_tail must be last".to_owned());
            }
        }
        if let SkillParameterValue::Choice { options: choices } = &argument.value {
            if choices.is_empty() {
                return Err(format!("choice argument {} cannot be empty", argument.name));
            }
            let mut values = std::collections::BTreeSet::new();
            for choice in choices {
                if choice.value.trim().is_empty() {
                    return Err("choice value cannot be empty".to_owned());
                }
                if !values.insert(choice.value.as_str()) {
                    return Err(format!("duplicate choice: {}", choice.value));
                }
            }
        }
    }
    Ok(())
}

fn validate_skill_name(name: &str, kind: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err(format!("{kind} name cannot be empty"));
    }
    if !name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(format!("{kind} name is not slash-safe: {name}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{load_from_dirs, parse};
    use goat_protocol::{SkillCommandShape, SkillParameterValue};

    fn write_skill(dir: &std::path::Path, name: &str, contents: &str) {
        let skill_dir = dir.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), contents).unwrap();
    }

    #[test]
    fn parses_frontmatter_and_body() {
        let parsed = parse(
            "---\nname: greet\ndescription: Say hello\n---\n\nDo the greeting.\n",
            "greet-dir",
        )
        .unwrap();
        assert_eq!(parsed.name, "greet");
        assert_eq!(parsed.description, "Say hello");
        assert_eq!(parsed.command, None);
        assert_eq!(parsed.body, "Do the greeting.");
    }

    #[test]
    fn name_falls_back_to_dir() {
        let parsed = parse("---\ndescription: x\n---\nbody", "from-dir").unwrap();
        assert_eq!(parsed.name, "from-dir");
    }

    #[test]
    fn missing_description_is_error() {
        assert!(parse("---\nname: x\n---\nbody", "d").is_err());
    }

    #[test]
    fn missing_frontmatter_is_error() {
        assert!(parse("no frontmatter here", "d").is_err());
    }

    #[test]
    fn parses_arguments_schema() {
        let parsed = parse(
            "---\nname: review\ndescription: Review code\narguments:\n  - name: instructions\n    description: What to focus on\n    required: false\n    value: text_tail\n---\nbody",
            "review",
        )
        .unwrap();
        let Some(SkillCommandShape::Arguments { items: arguments }) = parsed.command else {
            panic!("expected arguments");
        };
        assert_eq!(arguments[0].name, "instructions");
        assert_eq!(arguments[0].value, SkillParameterValue::TextTail {});
    }

    #[test]
    fn parses_subcommands_schema() {
        let parsed = parse(
            "---\nname: review\ndescription: Review code\nsubcommands:\n  - name: security\n    description: Security review\n    arguments:\n      - name: target\n        description: File or directory\n        required: true\n        value: word\n---\nbody",
            "review",
        )
        .unwrap();
        let Some(SkillCommandShape::Subcommands { items: subcommands }) = parsed.command else {
            panic!("expected subcommands");
        };
        assert_eq!(subcommands[0].name, "security");
    }

    #[test]
    fn both_arguments_and_subcommands_is_error() {
        assert!(
            parse(
                "---\ndescription: x\narguments: []\nsubcommands: []\n---\nbody",
                "d"
            )
            .is_err()
        );
    }

    #[test]
    fn scans_directory() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(dir.path(), "alpha", "---\ndescription: A\n---\nalpha body");
        write_skill(dir.path(), "beta", "---\ndescription: B\n---\nbeta body");
        let set = load_from_dirs(&[dir.path().to_path_buf()]);
        assert_eq!(set.len(), 2);
        assert_eq!(set.get("alpha").unwrap().body, "alpha body");
        assert_eq!(set.get("beta").unwrap().description, "B");
    }

    #[test]
    fn project_overrides_global() {
        let global = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        write_skill(
            global.path(),
            "shared",
            "---\ndescription: global\n---\nglobal body",
        );
        write_skill(
            project.path(),
            "shared",
            "---\ndescription: project\n---\nproject body",
        );
        let set = load_from_dirs(&[global.path().to_path_buf(), project.path().to_path_buf()]);
        assert_eq!(set.len(), 1);
        assert_eq!(set.get("shared").unwrap().body, "project body");
        assert_eq!(set.get("shared").unwrap().description, "project");
    }
}
