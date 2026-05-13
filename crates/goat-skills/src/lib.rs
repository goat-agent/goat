use std::collections::HashMap;
use std::path::{Path, PathBuf};

use goat_types::PersonaId;
use serde::Deserialize;
use thiserror::Error;
use tracing::warn;

const MAX_NAME_LEN: usize = 64;
const MAX_DESCRIPTION_LEN: usize = 1024;
const RESOURCE_DIRS: [&str; 3] = ["scripts", "references", "assets"];

#[derive(Debug, Error)]
pub enum SkillError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("yaml in {path}: {source}")]
    Yaml {
        path: PathBuf,
        source: serde_yaml::Error,
    },
    #[error("SKILL.md missing front-matter at {0}")]
    MissingFrontMatter(PathBuf),
    #[error("skill `{name}`: name must match parent dir `{dir}`")]
    NameMismatch { name: String, dir: String },
    #[error("skill at {path}: {kind} ({reason})")]
    Validation {
        path: PathBuf,
        kind: &'static str,
        reason: String,
    },
    #[error("skill not found: {0}")]
    NotFound(String),
}

#[derive(Debug, Deserialize)]
struct SkillFrontMatter {
    name: String,
    description: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SkillScope {
    AgentsUser,
    Common,
    Persona { persona: PersonaId, slug: String },
}

impl SkillScope {
    pub fn label(&self) -> &str {
        match self {
            SkillScope::AgentsUser => "~/.agents",
            SkillScope::Common => "common",
            SkillScope::Persona { slug, .. } => slug,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SkillEntry {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub scope: SkillScope,
}

#[derive(Clone, Debug)]
pub struct SkillDiagnostic {
    pub path: PathBuf,
    pub scope: SkillScope,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillResource {
    pub kind: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ActivatedSkill {
    pub name: String,
    pub body: String,
    pub skill_dir: PathBuf,
    pub resources: Vec<SkillResource>,
}

#[derive(Clone, Debug, Default)]
pub struct SkillIndex {
    agents_user: Vec<SkillEntry>,
    common: Vec<SkillEntry>,
    by_persona: HashMap<PersonaId, Vec<SkillEntry>>,
    diagnostics: Vec<SkillDiagnostic>,
}

impl SkillIndex {
    pub fn discover_root(root: &Path) -> Self {
        let personas = persona_pairs(root);
        Self::discover_with_agents_dir(root, &personas, default_agents_skills_dir().as_deref())
    }

    pub fn discover(root: &Path, personas: &[(PersonaId, String)]) -> Self {
        Self::discover_with_agents_dir(root, personas, default_agents_skills_dir().as_deref())
    }

    pub fn discover_with_agents_dir(
        root: &Path,
        personas: &[(PersonaId, String)],
        agents_dir: Option<&Path>,
    ) -> Self {
        let mut diagnostics = Vec::new();
        let mut by_persona = HashMap::new();
        for (persona, slug) in personas {
            let scope = SkillScope::Persona {
                persona: *persona,
                slug: slug.clone(),
            };
            let dir = root.join("personas").join(slug).join("skills");
            let entries = scan_dir(&dir, scope, &mut diagnostics);
            if !entries.is_empty() {
                by_persona.insert(*persona, entries);
            }
        }
        let agents_user = agents_dir
            .map(|dir| scan_dir(dir, SkillScope::AgentsUser, &mut diagnostics))
            .unwrap_or_default();
        let common = scan_dir(&root.join("skills"), SkillScope::Common, &mut diagnostics);
        Self {
            agents_user,
            common,
            by_persona,
            diagnostics,
        }
    }

    pub fn system_prompt_block(&self, persona: PersonaId) -> Option<String> {
        let entries = self.effective_entries(persona);
        if entries.is_empty() {
            return None;
        }
        let mut s = String::from(
            "The following skills provide specialized instructions for specific tasks.\n\
When a task matches a skill's description, call the `skill` tool with the skill name before proceeding.\n\
Do not load skill resources eagerly; use listed resource paths only when needed.\n\
<available_skills>\n",
        );
        for e in entries {
            s.push_str("  <skill>\n");
            s.push_str(&format!("    <name>{}</name>\n", xml_escape(&e.name)));
            s.push_str(&format!(
                "    <description>{}</description>\n",
                xml_escape(&e.description)
            ));
            s.push_str("  </skill>\n");
        }
        s.push_str("</available_skills>");
        Some(s)
    }

    pub fn activate(&self, persona: PersonaId, name: &str) -> Result<ActivatedSkill, SkillError> {
        let entry = self
            .effective_entries(persona)
            .into_iter()
            .find(|e| e.name == name)
            .ok_or_else(|| SkillError::NotFound(name.to_string()))?;
        let raw = std::fs::read_to_string(&entry.path)?;
        let body = strip_front_matter(&raw);
        let skill_dir = entry
            .path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| entry.path.clone());
        let resources = list_resources(&skill_dir);
        Ok(ActivatedSkill {
            name: entry.name.clone(),
            body,
            skill_dir,
            resources,
        })
    }

    pub fn body(&self, persona: PersonaId, name: &str) -> Option<String> {
        self.activate(persona, name).ok().map(|skill| skill.body)
    }

    pub fn common(&self) -> &[SkillEntry] {
        &self.common
    }

    pub fn agents_user(&self) -> &[SkillEntry] {
        &self.agents_user
    }

    pub fn for_persona(&self, persona: PersonaId) -> &[SkillEntry] {
        self.by_persona
            .get(&persona)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn diagnostics(&self) -> &[SkillDiagnostic] {
        &self.diagnostics
    }

    pub fn all_entries(&self) -> Vec<&SkillEntry> {
        let mut out: Vec<&SkillEntry> = Vec::new();
        out.extend(self.agents_user.iter());
        out.extend(self.common.iter());
        for entries in self.by_persona.values() {
            out.extend(entries.iter());
        }
        out.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.scope.label().cmp(b.scope.label()))
        });
        out
    }

    pub fn effective_entries(&self, persona: PersonaId) -> Vec<&SkillEntry> {
        let mut merged: HashMap<&str, &SkillEntry> = HashMap::new();
        for e in &self.agents_user {
            merged.insert(&e.name, e);
        }
        for e in &self.common {
            merged.insert(&e.name, e);
        }
        if let Some(persona_entries) = self.by_persona.get(&persona) {
            for e in persona_entries {
                merged.insert(&e.name, e);
            }
        }
        let mut entries: Vec<&SkillEntry> = merged.into_values().collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        entries
    }
}

pub fn format_activated_skill(skill: &ActivatedSkill, args: Option<&str>) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "<skill_content name=\"{}\">\n",
        escape_attr(&skill.name)
    ));
    let body = substitute_arguments(skill.body.trim(), args);
    out.push_str(&body);
    out.push_str("\n\nSkill directory: ");
    out.push_str(&skill.skill_dir.display().to_string());
    out.push_str("\nRelative paths in this skill are relative to the skill directory.\n");
    if !skill.resources.is_empty() {
        out.push_str("<skill_resources>\n");
        for resource in &skill.resources {
            out.push_str(&format!(
                "  <file kind=\"{}\">{}</file>\n",
                escape_attr(&resource.kind),
                escape_text(&resource.path.to_string_lossy())
            ));
        }
        out.push_str("</skill_resources>\n");
    }
    out.push_str("</skill_content>");
    out
}

