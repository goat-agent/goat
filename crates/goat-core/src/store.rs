use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

use crate::account::{self, Account, Credential};
use crate::image::{self, Image, Picture};
use crate::mcp;
use crate::message::{self, Message, Origin, Part, Role};
use crate::session;

const MIGRATIONS: &[&str] = &[
    "
    create table sessions (
        id blob primary key,
        cwd text not null,
        instructions text not null,
        created_at integer not null
    ) strict;

    create table messages (
        id blob primary key,
        session_id blob not null references sessions on delete cascade,
        role text not null,
        model text,
        account_id blob,
        parts text not null
    ) strict;

    create index messages_by_session on messages (session_id, id);

    create table accounts (
        id blob primary key,
        provider text not null,
        name text not null,
        credential text not null,
        unique (provider, name)
    ) strict;
",
    "
    create table images (
        id text primary key,
        data blob not null
    ) strict;
",
    "
    alter table sessions add column title text;
",
    "
    create table changes (
        id blob primary key,
        session_id blob not null references sessions on delete cascade,
        path text not null,
        before blob
    ) strict;

    create index changes_by_path on changes (session_id, path, id);
",
    "
    drop table changes;

    create table snapshots (
        message_id blob primary key references messages on delete cascade,
        tree text not null
    ) strict;
",
    "
    create table mcp_reviews (
        path text primary key,
        digest blob not null,
        decision text not null
    ) strict;

    create table logins (
        key text primary key,
        credentials text not null
    ) strict;
",
    "
    create table plugins (
        name text primary key,
        enabled integer not null
    ) strict;
",
    "
    alter table sessions add column spawned_by blob references messages on delete cascade;
",
];

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
static MEMORY: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("create the database file")]
    Create(#[source] std::io::Error),
    #[error("database")]
    Sqlite(#[from] rusqlite::Error),
    #[error("stored json is malformed")]
    Json(#[from] serde_json::Error),
    #[error("stored {0} is not recognized")]
    Unknown(&'static str),
}

#[derive(Debug)]
pub struct Store {
    conn: Connection,
    location: PathBuf,
}

#[derive(Debug)]
pub struct Logins {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recent {
    pub id: session::Id,
    pub title: String,
    pub messages: u64,
    pub active_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    pub id: message::Id,
    pub text: String,
    pub images: Vec<Image>,
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: session::Id,
    pub cwd: PathBuf,
    pub instructions: Box<str>,
    pub created_at: i64,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, Error> {
        create_private(path).map_err(Error::Create)?;
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "wal")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        let store = Self {
            conn,
            location: path.to_owned(),
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self, Error> {
        let number = MEMORY.fetch_add(1, Ordering::Relaxed);
        let location = PathBuf::from(format!(
            "file:goat-{}-{number}?mode=memory&cache=shared",
            std::process::id()
        ));
        let conn = Connection::open(&location)?;
        conn.pragma_update(None, "foreign_keys", true)?;
        let store = Self { conn, location };
        store.migrate()?;
        Ok(store)
    }

    pub(crate) fn connect(location: &Path) -> Result<Self, Error> {
        let conn = Connection::open(location)?;
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        Ok(Self {
            conn,
            location: location.to_owned(),
        })
    }

    pub(crate) fn location(&self) -> &Path {
        &self.location
    }

    pub fn logins(&self) -> Result<Logins, Error> {
        let conn = Connection::open(&self.location)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        Ok(Logins {
            conn: Mutex::new(conn),
        })
    }

    fn migrate(&self) -> Result<(), Error> {
        let version: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        for (number, sql) in (1..)
            .zip(MIGRATIONS)
            .skip_while(|(number, _)| *number <= version)
        {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", number)?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn create_session(&self, cwd: &Path, instructions: &str) -> Result<session::Id, Error> {
        let id = session::Id::generate();
        self.conn.execute(
            "insert into sessions (id, cwd, instructions, created_at) values (?1, ?2, ?3, ?4)",
            params![id, cwd.to_string_lossy(), instructions, now()],
        )?;
        Ok(id)
    }

    pub(crate) fn create_agent_session(
        &self,
        cwd: &Path,
        instructions: &str,
        spawned_by: message::Id,
    ) -> Result<session::Id, Error> {
        let id = session::Id::generate();
        self.conn.execute(
            "insert into sessions (id, cwd, instructions, created_at, spawned_by) values (?1, ?2, ?3, ?4, ?5)",
            params![id, cwd.to_string_lossy(), instructions, now(), spawned_by],
        )?;
        Ok(id)
    }

    pub(crate) fn is_agent_of(
        &self,
        agent: session::Id,
        parent: session::Id,
    ) -> Result<bool, Error> {
        Ok(self
            .conn
            .query_row(
                "select 1 from sessions s join messages m on m.id = s.spawned_by
                 where s.id = ?1 and m.session_id = ?2",
                params![agent, parent],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn session(&self, id: session::Id) -> Result<Option<SessionInfo>, Error> {
        Ok(self
            .conn
            .query_row(
                "select id, cwd, instructions, created_at from sessions where id = ?1",
                [id],
                session_info,
            )
            .optional()?)
    }

    pub fn latest_session(&self, cwd: &Path) -> Result<Option<SessionInfo>, Error> {
        Ok(self
            .conn
            .query_row(
                "select id, cwd, instructions, created_at from sessions where cwd = ?1 and spawned_by is null order by id desc limit 1",
                [cwd.to_string_lossy()],
                session_info,
            )
            .optional()?)
    }

    pub fn append(&self, session: session::Id, message: &Message) -> Result<(), Error> {
        self.conn.execute(
            "insert into messages (id, session_id, role, model, account_id, parts) values (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                message.id,
                session,
                message.role.as_str(),
                message.origin.as_ref().map(|o| &*o.model),
                message.origin.as_ref().map(|o| o.account),
                serde_json::to_string(&message.parts)?,
            ],
        )?;
        Ok(())
    }

    pub fn put_image(&self, picture: &Picture) -> Result<(), Error> {
        self.conn.execute(
            "insert or ignore into images (id, data) values (?1, ?2)",
            params![picture.image.id.as_str(), picture.bytes],
        )?;
        Ok(())
    }

    pub fn image(&self, id: &image::Id) -> Result<Option<Vec<u8>>, Error> {
        Ok(self
            .conn
            .query_row(
                "select data from images where id = ?1",
                [id.as_str()],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn context(&self, session: session::Id) -> Result<Vec<Message>, Error> {
        let summary: Option<(message::Id, String)> = self
            .conn
            .query_row(
                "select id, parts from messages where session_id = ?1 and role = 'summary' order by id desc limit 1",
                [session],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((summary_id, parts)) = summary else {
            return self.messages(session, None);
        };
        let tail = serde_json::from_str::<Vec<Part>>(&parts)?
            .iter()
            .find_map(|part| match part {
                Part::Summary { tail, .. } => Some(*tail),
                _ => None,
            })
            .unwrap_or(summary_id);
        let mut messages = self.messages(session, Some(tail))?;
        if let Some(index) = messages.iter().position(|m| m.id == summary_id) {
            let summary = messages.remove(index);
            messages.insert(0, summary);
        }
        Ok(messages)
    }

    pub fn sent(&self, session: session::Id) -> Result<Vec<Sent>, Error> {
        let mut statement = self.conn.prepare(
            "select id, parts from messages where session_id = ?1 and role = 'user' order by id",
        )?;
        let rows = statement.query_map([session], |row| {
            Ok((row.get::<_, message::Id>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut sent = Vec::new();
        for row in rows {
            let (id, parts) = row?;
            let Ok(parts) = serde_json::from_str::<Vec<Part>>(&parts) else {
                continue;
            };
            let Some(text) = parts.iter().find_map(|part| match part {
                Part::Text { text, .. } => Some(text.to_string()),
                _ => None,
            }) else {
                continue;
            };
            let images = parts
                .into_iter()
                .filter_map(|part| match part {
                    Part::Image(image) => Some(image),
                    _ => None,
                })
                .collect();
            sent.push(Sent { id, text, images });
        }
        Ok(sent)
    }

    pub fn truncate(&self, session: session::Id, from: message::Id) -> Result<(), Error> {
        self.conn.execute(
            "delete from messages where session_id = ?1 and id >= ?2",
            params![session, from],
        )?;
        Ok(())
    }

    pub fn put_snapshot(&self, prompt: message::Id, tree: &str) -> Result<(), Error> {
        self.conn.execute(
            "insert or replace into snapshots (message_id, tree) values (?1, ?2)",
            params![prompt, tree],
        )?;
        Ok(())
    }

    pub fn snapshot(&self, prompt: message::Id) -> Result<Option<String>, Error> {
        Ok(self
            .conn
            .query_row(
                "select tree from snapshots where message_id = ?1",
                [prompt],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn set_title(&self, session: session::Id, title: &str) -> Result<(), Error> {
        self.conn.execute(
            "update sessions set title = ?2 where id = ?1",
            params![session, title],
        )?;
        Ok(())
    }

    pub fn title(&self, session: session::Id) -> Result<Option<String>, Error> {
        Ok(self
            .conn
            .query_row(
                "select title from sessions where id = ?1",
                [session],
                |row| row.get(0),
            )
            .optional()?
            .flatten())
    }

    pub fn recent_sessions(&self, cwd: &Path, limit: u32) -> Result<Vec<Recent>, Error> {
        let mut statement = self.conn.prepare(
            "select s.id, s.title,
                (select count(*) from messages m where m.session_id = s.id),
                (select m.parts from messages m where m.session_id = s.id and m.role = 'user' order by m.id limit 1),
                (select max(m.id) from messages m where m.session_id = s.id) as last
             from sessions s
             where s.cwd = ?1 and s.spawned_by is null
               and exists (select 1 from messages m where m.session_id = s.id)
             order by last desc limit ?2",
        )?;
        let rows = statement.query_map(params![cwd.to_string_lossy(), limit], |row| {
            Ok((
                row.get::<_, session::Id>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<message::Id>>(4)?,
            ))
        })?;
        rows.map(|row| {
            let (id, named, messages, first, last) = row?;
            Ok(Recent {
                id,
                title: named.unwrap_or_else(|| first.as_deref().map_or_default(title)),
                messages: u64::try_from(messages).unwrap_or(0),
                active_at: last.and_then(|last| last.unix_seconds()),
            })
        })
        .collect()
    }

    pub fn each_message(
        &self,
        session: session::Id,
        mut visit: impl FnMut(Message),
    ) -> Result<(), Error> {
        let mut statement = self.conn.prepare(
            "select id, role, model, account_id, parts from messages
             where session_id = ?1 order by id",
        )?;
        let mut rows = statement.query([session])?;
        while let Some(row) = rows.next()? {
            let role: String = row.get(1)?;
            let Some(role) = Role::parse(&role) else {
                continue;
            };
            let Ok(parts) = serde_json::from_str(&row.get::<_, String>(4)?) else {
                continue;
            };
            let origin = row
                .get::<_, Option<String>>(2)?
                .zip(row.get::<_, Option<account::Id>>(3)?)
                .map(|(model, account)| Origin {
                    model: model.into(),
                    account,
                });
            visit(Message {
                id: row.get(0)?,
                role,
                origin,
                parts,
            });
        }
        Ok(())
    }

    fn messages(
        &self,
        session: session::Id,
        from: Option<message::Id>,
    ) -> Result<Vec<Message>, Error> {
        let mut statement = self.conn.prepare(
            "select id, role, model, account_id, parts from messages
             where session_id = ?1 and (?2 is null or id >= ?2)
             order by id",
        )?;
        let rows = statement.query_map(params![session, from], |row| {
            Ok((
                row.get::<_, message::Id>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<account::Id>>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        rows.map(|row| {
            let (id, role, model, account, parts) = row?;
            Ok(Message {
                id,
                role: Role::parse(&role).ok_or(Error::Unknown("role"))?,
                origin: model.zip(account).map(|(model, account)| Origin {
                    model: model.into(),
                    account,
                }),
                parts: serde_json::from_str(&parts)?,
            })
        })
        .collect()
    }

    pub fn last_origin(&self, session: session::Id) -> Result<Option<Origin>, Error> {
        self.origin_query(
            "select model, account_id from messages where session_id = ?1 and model is not null order by id desc limit 1",
            session,
        )
    }

    pub fn last_model(&self, account: account::Id) -> Result<Option<Box<str>>, Error> {
        Ok(self
            .conn
            .query_row(
                "select model from messages where account_id = ?1 and model is not null order by id desc limit 1",
                [account],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(String::into_boxed_str))
    }

    pub fn last_used_account(&self) -> Result<Option<account::Id>, Error> {
        Ok(self
            .conn
            .query_row(
                "select account_id from messages where account_id is not null order by id desc limit 1",
                [],
                |row| row.get(0),
            )
            .optional()?)
    }

    fn origin_query(&self, sql: &str, session: session::Id) -> Result<Option<Origin>, Error> {
        Ok(self
            .conn
            .query_row(sql, [session], |row| {
                Ok(Origin {
                    model: row.get::<_, String>(0)?.into(),
                    account: row.get(1)?,
                })
            })
            .optional()?)
    }

    pub fn accounts(&self) -> Result<Vec<Account>, Error> {
        let mut statement = self.conn.prepare(
            "select id, provider, name, credential from accounts order by provider, name",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, account::Id>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        rows.map(|row| {
            let (id, provider, name, credential) = row?;
            Ok(Account {
                id,
                kind: account::Kind::parse(&provider).ok_or(Error::Unknown("provider"))?,
                name: name.into(),
                credential: serde_json::from_str::<Credential>(&credential)?,
            })
        })
        .collect()
    }

    pub fn account(&self, id: account::Id) -> Result<Option<Account>, Error> {
        Ok(self
            .accounts()?
            .into_iter()
            .find(|account| account.id == id))
    }

    pub fn save_account(&self, account: &Account) -> Result<(), Error> {
        self.conn.execute(
            "insert into accounts (id, provider, name, credential) values (?1, ?2, ?3, ?4)
             on conflict (provider, name) do update set credential = excluded.credential",
            params![
                account.id,
                account.kind.as_str(),
                &*account.name,
                serde_json::to_string(&account.credential)?,
            ],
        )?;
        Ok(())
    }

    pub fn remove_account(&self, id: account::Id) -> Result<bool, Error> {
        Ok(self
            .conn
            .execute("delete from accounts where id = ?1", [id])?
            > 0)
    }

    pub fn mcp_reviews(&self) -> Result<Vec<mcp::Review>, Error> {
        let mut statement = self
            .conn
            .prepare("select path, digest, decision from mcp_reviews")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, [u8; 32]>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (path, digest, decision) = row?;
            Ok(mcp::Review {
                path: PathBuf::from(path),
                digest: mcp::Digest::from_bytes(digest),
                decision: mcp::Decision::parse(&decision).ok_or(Error::Unknown("mcp decision"))?,
            })
        })
        .collect()
    }

    pub fn plugin_enabled(&self, name: &str) -> Result<Option<bool>, Error> {
        Ok(self
            .conn
            .query_row(
                "select enabled from plugins where name = ?1",
                [name],
                |row| row.get::<_, bool>(0),
            )
            .optional()?)
    }

    pub fn set_plugin_enabled(&self, name: &str, is_enabled: bool) -> Result<(), Error> {
        self.conn.execute(
            "insert into plugins (name, enabled) values (?1, ?2)
             on conflict (name) do update set enabled = excluded.enabled",
            params![name, is_enabled],
        )?;
        Ok(())
    }

    pub fn save_mcp_review(&self, review: &mcp::Review) -> Result<(), Error> {
        self.conn.execute(
            "insert into mcp_reviews (path, digest, decision) values (?1, ?2, ?3)
             on conflict (path) do update set digest = excluded.digest, decision = excluded.decision",
            params![
                review.path.to_string_lossy(),
                review.digest.as_bytes(),
                review.decision.as_str(),
            ],
        )?;
        Ok(())
    }
}

impl Logins {
    pub fn get(&self, key: &str) -> Result<Option<String>, Error> {
        Ok(self
            .lock()
            .query_row(
                "select credentials from logins where key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn save(&self, key: &str, credentials: &str) -> Result<(), Error> {
        self.lock().execute(
            "insert into logins (key, credentials) values (?1, ?2)
             on conflict (key) do update set credentials = excluded.credentials",
            [key, credentials],
        )?;
        Ok(())
    }

    pub fn remove(&self, key: &str) -> Result<bool, Error> {
        Ok(self
            .lock()
            .execute("delete from logins where key = ?1", [key])?
            > 0)
    }

    pub fn keys(&self, prefix: &str) -> Result<Vec<String>, Error> {
        let conn = self.lock();
        let mut statement = conn
            .prepare("select key from logins where substr(key, 1, length(?1)) = ?1 order by key")?;
        let keys = statement
            .query_map([prefix], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(keys)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn title(parts: &str) -> String {
    let parts: Vec<Part> = serde_json::from_str(parts).unwrap_or_default();
    parts
        .iter()
        .find_map(|part| match part {
            Part::Text { text, .. } => Some(first_line(text).to_owned()),
            Part::Shell { command, .. } => Some(format!("!{}", first_line(command))),
            _ => None,
        })
        .unwrap_or_default()
}

fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
}

fn session_info(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionInfo> {
    Ok(SessionInfo {
        id: row.get(0)?,
        cwd: PathBuf::from(row.get::<_, String>(1)?),
        instructions: row.get::<_, String>(2)?.into(),
        created_at: row.get(3)?,
    })
}

fn create_private(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .map(drop)
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_mcp_review_replaces_the_old_one_for_the_same_file() {
        let store = Store::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".goat")).unwrap();
        let path = dir.path().join(".goat/mcp.toml");
        std::fs::write(&path, "[a]\nurl = \"https://a\"\n").unwrap();
        let first = mcp::config(dir.path(), &dir.path().join("home"), |_| None, &[]).unreviewed;
        std::fs::write(&path, "[b]\nurl = \"https://b\"\n").unwrap();
        let second = mcp::config(dir.path(), &dir.path().join("home"), |_| None, &[]).unreviewed;

        store
            .save_mcp_review(&first[0].clone().decide(mcp::Decision::Allowed))
            .unwrap();
        store
            .save_mcp_review(&second[0].clone().decide(mcp::Decision::Denied))
            .unwrap();

        assert_eq!(
            store.mcp_reviews().unwrap(),
            [second[0].clone().decide(mcp::Decision::Denied)]
        );
    }

    #[test]
    fn logins_are_shared_with_the_store_they_come_from() {
        let store = Store::open_in_memory().unwrap();
        let logins = store.logins().unwrap();

        logins.save("mcp:https://a", "{\"v\":1}").unwrap();
        logins.save("mcp:https://a", "{\"v\":2}").unwrap();

        assert_eq!(
            store.logins().unwrap().get("mcp:https://a").unwrap(),
            Some("{\"v\":2}".to_owned())
        );
        assert!(logins.remove("mcp:https://a").unwrap());
        assert_eq!(logins.get("mcp:https://a").unwrap(), None);
    }

    #[test]
    fn a_plugin_remembers_whether_it_is_enabled() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.plugin_enabled("computer").unwrap(), None);

        store.set_plugin_enabled("computer", true).unwrap();
        store.set_plugin_enabled("gmail", false).unwrap();

        assert_eq!(store.plugin_enabled("computer").unwrap(), Some(true));
        assert_eq!(store.plugin_enabled("gmail").unwrap(), Some(false));
    }

    #[test]
    fn login_keys_are_listed_by_exact_prefix() {
        let store = Store::open_in_memory().unwrap();
        let logins = store.logins().unwrap();
        for key in [
            "mcp:https://a/mcp",
            "mcp:https://a/mcp#work",
            "mcp:https://a/mcp2",
            "mcp:https://a_mcp#x",
        ] {
            logins.save(key, "{}").unwrap();
        }

        let keys = logins.keys("mcp:https://a/mcp#").unwrap();

        assert_eq!(keys, ["mcp:https://a/mcp#work"]);
    }

    #[test]
    fn an_image_is_stored_once_and_read_back() {
        let store = Store::open_in_memory().unwrap();
        let picture = crate::image::Picture {
            image: crate::image::Image {
                id: crate::image::Id::of(b"bytes"),
                media: crate::image::Media::Png,
            },
            width: 1,
            height: 1,
            bytes: b"bytes".to_vec(),
        };

        store.put_image(&picture).unwrap();
        store.put_image(&picture).unwrap();

        assert_eq!(
            store.image(&picture.image.id).unwrap(),
            Some(b"bytes".to_vec())
        );
    }

    fn origin() -> Origin {
        Origin {
            model: "gpt-6.2-astra".into(),
            account: account::Id::generate(),
        }
    }

    #[test]
    fn messages_round_trip_in_order() {
        let store = Store::open_in_memory().unwrap();
        let session = store.create_session(Path::new("/w"), "be brief").unwrap();
        let origin = origin();
        store.append(session, &Message::user("hi")).unwrap();
        store
            .append(
                session,
                &Message::new(
                    Role::Assistant,
                    Some(origin.clone()),
                    vec![Part::text("hello")],
                ),
            )
            .unwrap();
        let messages = store.context(session).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, Role::User);
        assert_eq!(messages[1].origin, Some(origin.clone()));
        assert_eq!(store.last_origin(session).unwrap(), Some(origin));
    }

    #[test]
    fn context_starts_at_the_latest_summary() {
        let store = Store::open_in_memory().unwrap();
        let session = store.create_session(Path::new("/w"), "be brief").unwrap();
        store.append(session, &Message::user("old")).unwrap();
        let tail = Message::user("current");
        store.append(session, &tail).unwrap();
        let summary = Message::new(
            Role::Summary,
            None,
            vec![Part::Summary {
                text: "earlier work".into(),
                tail: tail.id,
            }],
        );
        store.append(session, &summary).unwrap();
        store.append(session, &Message::user("after")).unwrap();

        let roles: Vec<_> = store
            .context(session)
            .unwrap()
            .iter()
            .map(|m| match &m.parts[0] {
                Part::Text { text, .. } | Part::Summary { text, .. } => text.to_string(),
                _ => String::new(),
            })
            .collect();
        assert_eq!(roles, ["earlier work", "current", "after"]);
    }

    #[test]
    fn accounts_are_unique_per_provider_and_name() {
        let store = Store::open_in_memory().unwrap();
        let first = Account::new(
            account::Kind::OpenAi,
            "work",
            Credential::ApiKey { key: "a".into() },
        );
        let second = Account::new(
            account::Kind::OpenAi,
            "work",
            Credential::ApiKey { key: "b".into() },
        );
        store.save_account(&first).unwrap();
        store.save_account(&second).unwrap();
        let accounts = store.accounts().unwrap();
        assert_eq!(accounts.len(), 1);
        assert!(matches!(&accounts[0].credential, Credential::ApiKey { key } if &**key == "b"));
    }

    #[test]
    fn a_named_session_is_listed_by_its_title() {
        let store = Store::open_in_memory().unwrap();
        let cwd = Path::new("/work");
        let id = store.create_session(cwd, "be brief").unwrap();
        store
            .append(id, &Message::user("first line\nmore"))
            .unwrap();

        let unnamed = store.recent_sessions(cwd, 10).unwrap();
        store.set_title(id, "Startup banner").unwrap();
        let named = store.recent_sessions(cwd, 10).unwrap();

        assert_eq!(unnamed[0].title, "first line");
        assert_eq!(named[0].title, "Startup banner");
        assert_eq!(store.title(id).unwrap().as_deref(), Some("Startup banner"));
    }

    #[test]
    fn empty_sessions_are_not_listed() {
        let store = Store::open_in_memory().unwrap();
        let cwd = Path::new("/work");
        store.create_session(cwd, "be brief").unwrap();

        assert_eq!(store.recent_sessions(cwd, 10).unwrap(), Vec::new());
    }

    fn agent_of(store: &Store, cwd: &Path) -> (session::Id, Message, session::Id) {
        let parent = store.create_session(cwd, "be brief").unwrap();
        store.append(parent, &Message::user("look around")).unwrap();
        let call = Message::new(Role::Assistant, Some(origin()), vec![Part::text("on it")]);
        store.append(parent, &call).unwrap();
        let agent = store
            .create_agent_session(cwd, "be brief", call.id)
            .unwrap();
        store.append(agent, &Message::user("search")).unwrap();
        (parent, call, agent)
    }

    #[test]
    fn agent_sessions_are_not_listed_or_continued() {
        let store = Store::open_in_memory().unwrap();
        let cwd = Path::new("/work");
        let (parent, _, _) = agent_of(&store, cwd);

        let listed: Vec<session::Id> = store
            .recent_sessions(cwd, 10)
            .unwrap()
            .iter()
            .map(|recent| recent.id)
            .collect();

        assert_eq!(listed, [parent]);
        assert_eq!(store.latest_session(cwd).unwrap().unwrap().id, parent);
    }

    #[test]
    fn rewinding_past_the_call_deletes_the_agent_session() {
        let store = Store::open_in_memory().unwrap();
        let (parent, call, agent) = agent_of(&store, Path::new("/work"));

        store.truncate(parent, call.id).unwrap();

        assert!(store.session(agent).unwrap().is_none());
        assert!(store.context(agent).unwrap().is_empty());
    }

    #[test]
    fn an_agent_belongs_only_to_the_session_that_started_it() {
        let store = Store::open_in_memory().unwrap();
        let cwd = Path::new("/work");
        let (parent, _, agent) = agent_of(&store, cwd);
        let other = store.create_session(cwd, "be brief").unwrap();

        assert!(store.is_agent_of(agent, parent).unwrap());
        assert!(!store.is_agent_of(agent, other).unwrap());
    }

    #[test]
    fn database_file_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("goat.db");
        Store::open(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
