use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ignore::WalkBuilder;

const FILE: &str = "SKILL.md";
const FILE_BYTES: u64 = 1024 * 1024;
const PER_ROOT: usize = 200;
const RESOURCES: usize = 50;
const RESOURCE_DEPTH: usize = 3;
const PLACEHOLDER: &str = "$ARGUMENTS";
const CATALOG: &str = "\
Skills hold instructions for specific tasks. When a task matches a skill's description, \
load it first with `await skill(name, args)` in a run and follow it; pass the user's arguments \
as `args` when there are any. Its instructions come after that run's result.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub hint: Option<String>,
    pub source: Source,
    pub for_model: bool,
    pub for_user: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    File(PathBuf),
    Plugin(Arc<str>),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Found {
    pub skills: Vec<Skill>,
    pub skipped: Vec<(PathBuf, String)>,
}

pub fn discover(workspace: &Path, home: &Path) -> Found {
    let root = workspace
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .unwrap_or(workspace);
    let project = workspace
        .ancestors()
        .take_while(|dir| dir.starts_with(root));
    let mut roots: Vec<PathBuf> = Vec::new();
    for dir in project.chain([home]) {
        for kind in [".goat", ".agents"] {
            let skills = dir.join(kind).join("skills");
            if !roots.contains(&skills) {
                roots.push(skills);
            }
        }
    }
    let mut found = Found::default();
    let mut names = HashSet::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path().join(FILE))
            .filter(|path| path.is_file())
            .collect();
        dirs.sort();
        for location in dirs.into_iter().take(PER_ROOT) {
            match parse(&location) {
                Ok(skill) if names.insert(skill.name.clone()) => found.skills.push(skill),
                Ok(_) => {}
                Err(problem) => found.skipped.push((location, problem)),
            }
        }
    }
    found
}

pub fn invoked<'a>(skills: &'a [Skill], text: &'a str) -> Option<(&'a Skill, &'a str)> {
    let rest = text.trim_start().strip_prefix('/')?;
    let (word, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let skill = skills
        .iter()
        .find(|skill| skill.for_user && skill.name == word)?;
    Some((skill, args.trim()))
}

pub(crate) fn catalog(skills: &[Skill]) -> Option<String> {
    let listed: Vec<&Skill> = skills.iter().filter(|skill| skill.for_model).collect();
    if listed.is_empty() {
        return None;
    }
    let mut text = format!("{CATALOG}\n\n<available_skills>\n");
    for skill in listed {
        write!(
            text,
            "<skill>\n<name>{}</name>\n<description>{}</description>\n</skill>\n",
            escape(&skill.name),
            escape(&skill.description)
        )
        .ok();
    }
    text.push_str("</available_skills>");
    Some(text)
}

pub(crate) fn activate(skill: &Skill, args: &str) -> Result<String, String> {
    let text = match &skill.source {
        Source::File(location) => read(location)?,
        Source::Plugin(text) => text.to_string(),
    };
    let body = split(&text).map_or(text.as_str(), |(_, body)| body).trim();
    let (body, is_used) = substitute(body, args);
    let mut out = format!("<skill_content name=\"{}\">\n{body}", escape(&skill.name));
    if !is_used && !args.is_empty() {
        write!(out, "\n\nARGUMENTS: {args}").ok();
    }
    if let Source::File(location) = &skill.source {
        let dir = location.parent().unwrap_or(location);
        write!(
            out,
            "\n\nSkill directory: {}\nRelative paths in this skill are relative to the skill directory.",
            dir.display()
        )
        .ok();
        let files = resources(dir);
        if !files.is_empty() {
            out.push_str("\n\n<skill_resources>\n");
            for file in files {
                writeln!(out, "<file>{}</file>", escape(&file)).ok();
            }
            out.push_str("</skill_resources>");
        }
    }
    out.push_str("\n</skill_content>");
    Ok(out)
}

pub(crate) fn activated(text: &str) -> impl Iterator<Item = &str> {
    text.match_indices("<skill_content name=\"")
        .filter_map(|(at, open)| {
            let rest = &text[at + open.len()..];
            rest.find('"').map(|end| &rest[..end])
        })
}

fn parse(location: &Path) -> Result<Skill, String> {
    let text = read(location)?;
    let folder = location
        .parent()
        .and_then(Path::file_name)
        .map_or_default(|name| name.to_string_lossy().into_owned());
    let skill = from_text(&text, folder, Source::File(location.to_owned()))?;
    if skill.name.contains(':') {
        return Err("its name has a colon, which only plugin skills use".to_owned());
    }
    Ok(skill)
}

pub(crate) fn from_plugin(plugin: &str, folder: &str, text: &str) -> Result<Skill, String> {
    let mut skill = from_text(text, folder.to_owned(), Source::Plugin(text.into()))?;
    skill.name = format!("{plugin}:{}", skill.name);
    Ok(skill)
}