pub fn substitute_arguments(content: &str, args: Option<&str>) -> String {
    let Some(args) = args else {
        return content.to_string();
    };
    let parsed = parse_arguments(args);
    let content = replace_argument_indexes(content, &parsed);
    let content = replace_shorthand_indexes(&content, &parsed);
    content.replace("$ARGUMENTS", args)
}

fn parse_arguments(args: &str) -> Vec<String> {
    let mut parsed = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;

    for ch in args.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => current.push(ch),
            None if ch == '\'' || ch == '"' => quote = Some(ch),
            None if ch.is_whitespace() => {
                if !current.is_empty() {
                    parsed.push(std::mem::take(&mut current));
                }
            }
            None => current.push(ch),
        }
    }
    if escaped {
        current.push('\\');
    }
    if !current.is_empty() {
        parsed.push(current);
    }
    parsed
}

fn replace_argument_indexes(content: &str, parsed: &[String]) -> String {
    let mut out = String::new();
    let mut rest = content;
    while let Some(start) = rest.find("$ARGUMENTS[") {
        out.push_str(&rest[..start]);
        let after_prefix = &rest[start + "$ARGUMENTS[".len()..];
        let Some(end) = after_prefix.find(']') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let index_text = &after_prefix[..end];
        if !index_text.is_empty() && index_text.chars().all(|c| c.is_ascii_digit()) {
            let value = index_text
                .parse::<usize>()
                .ok()
                .and_then(|i| parsed.get(i))
                .map(String::as_str)
                .unwrap_or("");
            out.push_str(value);
            rest = &after_prefix[end + 1..];
        } else {
            out.push_str("$ARGUMENTS[");
            rest = after_prefix;
        }
    }
    out.push_str(rest);
    out
}

