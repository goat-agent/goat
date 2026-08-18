use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::SkillError;
use crate::manifest::{self, Argument};

const RESOURCE_DIRS: [&str; 3] = ["scripts", "references", "assets"];

const BUILTIN: &[(&str, &str)] = &[
    ("goat", include_str!("../builtin/goat/SKILL.md")),
    (
        "configuring-goat",
        include_str!("../builtin/configuring-goat/SKILL.md"),
    ),
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Scope {
    Builtin,
    AgentsUser,
    Common,
    Agent(String),
    Project,
}

impl Scope {
    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Self::Builtin => "builtin",
            Self::AgentsUser => "~/.agents",
            Self::Common => "common",
            Self::Agent(slug) => slug,
            Self::Project => "project",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub arguments: Vec<Argument>,
    pub body: String,
    pub dir: Option<PathBuf>,
    pub scope: Scope,
}

impl Skill {
    #[must_use]
    pub fn resources(&self) -> Vec<Resource> {
        let Some(dir) = &self.dir else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for kind in RESOURCE_DIRS {
            let Ok(entries) = std::fs::read_dir(dir.join(kind)) else {
                continue;
            };
            let mut found: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_file())
                .collect();
            found.sort();
            out.extend(found.into_iter().map(|path| Resource { kind, path }));
        }
        out
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Resource {
    pub kind: &'static str,
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub path: PathBuf,
    pub scope: Scope,
    pub message: String,
}

pub struct Scopes {
    root: PathBuf,
    agent: Option<String>,
    project: Option<PathBuf>,
    agents_user: Option<PathBuf>,
}

impl Scopes {
    #[must_use]
    pub fn agent(root: impl Into<PathBuf>, slug: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            agent: Some(slug.into()),
            project: None,
            agents_user: agents_user_dir(),
        }
    }

    #[must_use]
    pub fn code(root: impl Into<PathBuf>, cwd: &Path) -> Self {
        Self {
            root: root.into(),
            agent: None,
            project: Some(cwd.join(crate::PROJECT_SUBDIR)),
            agents_user: agents_user_dir(),
        }
    }

    #[must_use]
    pub fn with_agents_user(mut self, dir: Option<PathBuf>) -> Self {
        self.agents_user = dir;
        self
    }
}

#[derive(Clone, Debug, Default)]
pub struct SkillSet {
    skills: BTreeMap<String, Skill>,
    diagnostics: Vec<Diagnostic>,
}

impl SkillSet {
    #[must_use]
    pub fn load(scopes: &Scopes) -> Self {
        let mut set = Self::default();
        if scopes.agent.is_some() {
            set.absorb(builtin());
        }
        if let Some(dir) = &scopes.agents_user {
            set.absorb(set_scan(dir, &Scope::AgentsUser));
        }
        set.absorb(set_scan(&scopes.root.join("skills"), &Scope::Common));
        if let Some(slug) = &scopes.agent {
            let dir = scopes.root.join("agents").join(slug).join("skills");
            set.absorb(set_scan(&dir, &Scope::Agent(slug.clone())));
        }
        if let Some(dir) = &scopes.project {
            set.absorb(set_scan(dir, &Scope::Project));
        }
        set
    }

    fn absorb(&mut self, layer: Layer) {
        for skill in layer.skills {
            self.skills.insert(skill.name.clone(), skill);
        }
        self.diagnostics.extend(layer.diagnostics);
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }

    pub fn activate(&self, name: &str) -> Result<&Skill, SkillError> {
        self.get(name)
            .ok_or_else(|| SkillError::NotFound(name.to_owned()))
    }

    pub fn iter(&self) -> impl Iterator<Item = &Skill> {
        self.skills.values()
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    #[must_use]
    pub fn catalog(&self) -> Option<String> {
        crate::render::catalog(self.skills.values())
    }
}

pub struct Survey {
    pub skills: Vec<Skill>,
    pub diagnostics: Vec<Diagnostic>,
}

#[must_use]
pub fn survey(root: &Path) -> Survey {
    let mut layers = vec![(0u8, builtin())];
    if let Some(dir) = agents_user_dir() {
        layers.push((1, set_scan(&dir, &Scope::AgentsUser)));
    }
    layers.push((2, set_scan(&root.join("skills"), &Scope::Common)));
    for slug in agent_slugs(root) {
        let dir = root.join("agents").join(&slug).join("skills");
        layers.push((3, set_scan(&dir, &Scope::Agent(slug))));
    }
    let mut ranked = Vec::new();
    let mut diagnostics = Vec::new();
    for (precedence, layer) in layers {
        ranked.extend(layer.skills.into_iter().map(|skill| (precedence, skill)));
        diagnostics.extend(layer.diagnostics);
    }
    ranked.sort_by(|(left_precedence, left), (right_precedence, right)| {
        left.name
            .cmp(&right.name)
            .then_with(|| right_precedence.cmp(left_precedence))
            .then_with(|| left.scope.label().cmp(right.scope.label()))
    });
    Survey {
        skills: ranked.into_iter().map(|(_, skill)| skill).collect(),
        diagnostics,
    }
}

#[derive(Default)]
struct Layer {
    skills: Vec<Skill>,
    diagnostics: Vec<Diagnostic>,
}

fn builtin() -> Layer {
    let mut layer = Layer::default();
    for (name, text) in BUILTIN {
        let path = PathBuf::from("<builtin>").join(name).join("SKILL.md");
        match manifest::parse(text, &path, name) {
            Ok(parsed) => layer.skills.push(Skill {
                name: parsed.name,
                description: parsed.description,
                arguments: parsed.arguments,
                body: parsed.body,
                dir: None,
                scope: Scope::Builtin,
            }),
            Err(err) => layer.diagnostics.push(Diagnostic {
                path,
                scope: Scope::Builtin,
                message: err.to_string(),
            }),
        }
    }
    layer
}

fn set_scan(dir: &Path, scope: &Scope) -> Layer {
    let mut layer = Layer::default();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return layer;
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    for path in dirs {
        let manifest_path = path.join("SKILL.md");
        let Ok(text) = std::fs::read_to_string(&manifest_path) else {
            continue;
        };
        let Some(dir_name) = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
        else {
            continue;
        };
        match manifest::parse(&text, &manifest_path, &dir_name) {
            Ok(parsed) => layer.skills.push(Skill {
                name: parsed.name,
                description: parsed.description,
                arguments: parsed.arguments,
                body: parsed.body,
                dir: Some(path),
                scope: scope.clone(),
            }),
            Err(err) => {
                tracing::warn!(path = %manifest_path.display(), reason = %err, "skipping skill");
                layer.diagnostics.push(Diagnostic {
                    path: manifest_path,
                    scope: scope.clone(),
                    message: err.to_string(),
                });
            }
        }
    }
    layer
}

fn agent_slugs(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root.join("agents")) else {
        return Vec::new();
    };
    let mut slugs: Vec<String> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("agent.md").is_file())
        .filter_map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .collect();
    slugs.sort();
    slugs
}

