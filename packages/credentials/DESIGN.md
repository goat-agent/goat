# @goat/credentials

Stores goat's named credentials, for any service, and turns one into what goat uses.

## Storage

- One SQLite database: `~/.goat/credentials.db`. Several goat processes write it at once, and SQLite gives them locks and transactions. Node has no other cross-process file lock.
- One `STRICT` table, `credentials`, created by the first write. It has four text columns: `name`, `provider`, `method` and `fields`.
- `provider` names the service and `method` names the kind of credential. `fields` is a JSON object whose shape the method defines. `set` enforces these rules, and reads refuse rows that break them:
  - A name is 1 to 64 lowercase ASCII letters, digits and hyphens, and starts with a letter or digit. `name/model` therefore splits at the first slash.
  - `provider` and `method` are 1 to 64 ASCII letters, digits and hyphens.
- A writer:
  1. creates the directory with mode 700 if it is missing, because a directory that group or others can write is refused,
  2. creates the database with mode 600 and without following a symbolic link,
  3. sets `journal_mode = WAL`, `synchronous = FULL`, `fullfsync = ON`, `checkpoint_fullfsync = ON` and `secure_delete = ON`,
  4. changes rows only inside `BEGIN IMMEDIATE`,
  5. runs `wal_checkpoint(TRUNCATE)` after it commits, so replaced and deleted secrets leave the WAL once no other connection holds an older read transaction.
- No process waits for a lock synchronously. A busy database is retried every 25 ms until 10 s have passed, then reported as `unavailable`. SQLite releases the lock of a process that dies.
- Reads never create the database.
- On POSIX, readers and writers refuse a database that is not a regular file, does not belong to the user, or that group or others can open, and a directory, after links are followed, that does not belong to the user or that group or others can write. SQLite gives its `-wal` and `-shm` files the database's mode. Windows gets no ownership or mode checks.
- Secrets are stored in plain text.
- Secrets never come from environment variables, because child processes inherit them, including every command an agent runs.

## Methods

- A method is one kind of credential, such as `anthropic` `apiKey`. The package that knows the service exports it, and its design document lists the fields.
- A method validates `fields` and builds what goat uses: a `Provider`, a client, or the validated fields themselves. Another consumer of the same kind wraps that method instead of repeating its fields.
- `connect` is pure and synchronous. Secrets that need I/O are resolved inside what it builds.
- The caller passes the methods it supports. Other credentials still list, and fail to connect with `unsupported`.
- `set` checks the name, `provider` and `method`, then validates through `connect` before it writes.
- A base URL uses `secureUrl`: https, or http only for `localhost`, `127.0.0.1` and `[::1]`.

## Evolution

- Everything new goes inside `fields`. A table change needs a migration, and none exists yet.
- An older goat may still run against the same store. A method therefore gains a field only if older readers can safely ignore it. A field that older versions must obey, such as one that routes requests, makes a new method, which they refuse as `unsupported`.
- A method never gains a required field. A field may accept more values later; older versions refuse them as `invalid`.
- `email`, `plan` and `label` at the top of `fields` are reserved for display. They are never secret.
- `set` replaces the whole row with the fields it is given. goat replaces a name without asking only when `connect` reports `missing`.

## Errors

Every failure is a `CredentialError`:

- `invalid`: a bad name, provider, method or fields, or an unsafe database file or directory.
- `missing`: no credential has that name.
- `malformed`: a row this package cannot read.
- `unsupported`: a method the caller did not pass.
- `unavailable`: the database stayed busy, cannot be opened, or is not a database.

A message never contains a field value. It repeats a name, provider or method only after they pass the rules, and may contain the database path and the file system's error text.

## Not in scope

- Choosing which credential to use. That is goat's configuration.
- Key pools and failover.
- Claude and Gemini CLI subscription OAuth. Their terms forbid third-party clients.
- Tokens that a client creates for itself, such as MCP OAuth tokens for each server. They use another database file with this package.

## Extension points

Each is an addition.

- **Refresh**, such as `openai` `chatgpt`:
  - `connect` gets an optional second argument: an `update(change)` bound to that credential. `update` reads the row, awaits `change` on the latest fields, and writes the result, all inside one `BEGIN IMMEDIATE` transaction that also covers the refresh request. No two processes therefore send the same refresh token.
  - `change` checks the expiry again, keeps fields it does not know, and keeps the old refresh token when none returns. A refresh never deletes a credential.
  - The refresh request times out before the 10 s lock wait ends.
  - What `connect` builds shares one refresh among its concurrent requests.
  - `set` calls `connect` without the second argument, so a refreshing method still builds a value that fails cleanly once its token expires.
- **Secret references**: a field may hold `{ "command": ["op", "read", "…"] }`, an argument list rather than a shell string. Fields that rotate never use one.
- **Login**: packages export login flows that return a `Credential`, possibly of another method. OpenRouter's PKCE login returns an `apiKey`.
- **Display fields** in `list`, read from the reserved keys.
- **Keychain storage** for secret fields.
