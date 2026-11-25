use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::catalog::Model;
use crate::frontmatter::{self, read, split};
use crate::mcp;

pub const GENERAL: &str = "general";
const FOLDER: &str = "agents";
const EXTENSION: &str = "md";
const PER_ROOT: usize = 200;
const BUILTIN: &[&str] = &[include_str!("../agents/general.md")];
const CATALOG: &str = "\
Agents do a task in a context of their own and return only their final answer, so their steps never enter this conversation. \
Start one in a run with `await agent(task)`, or `await agent(task, {as: name})` for one listed here, and run several at once with Promise.all. \
An agent sees only the task text, so put everything it needs in it.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub name: String,
    pub description: String,
    pub model: Option<Box<str>>,
    pub effort: Option<Box<str>>,
    pub source: Source,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Builtin(&'static str),
    File(PathBuf),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Found {
    pub agents: Vec<Agent>,
    pub skipped: Vec<(PathBuf, String)>,
}

impl Agent {
    pub(crate) fn prompt(&self) -> Result<String, String> {
        let text = match &self.source {
            Source::Builtin(text) => (*text).to_owned(),
            Source::File(location) => read(location)?,
        };
        Ok(split(&text)
            .map_or(text.as_str(), |(_, body)| body)
            .trim()
            .to_owned())
    }
}

impl Found {
    pub(crate) fn find(&self, name: &str) -> Option<&Agent> {
        self.agents.iter().find(|agent| agent.name == name)
    }
}

pub fn discover(workspace: &Path, home: &Path, catalog: &[Model]) -> Found {
    let root = workspace
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .unwrap_or(workspace);
    let mut roots: Vec<PathBuf> = workspace
        .ancestors()
        .take_while(|dir| dir.starts_with(root))
        .map(|dir| dir.join(".goat").join(FOLDER))
        .collect();
    let personal = home.join(".goat").join(FOLDER);
    if !roots.contains(&personal) {
        roots.push(personal);
    }
    let mut found = Found::default();
    let mut names = HashSet::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == EXTENSION))
            .collect();
        files.sort();
        for location in files.into_iter().take(PER_ROOT) {
            let parsed = read(&location).and_then(|text| {
                let stem = location
                    .file_stem()
                    .map_or_default(|stem| stem.to_string_lossy().into_owned());
                parse(&text, stem, Source::File(location.clone()), catalog)
            });
            match parsed {
                Ok(agent) if names.insert(agent.name.clone()) => found.agents.push(agent),
                Ok(_) => {}
                Err(problem) => found.skipped.push((location, problem)),
            }
        }
    }
    for text in BUILTIN {
        match parse(text, String::new(), Source::Builtin(text), catalog) {
            Ok(agent) if names.insert(agent.name.clone()) => found.agents.push(agent),
            Ok(_) => {}
            Err(problem) => unreachable!("built-in agents are valid: {problem}"),
        }
    }
    found
}

pub(crate) fn catalog(agents: &[Agent]) -> String {
    let mut text = format!("{CATALOG}\n\n<available_agents>\n");
    for agent in agents {
        write!(
            text,
            "<agent>\n<name>{}</name>\n<description>{}</description>\n</agent>\n",
            escape(&agent.name),
            escape(&agent.description)
        )
        .ok();
    }
    text.push_str("</available_agents>");
    text
}