fn agents_user_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".agents").join("skills"))
}

#[cfg(test)]
mod tests {
    use super::{Scope, Scopes, SkillSet, survey};
    use std::path::Path;

    fn write(dir: &Path, name: &str, text: &str) {
        let skill = dir.join(name);
        std::fs::create_dir_all(&skill).expect("a skill directory");
        std::fs::write(skill.join("SKILL.md"), text).expect("a manifest");
    }

    fn manifest(name: &str, description: &str) -> String {
        format!("---\nname: {name}\ndescription: {description}\n---\n\nbody of {name}\n")
    }

    fn tree(root: &Path) {
        std::fs::create_dir_all(root.join("agents/scout")).expect("an agent directory");
        std::fs::write(root.join("agents/scout/agent.md"), "scout").expect("an agent definition");
    }

    #[test]
    fn a_later_layer_shadows_an_earlier_one() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        tree(root);
        let agents_user = root.join("dot-agents");
        write(
            &agents_user,
            "review",
            &manifest("review", "from ~/.agents"),
        );
        write(
            &root.join("skills"),
            "review",
            &manifest("review", "common"),
        );
        write(
            &root.join("agents/scout/skills"),
            "review",
            &manifest("review", "the agent's own"),
        );

        let set = SkillSet::load(
            &Scopes::agent(root, "scout").with_agents_user(Some(agents_user.clone())),
        );
        let review = set.get("review").expect("review resolves");
        assert_eq!(review.description, "the agent's own");
        assert_eq!(review.scope, Scope::Agent("scout".to_owned()));

