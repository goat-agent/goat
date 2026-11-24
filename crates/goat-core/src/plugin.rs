use serde::Deserialize;

use crate::mcp::{self, Server, Source};
use crate::skill::{self, Skill};
use crate::store::{self, Logins};

const MANIFEST: &str = "plugin.toml";
const SERVERS: &str = "mcp.toml";
const SKILL: &str = "SKILL.md";
const CATALOG: &str = "\
Plugins that are off: {plugins}. \
When one would help, ask the user to run `goat plugin enable <name>` and start a new session.";
const BUILTIN: &[&[(&str, &str)]] = &[
    &[
        (MANIFEST, include_str!("../plugins/gmail/plugin.toml")),
        (SERVERS, include_str!("../plugins/gmail/mcp.toml")),
    ],
    &[
        (
            MANIFEST,
            include_str!("../plugins/google_calendar/plugin.toml"),
        ),
        (SERVERS, include_str!("../plugins/google_calendar/mcp.toml")),
    ],
    &[
        (
            MANIFEST,
            include_str!("../plugins/google_drive/plugin.toml"),
        ),
        (SERVERS, include_str!("../plugins/google_drive/mcp.toml")),
    ],
    &[
        (MANIFEST, include_str!("../plugins/google_docs/plugin.toml")),
        (SERVERS, include_str!("../plugins/google_docs/mcp.toml")),
    ],
    &[
        (
            MANIFEST,
            include_str!("../plugins/google_sheets/plugin.toml"),
        ),
        (SERVERS, include_str!("../plugins/google_sheets/mcp.toml")),
    ],
    &[
        (
            MANIFEST,
            include_str!("../plugins/google_slides/plugin.toml"),
        ),
        (SERVERS, include_str!("../plugins/google_slides/mcp.toml")),
    ],
    &[
        (
            MANIFEST,
            include_str!("../plugins/google_contacts/plugin.toml"),
        ),
        (SERVERS, include_str!("../plugins/google_contacts/mcp.toml")),
    ],
    &[
        (MANIFEST, include_str!("../plugins/computer/plugin.toml")),
        (SERVERS, include_str!("../plugins/computer/mcp.toml")),
        (
            "skills/use/SKILL.md",
            include_str!("../plugins/computer/skills/use/SKILL.md"),
        ),
    ],
    &[
        (MANIFEST, include_str!("../plugins/apple/plugin.toml")),
        (SERVERS, include_str!("../plugins/apple/mcp.toml")),
        (
            "skills/use/SKILL.md",
            include_str!("../plugins/apple/skills/use/SKILL.md"),
        ),
    ],
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plugin {
    pub name: String,
    pub description: String,
    pub servers: Vec<Server>,
    pub skills: Vec<Skill>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the plugin has no {MANIFEST}")]
    NoManifest,
    #[error("{MANIFEST}: {0}")]
    Manifest(String),
    #[error("{0} is not a plugin name; names use letters, digits, - and _")]
    BadName(String),
    #[error("{plugin}: {problem}")]
    Part { plugin: String, problem: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    name: String,
    description: String,
}

impl Plugin {
    pub fn load(files: &[(&str, &str)]) -> Result<Self, Error> {
        let manifest = files
            .iter()
            .find(|(path, _)| *path == MANIFEST)
            .ok_or(Error::NoManifest)?;
        let Manifest { name, description } =
            toml::from_str(manifest.1).map_err(|err| Error::Manifest(err.message().to_owned()))?;
        if !mcp::is_name(&name) {
            return Err(Error::BadName(name));
        }
        let broken = |problem: String| Error::Part {
            plugin: name.clone(),
            problem,
        };
        let mut problems = Vec::new();
        let servers = files
            .iter()
            .find(|(path, _)| *path == SERVERS)
            .map_or_else(Vec::new, |(_, text)| {
                mcp::parse(text, &Source::Plugin(name.clone()), |_| None, &mut problems)
            });
        if let Some(problem) = problems.into_iter().next() {
            return Err(broken(problem));
        }
        let skills = files
            .iter()
            .filter_map(|(path, text)| {
                let folder = path.strip_prefix("skills/")?.strip_suffix(SKILL)?;
                Some((folder.strip_suffix('/')?, text))
            })
            .map(|(folder, text)| skill::from_plugin(&name, folder, text).map_err(&broken))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            name,
            description,
            servers,
            skills,
        })
    }

    pub fn needs_sign_in(&self) -> bool {
        self.servers.iter().any(|server| match &server.transport {
            mcp::Transport::Url { .. } => true,
            mcp::Transport::Builtin(builtin) => builtin.needs_sign_in(),
            mcp::Transport::Command { .. } => false,
        })
    }

    pub fn is_on(&self, setting: Option<bool>, logins: &Logins) -> Result<bool, store::Error> {
        if !setting.unwrap_or_else(|| self.needs_sign_in()) {
            return Ok(false);
        }
        Ok(!self.needs_sign_in() || self.is_signed_in(logins)?)
    }

    pub fn forget_grants(&self, logins: &Logins) -> Result<(), store::Error> {
        for server in &self.servers {
            if let mcp::Transport::Builtin(builtin) = &server.transport {
                builtin.forget_grants(logins)?;
            }
        }
        Ok(())
    }

    pub fn missing_permissions(&self, ask: bool) -> Result<Vec<&'static str>, String> {
        let mut missing = Vec::new();
        for server in &self.servers {
            if let mcp::Transport::Builtin(builtin) = &server.transport {
                for permission in builtin.missing_permissions(ask)? {
                    if !missing.contains(&permission) {
                        missing.push(permission);
                    }
                }
            }
        }
        Ok(missing)
    }

    pub fn is_signed_in(&self, logins: &Logins) -> Result<bool, store::Error> {
        for server in &self.servers {
            if !mcp::sign_ins(logins, server)?.is_empty() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn remotes(&self) -> impl Iterator<Item = &Server> {
        self.servers
            .iter()
            .filter(|server| match &server.transport {
                mcp::Transport::Url { .. } => true,
                mcp::Transport::Builtin(builtin) => builtin.needs_sign_in(),
                mcp::Transport::Command { .. } => false,
            })
    }

    pub fn add_servers(&self, servers: &mut Vec<Server>) {
        for server in &self.servers {
            if !servers.iter().any(|known| known.name == server.name) {
                servers.push(server.clone());
            }
        }
    }

    pub(crate) fn join(&self, skills: &mut Vec<Skill>, servers: &mut Vec<Server>) {
        skills.extend(self.skills.iter().cloned());
        self.add_servers(servers);
    }
}

pub fn builtin() -> Result<Vec<Plugin>, Error> {
    BUILTIN.iter().map(|files| Plugin::load(files)).collect()
}

pub(crate) fn catalog(waiting: &[&Plugin]) -> Option<String> {
    if waiting.is_empty() {
        return None;
    }
    let listed: Vec<String> = waiting
        .iter()
        .map(|plugin| format!("{} ({})", plugin.name, plugin.description))
        .collect();
    Some(CATALOG.replace("{plugins}", &listed.join(", ")))
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use super::*;

    const NOTES: &[(&str, &str)] = &[
        (
            MANIFEST,
            "name = \"notes\"\ndescription = \"Read and write notes\"\n",
        ),
        (SERVERS, "[notes]\nurl = \"https://notes.example/mcp\"\n"),
        (
            "skills/guide/SKILL.md",
            "---\ndescription: How to use notes\n---\nSearch first.",
        ),
    ];

    #[test]
    fn every_builtin_plugin_loads() {
        builtin().unwrap();
    }

    #[test]
    fn a_plugin_brings_its_servers_and_prefixed_skills() {
        let plugin = Plugin::load(NOTES).unwrap();

        assert_eq!(plugin.name, "notes");
        assert_eq!(plugin.servers[0].name, "notes");
        assert_eq!(plugin.servers[0].source, Source::Plugin("notes".to_owned()));
        assert_eq!(plugin.skills[0].name, "notes:guide");
        assert_eq!(plugin.skills[0].description, "How to use notes");
    }

    #[test]
    fn a_plugin_is_signed_in_once_any_account_of_its_server_is() {
        let store = store::Store::open_in_memory().unwrap();
        let logins = store.logins().unwrap();
        let plugin = Plugin::load(NOTES).unwrap();
        assert!(!plugin.is_signed_in(&logins).unwrap());

        logins
            .save("mcp:https://notes.example/mcp#work", "{}")
            .unwrap();

        assert!(plugin.is_signed_in(&logins).unwrap());
    }

    #[test]
    fn a_plugin_that_signs_in_is_on_once_signed_in_unless_disabled() {
        let store = store::Store::open_in_memory().unwrap();
        let logins = store.logins().unwrap();
        let plugin = Plugin::load(NOTES).unwrap();
        logins.save("mcp:https://notes.example/mcp", "{}").unwrap();

        assert!(plugin.is_on(None, &logins).unwrap());
        assert!(!plugin.is_on(Some(false), &logins).unwrap());
    }

    #[test]
    fn a_plugin_without_sign_in_is_off_until_enabled() {
        let store = store::Store::open_in_memory().unwrap();
        let logins = store.logins().unwrap();
        let plugin = Plugin::load(&[
            (
                MANIFEST,
                "name = \"local\"\ndescription = \"Local tools\"\n",
            ),
            (SERVERS, "[local]\ncommand = [\"local-mcp\"]\n"),
        ])
        .unwrap();

        assert!(!plugin.is_on(None, &logins).unwrap());
        assert!(plugin.is_on(Some(true), &logins).unwrap());
    }

    #[test]
    fn a_user_server_with_the_same_name_wins_over_the_plugin() {
        let plugin = Plugin::load(NOTES).unwrap();
        let mine = Server {
            name: "notes".to_owned(),
            source: Source::File("mcp.toml".into()),
            transport: mcp::Transport::Command {
                program: "my-notes".to_owned(),
                args: Vec::new(),
                env: Vec::new(),
            },
        };
        let mut servers = vec![mine.clone()];
        let mut skills = Vec::new();

        plugin.join(&mut skills, &mut servers);

        assert_eq!(servers, [mine]);
        assert_eq!(skills.len(), 1);
    }

    #[test]
    fn a_plugin_with_a_bad_name_is_rejected() {
        let err = Plugin::load(&[(MANIFEST, "name = \"a.b\"\ndescription = \"x\"\n")]).unwrap_err();

        assert_matches!(err, Error::BadName(_));
    }

    #[test]
    fn plugins_waiting_for_sign_in_are_named_with_how_to_sign_in() {
        let plugin = Plugin::load(NOTES).unwrap();

        let text = catalog(&[&plugin]).unwrap();

        assert!(text.starts_with("Plugins that are off: notes (Read and write notes)."));
        assert!(text.contains("goat plugin enable <name>"));
    }
}