fn replace_shorthand_indexes(content: &str, parsed: &[String]) -> String {
    let chars: Vec<char> = content.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit() {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            if j == chars.len() || !is_word_char(chars[j]) {
                let index_text: String = chars[i + 1..j].iter().collect();
                let value = index_text
                    .parse::<usize>()
                    .ok()
                    .and_then(|idx| parsed.get(idx))
                    .map(String::as_str)
                    .unwrap_or("");
                out.push_str(value);
                i = j;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn is_word_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn escape_attr(s: &str) -> String {
    escape_text(s).replace('"', "&quot;")
}

fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn default_agents_skills_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".agents").join("skills"))
}

fn persona_pairs(root: &Path) -> Vec<(PersonaId, String)> {
    let personas_dir = root.join("personas");
    let mut out = Vec::new();
    let Ok(read) = std::fs::read_dir(personas_dir) else {
        return out;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if !path.is_dir() || !path.join("persona.md").exists() {
            continue;
        }
        if let Some(slug) = path.file_name().and_then(|s| s.to_str()) {
            out.push((PersonaId::from_slug(slug), slug.to_string()));
        }
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

fn scan_dir(
    dir: &Path,
    scope: SkillScope,
    diagnostics: &mut Vec<SkillDiagnostic>,
) -> Vec<SkillEntry> {
    if !dir.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) => {
            warn!(dir = %dir.display(), error = ?e, "scanning skills failed");
            diagnostics.push(SkillDiagnostic {
                path: dir.to_path_buf(),
                scope,
                message: format!("scanning failed: {e}"),
            });
            return out;
        }
    };
    for entry in read.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let dir_name = match path.file_name().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let skill_md = path.join("SKILL.md");
        if !skill_md.exists() {
            continue;
        }
        match load_skill(&skill_md, &dir_name, scope.clone()) {
            Ok(e) => out.push(e),
            Err(e) => {
                warn!(skill = %dir_name, error = ?e, "skipping skill");
                diagnostics.push(SkillDiagnostic {
                    path: skill_md,
                    scope: scope.clone(),
                    message: e.to_string(),
                });
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn load_skill(path: &Path, dir_name: &str, scope: SkillScope) -> Result<SkillEntry, SkillError> {
    let raw = std::fs::read_to_string(path)?;
    let (front, _body) = split_front_matter(&raw)
        .ok_or_else(|| SkillError::MissingFrontMatter(path.to_path_buf()))?;
    let fm: SkillFrontMatter = serde_yaml::from_str(front).map_err(|source| SkillError::Yaml {
        path: path.to_path_buf(),
        source,
    })?;
    validate_name(path, &fm.name)?;
    if fm.name != dir_name {
        return Err(SkillError::NameMismatch {
            name: fm.name,
            dir: dir_name.to_string(),
        });
    }
    if fm.description.is_empty() || fm.description.len() > MAX_DESCRIPTION_LEN {
        return Err(SkillError::Validation {
            path: path.to_path_buf(),
            kind: "description",
            reason: format!("must be 1..={MAX_DESCRIPTION_LEN}"),
        });
    }
    Ok(SkillEntry {
        name: fm.name,
        description: fm.description,
        path: path.to_path_buf(),
        scope,
    })
}

fn validate_name(path: &Path, name: &str) -> Result<(), SkillError> {
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        return Err(SkillError::Validation {
            path: path.to_path_buf(),
            kind: "name",
            reason: format!("must be 1..={MAX_NAME_LEN}"),
        });
    }
    let ok = name
        .chars()
        .all(|c| matches!(c, 'a'..='z' | '0'..='9' | '-'));
    if !ok {
        return Err(SkillError::Validation {
            path: path.to_path_buf(),
            kind: "name",
            reason: "only [a-z0-9-]".into(),
        });
    }
    if name.starts_with('-') || name.ends_with('-') || name.contains("--") {
        return Err(SkillError::Validation {
            path: path.to_path_buf(),
            kind: "name",
            reason: "no leading/trailing/consecutive hyphens".into(),
        });
    }
    Ok(())
}

fn list_resources(skill_dir: &Path) -> Vec<SkillResource> {
    let mut out = Vec::new();
    for kind in RESOURCE_DIRS {
        let dir = skill_dir.join(kind);
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if let Ok(rel) = path.strip_prefix(skill_dir) {
                out.push(SkillResource {
                    kind: kind.to_string(),
                    path: rel.to_path_buf(),
                });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

fn split_front_matter(s: &str) -> Option<(&str, &str)> {
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

fn strip_front_matter(s: &str) -> String {
    split_front_matter(s)
        .map(|(_, body)| body.to_string())
        .unwrap_or_else(|| s.to_string())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "goat-skills-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn name_validation_accepts_canonical() {
        let p = Path::new("/x");
        assert!(validate_name(p, "pdf-processing").is_ok());
        assert!(validate_name(p, "rust-format").is_ok());
    }

    #[test]
    fn name_validation_rejects() {
        let p = Path::new("/x");
        assert!(validate_name(p, "").is_err());
        assert!(validate_name(p, "-leading").is_err());
        assert!(validate_name(p, "trailing-").is_err());
        assert!(validate_name(p, "two--hyphens").is_err());
        assert!(validate_name(p, "UPPER").is_err());
        assert!(validate_name(p, "with space").is_err());
    }

    #[test]
    fn split_picks_up_yaml_block() {
        let raw = "---\nname: x\ndescription: hi\n---\n\nBody.";
        let (front, body) = split_front_matter(raw).unwrap();
        assert!(front.contains("name: x"));
        assert_eq!(body.trim(), "Body.");
    }

    #[test]
    fn discovery_precedence_prefers_persona_then_common_then_agents() {
        let root = temp_root("precedence");
        let agents = root.join("agents-skills");
        let persona = PersonaId::from_slug("dev");
        let personas = vec![(persona, "dev".to_string())];
        write(
            &agents.join("plan/SKILL.md"),
            "---\nname: plan\ndescription: agents\n---\nagents",
        );
        write(
            &root.join("skills/plan/SKILL.md"),
            "---\nname: plan\ndescription: common\n---\ncommon",
        );
        write(
            &root.join("personas/dev/skills/plan/SKILL.md"),
            "---\nname: plan\ndescription: persona\n---\npersona",
        );

        let idx = SkillIndex::discover_with_agents_dir(&root, &personas, Some(&agents));
        assert_eq!(
            idx.activate(persona, "plan").unwrap().body.trim(),
            "persona"
        );
        let other = PersonaId::from_slug("other");
        assert_eq!(idx.activate(other, "plan").unwrap().body.trim(), "common");
    }

    #[test]
    fn activation_lists_one_level_resources() {
        let root = temp_root("resources");
        write(
            &root.join("skills/analyze/SKILL.md"),
            "---\nname: analyze\ndescription: analyze stuff\n---\nbody",
        );
        write(&root.join("skills/analyze/scripts/run.sh"), "#!/bin/sh");
        write(&root.join("skills/analyze/references/guide.md"), "guide");
        write(&root.join("skills/analyze/assets/template.txt"), "template");
        write(&root.join("skills/analyze/references/deep/skip.md"), "skip");

        let idx = SkillIndex::discover_with_agents_dir(&root, &[], None);
        let skill = idx
            .activate(PersonaId::from_slug("dev"), "analyze")
            .unwrap();
        let paths: Vec<_> = skill
            .resources
            .into_iter()
            .map(|r| r.path.to_string_lossy().to_string())
            .collect();
        assert_eq!(
            paths,
            vec![
                "assets/template.txt",
                "references/guide.md",
                "scripts/run.sh"
            ]
        );
    }

    #[test]
    fn substitutes_arguments() {
        let content = "raw=$ARGUMENTS sub=$ARGUMENTS[0] task=$1 time=$2 missing=$9";
        let out = substitute_arguments(content, Some("add \"보고서 작성\" '내일 9시'"));
        assert_eq!(
            out,
            "raw=add \"보고서 작성\" '내일 9시' sub=add task=보고서 작성 time=내일 9시 missing="
        );
    }

    #[test]
    fn malformed_skills_are_reported() {
        let root = temp_root("diagnostics");
        write(&root.join("skills/bad/SKILL.md"), "no frontmatter");
        let idx = SkillIndex::discover_with_agents_dir(&root, &[], None);
        assert!(idx.common().is_empty());
        assert_eq!(idx.diagnostics().len(), 1);
    }
}