fn from_text(text: &str, folder: String, source: Source) -> Result<Skill, String> {
    let (front, _) = split(text).ok_or("it has no frontmatter between --- lines")?;
    let fields = fields(front);
    let field = |key: &str| {
        fields
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.trim())
            .filter(|value| !value.is_empty())
    };
    let description = field("description").ok_or("it has no description")?;
    let flag = |key: &str, default: bool| {
        field(key).map_or(default, |value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "true" | "yes" | "on" | "1"
            )
        })
    };
    Ok(Skill {
        name: field("name").map_or(folder, str::to_owned),
        description: description.to_owned(),
        hint: field("argument-hint").map(str::to_owned),
        source,
        for_model: !flag("disable-model-invocation", false),
        for_user: flag("user-invocable", true),
    })
}

fn read(location: &Path) -> Result<String, String> {
    let file = std::fs::File::open(location)
        .map_err(|err| format!("could not read {}: {err}", location.display()))?;
    let mut bytes = Vec::new();
    file.take(FILE_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|err| format!("could not read {}: {err}", location.display()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn split(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start_matches('\u{feff}');
    let rest = text.strip_prefix("---")?;
    let rest = rest
        .strip_prefix('\n')
        .or_else(|| rest.strip_prefix("\r\n"))?;
    let mut at = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return Some((&rest[..at], &rest[at + line.len()..]));
        }
        at += line.len();
    }
    None
}

fn fields(front: &str) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    let mut lines = front.lines().peekable();
    while let Some(line) = lines.next() {
        if line.starts_with([' ', '\t', '#', '-']) {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        let block: Option<&str> = match value {
            ">" | ">-" | ">+" => Some(" "),
            "|" | "|-" | "|+" => Some("\n"),
            _ => None,
        };
        let value = if let Some(join) = block {
            let mut parts = Vec::new();
            while let Some(next) =
                lines.next_if(|next| next.is_empty() || next.starts_with([' ', '\t']))
            {
                parts.push(next.trim());
            }
            parts.join(join).trim().to_owned()
        } else {
            scalar(value)
        };
        fields.push((key.trim().to_owned(), value));
    }
    fields
}

fn scalar(value: &str) -> String {
    if let Some(inner) = value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        return inner.replace("\\\"", "\"").replace("\\\\", "\\");
    }
    if let Some(inner) = value
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    {
        return inner.replace("''", "'");
    }
    match value.find(" #") {
        Some(comment) => value[..comment].trim_end().to_owned(),
        None => value.to_owned(),
    }
}

fn substitute(body: &str, args: &str) -> (String, bool) {
    let mut out = String::with_capacity(body.len() + args.len());
    let mut is_used = false;
    let mut rest = body;
    while let Some(at) = rest.find(PLACEHOLDER) {
        let before = &rest[..at];
        let is_escaped = before.ends_with('\\') && !before.ends_with("\\\\");
        if is_escaped {
            out.push_str(&before[..before.len() - 1]);
            out.push_str(PLACEHOLDER);
        } else {
            out.push_str(before);
            out.push_str(args);
            is_used = true;
        }
        rest = &rest[at + PLACEHOLDER.len()..];
    }
    out.push_str(rest);
    (out, is_used)
}