        let set = SkillSet::load(&Scopes::agent(root, "other").with_agents_user(Some(agents_user)));
        assert_eq!(
            set.get("review").expect("review resolves").description,
            "common",
            "an unrelated agent sees the common layer"
        );
    }

    #[test]
    fn a_user_skill_shadows_a_builtin() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        tree(root);
        write(&root.join("skills"), "goat", &manifest("goat", "mine"));
        let set = SkillSet::load(&Scopes::agent(root, "scout").with_agents_user(None));
        assert_eq!(set.get("goat").expect("goat resolves").description, "mine");
        assert!(
            set.get("configuring-goat").is_some(),
            "the other builtin survives"
        );
    }

    #[test]
    fn builtins_reach_the_agent_and_not_a_code_session() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        tree(root);
        let agent = SkillSet::load(&Scopes::agent(root, "scout").with_agents_user(None));
        assert!(agent.get("goat").is_some());
        let code = SkillSet::load(&Scopes::code(root, root).with_agents_user(None));
        assert!(
            code.get("goat").is_none(),
            "the builtins describe the resident agent, not a coding session"
        );
    }

    #[test]
    fn a_project_skill_shadows_the_common_one() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cwd = root.join("repo");
        write(&root.join("skills"), "build", &manifest("build", "common"));
        write(
            &cwd.join(crate::PROJECT_SUBDIR),
            "build",
            &manifest("build", "project"),
        );
        let set = SkillSet::load(&Scopes::code(root, &cwd).with_agents_user(None));
        assert_eq!(
            set.get("build").expect("build resolves").description,
            "project"
        );
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn a_malformed_skill_becomes_a_diagnostic() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        tree(root);
        write(
            &root.join("skills"),
            "broken",
            "---\nname: broken\n---\nbody",
        );
        let set = SkillSet::load(&Scopes::agent(root, "scout").with_agents_user(None));
        assert!(set.get("broken").is_none());
        assert_eq!(set.diagnostics().len(), 1);
        assert_eq!(set.diagnostics()[0].scope, Scope::Common);
    }

    #[test]
    fn resources_are_listed_one_level_deep() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("skills"), "audit", &manifest("audit", "d"));
        let skill_dir = root.join("skills/audit");
        std::fs::create_dir_all(skill_dir.join("scripts")).unwrap();
        std::fs::write(skill_dir.join("scripts/run.sh"), "#!/bin/sh").unwrap();
        std::fs::create_dir_all(skill_dir.join("scripts/nested")).unwrap();
        std::fs::write(skill_dir.join("scripts/nested/deep.sh"), "#!/bin/sh").unwrap();

        let set = SkillSet::load(&Scopes::code(root, root).with_agents_user(None));
        let resources = set.get("audit").expect("audit resolves").resources();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].kind, "scripts");
        assert!(resources[0].path.ends_with("run.sh"));
    }

    #[test]
    fn a_survey_reports_every_layer_including_shadowed_ones() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        tree(root);
        write(
            &root.join("skills"),
            "review",
            &manifest("review", "common"),
        );
        write(
            &root.join("agents/scout/skills"),
            "review",
            &manifest("review", "the agent's own"),
        );
        let surveyed = survey(root);
        let scopes: Vec<&str> = surveyed
            .skills
            .iter()
            .filter(|skill| skill.name == "review")
            .map(|skill| skill.scope.label())
            .collect();
        assert_eq!(scopes, vec!["scout", "common"]);
    }
}
