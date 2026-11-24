use std::path::Path;

use toml_edit::{Array, DocumentMut, InlineTable, Item, Table};

use super::{Decision, Digest, Error, Review, Server, Source, Transport, is_name, sign_ins};
use crate::store::Store;

pub fn add(store: &Store, file: &Path, name: &str, transport: &Transport) -> Result<(), Error> {
    if !is_name(name) {
        return Err(Error::BadName(name.to_owned()));
    }
    rewrite(store, file, |document| {
        if document.contains_key(name) {
            return Err(Error::Exists {
                name: name.to_owned(),
                path: file.to_owned(),
            });
        }
        document.insert(name, Item::Table(table(transport)));
        Ok(())
    })
}

pub fn remove(store: &Store, server: &Server) -> Result<(), Error> {
    let file = match &server.source {
        Source::File(file) => file,
        Source::Plugin(plugin) => {
            return Err(Error::FromPlugin {
                server: server.name.clone(),
                plugin: plugin.clone(),
            });
        }
    };
    rewrite(store, file, |document| {
        document
            .remove(&server.name)
            .map(drop)
            .ok_or_else(|| Error::Unknown(server.name.clone()))
    })?;
    let logins = store.logins()?;
    for account in sign_ins(&logins, server)? {
        super::sign_out(&logins, server, account.as_deref())?;
    }
    Ok(())
}

fn rewrite(
    store: &Store,
    file: &Path,
    change: impl FnOnce(&mut DocumentMut) -> Result<(), Error>,
) -> Result<(), Error> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(Error::Read {
                path: file.to_owned(),
                source,
            });
        }
    };
    let digest = Digest::of(text.as_bytes());
    let is_allowed = text.is_empty()
        || store.mcp_reviews()?.iter().any(|review| {
            review.path == file && review.digest == digest && review.decision == Decision::Allowed
        });
    let mut document: DocumentMut = text.parse().map_err(|source| Error::Parse {
        path: file.to_owned(),
        source,
    })?;
    change(&mut document)?;
    let written = document.to_string();
    let unwritten = |source| Error::Write {
        path: file.to_owned(),
        source,
    };
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(unwritten)?;
    }
    std::fs::write(file, &written).map_err(unwritten)?;
    if is_allowed {
        store.save_mcp_review(&Review {
            path: file.to_owned(),
            digest: Digest::of(written.as_bytes()),
            decision: Decision::Allowed,
        })?;
    }
    Ok(())
}

fn table(transport: &Transport) -> Table {
    let mut table = Table::new();
    match transport {
        Transport::Command { program, args, env } => {
            let command: Array = std::iter::once(program)
                .chain(args)
                .map(String::as_str)
                .collect();
            table.insert("command", toml_edit::value(command));
            if !env.is_empty() {
                table.insert("env", toml_edit::value(pairs(env)));
            }
        }
        Transport::Url {
            url,
            headers,
            client,
        } => {
            table.insert("url", toml_edit::value(url.as_str()));
            if !headers.is_empty() {
                table.insert("headers", toml_edit::value(pairs(headers)));
            }
            if let Some(client) = client {
                let mut oauth = InlineTable::new();
                oauth.insert("client_id", client.id.as_str().into());
                if let Some(secret) = &client.secret {
                    oauth.insert("client_secret", secret.as_str().into());
                }
                if let Some(port) = client.port {
                    oauth.insert("callback_port", i64::from(port).into());
                }
                table.insert("oauth", toml_edit::value(oauth));
            }
        }
        Transport::Builtin(builtin) => {
            table.insert("builtin", toml_edit::value(builtin.as_str()));
        }
    }
    table
}