fn resources(dir: &Path) -> Vec<String> {
    let mut files: Vec<String> = WalkBuilder::new(dir)
        .max_depth(Some(RESOURCE_DEPTH))
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .filter_map(|entry| {
            let relative = entry.path().strip_prefix(dir).ok()?;
            (relative != Path::new(FILE)).then(|| relative.display().to_string())
        })
        .take(RESOURCES)
        .collect();
    files.sort();
    files
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill_at(root: &Path, name: &str, front: &str, body: &str) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE);
        std::fs::write(&path, format!("---\n{front}\n---\n{body}")).unwrap();
        path
    }

    #[test]
    fn frontmatter_is_read_leniently() {
        let dir = tempfile::tempdir().unwrap();
        let path = skill_at(
            dir.path(),
            "commit",
            "name: commit\ndescription: Use when: the user asks to commit # note\nargument-hint: \"[push|pr|merge]\"\nmetadata:\n  author: me\nuser-invocable: yes",
            "Body",
        );

        let skill = parse(&path).unwrap();

        assert_eq!(skill.description, "Use when: the user asks to commit");
        assert_eq!(skill.hint.as_deref(), Some("[push|pr|merge]"));
        assert!(skill.for_model && skill.for_user);
    }

    #[test]
    fn a_folded_description_and_a_missing_name_fall_back_sensibly() {
        let dir = tempfile::tempdir().unwrap();
        let path = skill_at(
            dir.path(),
            "pdf",
            "description: >\n  Extract text\n  from PDFs.\ndisable-model-invocation: true",
            "",
        );

        let skill = parse(&path).unwrap();

        assert_eq!(skill.name, "pdf");
        assert_eq!(skill.description, "Extract text from PDFs.");
        assert!(!skill.for_model);
    }

    #[test]
    fn a_disk_skill_with_a_colon_in_its_name_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = skill_at(dir.path(), "mail", "name: gmail:api\ndescription: x", "");

        let err = parse(&path).unwrap_err();

        assert_eq!(err, "its name has a colon, which only plugin skills use");
    }

    #[test]
    fn a_plugin_skill_is_prefixed_and_activates_without_a_directory() {
        let skill = from_plugin(
            "gmail",
            "api",
            "---\ndescription: Use Gmail\n---\nSearch first.",
        )
        .unwrap();

        let text = activate(&skill, "").unwrap();

        assert_eq!(skill.name, "gmail:api");
        assert_eq!(
            text,
            "<skill_content name=\"gmail:api\">\nSearch first.\n</skill_content>"
        );
    }

    #[test]
    fn a_skill_without_a_description_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = skill_at(dir.path(), "empty", "name: empty", "");

        assert_eq!(parse(&path).unwrap_err(), "it has no description");
    }

    #[test]
    fn project_skills_win_over_personal_ones_and_goat_over_agents() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let workspace = dir.path().join("work");
        skill_at(
            &home.join(".agents/skills"),
            "review",
            "description: personal",
            "",
        );
        skill_at(
            &workspace.join(".agents/skills"),
            "review",
            "description: project",
            "",
        );
        skill_at(
            &workspace.join(".agents/skills"),
            "deploy",
            "description: agents",
            "",
        );
        skill_at(
            &workspace.join(".goat/skills"),
            "deploy",
            "description: goat",
            "",
        );
        skill_at(&home.join(".goat/skills"), "notes", "name: notes", "");

        let found = discover(&workspace, &home);

        let described: Vec<(&str, &str)> = found
            .skills
            .iter()
            .map(|skill| (skill.name.as_str(), skill.description.as_str()))
            .collect();
        assert_eq!(described, [("deploy", "goat"), ("review", "project")]);
        assert_eq!(found.skipped.len(), 1);
    }

    #[test]
    fn arguments_fill_the_placeholder_or_follow_the_body() {
        let dir = tempfile::tempdir().unwrap();
        let fix = skill_at(
            dir.path(),
            "fix",
            "description: d",
            "Fix issue $ARGUMENTS, not \\$ARGUMENTS.",
        );
        let plain = skill_at(dir.path(), "plain", "description: d", "Do it.");
        std::fs::create_dir_all(dir.path().join("plain/scripts")).unwrap();
        std::fs::write(dir.path().join("plain/scripts/run.sh"), "").unwrap();

        let fixed = activate(&parse(&fix).unwrap(), "42").unwrap();
        let done = activate(&parse(&plain).unwrap(), "fast").unwrap();

        assert!(fixed.starts_with(
            "<skill_content name=\"fix\">\nFix issue 42, not $ARGUMENTS.\n\nSkill directory: "
        ));
        assert!(!fixed.contains("ARGUMENTS: 42"));
        assert!(done.contains("Do it.\n\nARGUMENTS: fast\n\nSkill directory: "));
        assert!(
            done.contains("<skill_resources>\n<file>scripts/run.sh</file>\n</skill_resources>")
        );
        assert_eq!(activated(&done).collect::<Vec<_>>(), ["plain"]);
    }

    #[test]
    fn the_catalog_lists_only_skills_the_model_may_load() {
        let skill = |name: &str, for_model| Skill {
            name: name.to_owned(),
            description: "a <b> & c".to_owned(),
            hint: None,
            source: Source::File(PathBuf::new()),
            for_model,
            for_user: true,
        };

        let text = catalog(&[skill("open", true), skill("hidden", false)]).unwrap();

        assert!(text.contains("<name>open</name>\n<description>a &lt;b&gt; &amp; c</description>"));
        assert!(!text.contains("hidden"));
        assert_eq!(catalog(&[skill("hidden", false)]), None);
    }

    #[test]
    fn a_slash_command_invokes_a_user_skill() {
        let skill = Skill {
            name: "commit".to_owned(),
            description: String::new(),
            hint: None,
            source: Source::File(PathBuf::new()),
            for_model: true,
            for_user: true,
        };
        let skills = [skill];

        assert_eq!(
            invoked(&skills, "/commit merge now").map(|(_, args)| args),
            Some("merge now")
        );
        assert!(invoked(&skills, "/commits").is_none());
        assert!(invoked(&skills, "please /commit").is_none());
    }
}
