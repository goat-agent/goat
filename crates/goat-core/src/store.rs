use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

use crate::account::{self, Account, Credential};
use crate::image::{self, Picture};
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
];

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
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self, Error> {
        let store = Self {
            conn: Connection::open_in_memory()?,
        };
        store.migrate()?;
        Ok(store)
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
                "select id, cwd, instructions, created_at from sessions where cwd = ?1 order by id desc limit 1",
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
    fn database_file_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("goat.db");
        Store::open(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