fn pairs(pairs: &[(String, String)]) -> InlineTable {
    pairs
        .iter()
        .map(|(key, value)| (key.as_str(), toml_edit::Value::from(value.as_str())))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use std::path::PathBuf;

    use super::*;
    use crate::mcp::{OauthClient, config};

    fn command(program: &str) -> Transport {
        Transport::Command {
            program: program.to_owned(),
            args: vec!["--stdio".to_owned()],
            env: vec![("TOKEN".to_owned(), "$TOKEN".to_owned())],
        }
    }

    fn project(dir: &Path) -> (PathBuf, PathBuf) {
        let workspace = dir.join("work");
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(workspace.join(".git")).unwrap();
        (workspace, home)
    }

    #[test]
    fn an_added_server_keeps_the_rest_of_the_file_as_written() {
        let store = Store::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mcp.toml");
        std::fs::write(&file, "# mine\n[old]\nurl = \"https://old\"   # keep\n").unwrap();

        add(&store, &file, "new", &command("server")).unwrap();

        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "# mine\n[old]\nurl = \"https://old\"   # keep\n\n[new]\ncommand = [\"server\", \"--stdio\"]\nenv = { TOKEN = \"$TOKEN\" }\n"
        );
    }

    #[test]
    fn an_added_url_writes_its_oauth_client() {
        let store = Store::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mcp.toml");
        let transport = Transport::Url {
            url: "https://mcp.slack.com/mcp".to_owned(),
            headers: Vec::new(),
            client: Some(OauthClient {
                id: "123".to_owned(),
                secret: Some("$SECRET".to_owned()),
                port: Some(3118),
            }),
        };

        add(&store, &file, "slack", &transport).unwrap();

        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "[slack]\nurl = \"https://mcp.slack.com/mcp\"\noauth = { client_id = \"123\", client_secret = \"$SECRET\", callback_port = 3118 }\n"
        );
    }

    #[test]
    fn adding_a_name_that_is_taken_is_rejected() {
        let store = Store::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mcp.toml");
        add(&store, &file, "notes", &command("a")).unwrap();

        let err = add(&store, &file, "notes", &command("b")).unwrap_err();

        assert_matches!(err, Error::Exists { .. });
    }

    #[test]
    fn adding_to_an_allowed_project_file_keeps_it_allowed() {
        let store = Store::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (workspace, home) = project(dir.path());
        let file = crate::mcp::project_file(&workspace);
        add(&store, &file, "first", &command("a")).unwrap();

        add(&store, &file, "second", &command("b")).unwrap();

        let config = config(&workspace, &home, |_| None, &store.mcp_reviews().unwrap());
        assert_eq!(config.servers.len(), 2);
        assert_eq!(config.unreviewed, Vec::new());
    }

    #[test]
    fn adding_to_an_unreviewed_project_file_leaves_it_for_review() {
        let store = Store::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (workspace, home) = project(dir.path());
        let file = crate::mcp::project_file(&workspace);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "[cloned]\ncommand = [\"curl\"]\n").unwrap();

        add(&store, &file, "mine", &command("a")).unwrap();

        let config = config(&workspace, &home, |_| None, &store.mcp_reviews().unwrap());
        assert_eq!(config.servers, Vec::new());
        assert_eq!(config.unreviewed.len(), 1);
    }

    #[test]
    fn adding_a_name_outside_the_name_rule_is_rejected() {
        let store = Store::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();

        let err = add(&store, &dir.path().join("mcp.toml"), "a#b", &command("x")).unwrap_err();

        assert_matches!(err, Error::BadName(_));
    }

    #[test]
    fn removing_a_plugin_server_is_refused() {
        let store = Store::open_in_memory().unwrap();
        let server = Server {
            name: "gmail".to_owned(),
            source: Source::Plugin("gmail".to_owned()),
            transport: command("x"),
        };

        let err = remove(&store, &server).unwrap_err();

        assert_matches!(err, Error::FromPlugin { .. });
    }

    #[test]
    fn a_removed_server_loses_its_entry_and_every_sign_in() {
        let store = Store::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (workspace, home) = project(dir.path());
        let file = crate::mcp::personal_file(&home);
        let transport = Transport::Url {
            url: "https://notes".to_owned(),
            headers: Vec::new(),
            client: None,
        };
        add(&store, &file, "notes", &transport).unwrap();
        let logins = store.logins().unwrap();
        logins
            .save(&crate::mcp::login_key("https://notes", None), "{}")
            .unwrap();
        logins
            .save(&crate::mcp::login_key("https://notes", Some("work")), "{}")
            .unwrap();
        let server = config(&workspace, &home, |_| None, &[]).servers.remove(0);

        remove(&store, &server).unwrap();

        assert_eq!(config(&workspace, &home, |_| None, &[]).servers, Vec::new());
        assert_eq!(
            logins.keys("mcp:https://notes").unwrap(),
            Vec::<String>::new()
        );
    }
}