fn parse(text: &str, stem: String, source: Source, catalog: &[Model]) -> Result<Agent, String> {
    let (front, _) = split(text).ok_or("it has no frontmatter between --- lines")?;
    let fields = frontmatter::fields(front);
    let field = |key: &str| {
        fields
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.trim())
            .filter(|value| !value.is_empty())
    };
    let name = field("name").map_or(stem, str::to_owned);
    if !mcp::is_name(&name) {
        return Err(format!(
            "{name:?} is not an agent name; names use letters, digits, - and _"
        ));
    }
    let description = field("description").ok_or("it has no description")?;
    let (model, effort) = match field("model") {
        Some(written) => {
            let (id, effort) = match written.split_once(':') {
                Some((id, effort)) => (id, Some(effort)),
                None => (written, None),
            };
            let known = catalog
                .iter()
                .find(|model| &*model.id == id)
                .ok_or_else(|| format!("goat does not know the model {id}"))?;
            if let Some(effort) = effort
                && !known.accepts(effort)
            {
                return Err(format!("{id} has no effort level {effort}"));
            }
            (Some(id.into()), effort.map(Into::into))
        }
        None => (None, None),
    };
    Ok(Agent {
        name,
        description: description.to_owned(),
        model,
        effort,
        source,
    })
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog;

    fn agent_at(dir: &Path, file: &str, text: &str) -> PathBuf {
        let folder = dir.join(".goat").join(FOLDER);
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join(file);
        std::fs::write(&path, text).unwrap();
        path
    }

    fn models() -> Vec<Model> {
        catalog::models().unwrap()
    }

    #[test]
    fn general_is_built_in_and_uses_the_session_model() {
        let dir = tempfile::tempdir().unwrap();

        let found = discover(dir.path(), dir.path(), &models());

        let general = found.find(GENERAL).unwrap();
        assert_eq!(
            (general.model.as_deref(), &general.source),
            (None, &Source::Builtin(BUILTIN[0]))
        );
        assert!(general.prompt().unwrap().starts_with("Do the whole task"));
    }

    #[test]
    fn a_file_with_the_same_name_replaces_general() {
        let dir = tempfile::tempdir().unwrap();
        agent_at(
            dir.path(),
            "general.md",
            "---\nname: general\ndescription: Mine\nmodel: claude-sonnet-5-5\n---\nBe brief.\n",
        );

        let found = discover(dir.path(), dir.path(), &models());

        let general = found.find(GENERAL).unwrap();
        assert_eq!(
            (
                general.description.as_str(),
                general.model.as_deref(),
                general.prompt().unwrap()
            ),
            ("Mine", Some("claude-sonnet-5-5"), "Be brief.".to_owned())
        );
        assert_eq!(found.agents.len(), 1);
    }

    #[test]
    fn the_file_name_names_an_agent_without_a_name_field() {
        let dir = tempfile::tempdir().unwrap();
        agent_at(
            dir.path(),
            "scout.md",
            "---\ndescription: Searches\nmodel: gpt-6-luna:low\n---\nSearch only.\n",
        );

        let found = discover(dir.path(), dir.path(), &models());

        let scout = found.find("scout").unwrap();
        assert_eq!(
            (scout.model.as_deref(), scout.effort.as_deref()),
            (Some("gpt-6-luna"), Some("low"))
        );
    }

    #[test]
    fn a_model_goat_does_not_know_skips_the_agent() {
        let dir = tempfile::tempdir().unwrap();
        let path = agent_at(
            dir.path(),
            "scout.md",
            "---\ndescription: Searches\nmodel: haiku\n---\n",
        );

        let found = discover(dir.path(), dir.path(), &models());

        assert_eq!(found.find("scout"), None);
        assert_eq!(
            found.skipped,
            [(path, "goat does not know the model haiku".to_owned())]
        );
    }

    #[test]
    fn a_project_agent_wins_over_a_personal_one() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir(project.path().join(".git")).unwrap();
        agent_at(home.path(), "scout.md", "---\ndescription: Personal\n---\n");
        agent_at(
            project.path(),
            "scout.md",
            "---\ndescription: Project\n---\n",
        );

        let found = discover(project.path(), home.path(), &models());

        assert_eq!(found.find("scout").unwrap().description, "Project");
    }

    #[test]
    fn the_catalog_lists_names_and_descriptions() {
        let dir = tempfile::tempdir().unwrap();

        let text = catalog(&discover(dir.path(), dir.path(), &models()).agents);

        assert!(text.contains("<name>general</name>"));
        assert!(text.contains("`await agent(task, {as: name})`"));
    }
}
