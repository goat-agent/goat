import { constants } from "node:fs";
import { lstat, mkdir, open, stat } from "node:fs/promises";
import { dirname } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { setTimeout as sleep } from "node:timers/promises";
import { err, ok, type Result } from "@goat/result";
import type { CredentialError } from "./credential.ts";
import { schema } from "./schema.ts";

export interface Sql {
  get(query: string, ...values: readonly string[]): unknown;
  all(query: string, ...values: readonly string[]): unknown;
  run(query: string, ...values: readonly string[]): number;
}

// SQLite stores the statement without IF NOT EXISTS and keeps the writer's whitespace.
const table = flat(schema.replace("CREATE TABLE IF NOT EXISTS", "CREATE TABLE"));
const findTable = "SELECT sql FROM sqlite_schema WHERE name = 'credentials' COLLATE NOCASE";
const posix = process.platform !== "win32";
const create = constants.O_WRONLY | constants.O_CREAT | constants.O_NOFOLLOW | constants.O_NONBLOCK;
const lockWait = 10_000;
const lockRetry = 25;
const busy = 5;
const constraint = 19;

export async function read<R>(
  path: string,
  query: (sql: Sql) => R,
): Promise<Result<R, CredentialError> | undefined> {
  const exists = await inspect(path);
  if (!exists.ok) {
    return exists;
  }
  if (!exists.value) {
    return undefined;
  }
  try {
    return await session(path, false, (sql) => {
      const found = sql.get(findTable);
      if (found !== undefined && !sameTable(found)) {
        return err(foreignTable());
      }
      return found === undefined ? undefined : ok(query(sql));
    });
  } catch (error) {
    return err(failure(error));
  }
}

export async function write<R>(
  path: string,
  change: (sql: Sql) => R,
): Promise<Result<R, CredentialError>> {
  try {
    await mkdir(dirname(path), { recursive: true, mode: 0o700 });
  } catch (error) {
    return err(failure(error));
  }
  let exists = await inspect(path);
  if (exists.ok && !exists.value) {
    try {
      await (await open(path, create, 0o600)).close();
    } catch (error) {
      return err(failure(error));
    }
    exists = await inspect(path);
  }
  if (!exists.ok) {
    return exists;
  }
  try {
    return ok(await session(path, true, change));
  } catch (error) {
    return err(failure(error));
  }
}

async function session<R>(path: string, transaction: boolean, use: (sql: Sql) => R): Promise<R> {
  const database = new DatabaseSync(path, { timeout: 0 });
  const sql: Sql = {
    get: (query, ...values) => database.prepare(query).get(...values),
    all: (query, ...values) => database.prepare(query).all(...values),
    run: (query, ...values) => Number(database.prepare(query).run(...values).changes),
  };
  try {
    const deadline = Date.now() + lockWait;
    if (!transaction) {
      return await retry(() => use(sql), deadline);
    }
    await retry(() => {
      database.exec("PRAGMA journal_mode = WAL");
    }, deadline);
    database.exec(
      "PRAGMA synchronous = FULL; PRAGMA fullfsync = ON; PRAGMA checkpoint_fullfsync = ON; PRAGMA secure_delete = ON",
    );
    await retry(() => {
      database.exec("BEGIN IMMEDIATE");
    }, deadline);
    const result = commit(
      (query) => {
        database.exec(query);
      },
      sql,
      use,
    );
    // Drops replaced and deleted secrets from the WAL; a busy checkpoint only returns busy = 1.
    database.exec("PRAGMA wal_checkpoint(TRUNCATE)");
    return result;
  } finally {
    database.close();
  }
}

function commit<R>(exec: (query: string) => void, sql: Sql, use: (sql: Sql) => R): R {
  try {
    exec(schema);
    if (!sameTable(sql.get(findTable))) {
      throw new ForeignTable();
    }
    const result = use(sql);
    exec("COMMIT");
    return result;
  } catch (error) {
    exec("ROLLBACK");
    throw error;
  }
}

async function retry<R>(attempt: () => R, deadline: number): Promise<R> {
  try {
    return attempt();
  } catch (error) {
    if (errcode(error) !== busy || Date.now() > deadline) {
      throw error;
    }
  }
  await sleep(lockRetry);
  return retry(attempt, deadline);
}

class ForeignTable extends Error {}

function sameTable(row: unknown): boolean {
  return (
    typeof row === "object" &&
    row !== null &&
    "sql" in row &&
    typeof row.sql === "string" &&
    flat(row.sql) === table
  );
}

function flat(sql: string): string {
  return sql.replaceAll(/\s+/gu, " ").trim();
}

function foreignTable(): CredentialError {
  return {
    kind: "malformed",
    message: "The credential store has a table this module does not know.",
  };
}

async function unsafeFolder(path: string): Promise<CredentialError | undefined> {
  const folder = await stat(dirname(path));
  return posix && (folder.uid !== process.getuid?.() || (folder.mode & 0o022) !== 0)
    ? {
        kind: "invalid",
        message: `${dirname(path)} must belong to you and be writable only by you.`,
      }
    : undefined;
}

async function inspect(path: string): Promise<Result<boolean, CredentialError>> {
  try {
    const folder = await unsafeFolder(path);
    if (folder !== undefined) {
      return err(folder);
    }
    const info = await lstat(path);
    if (!info.isFile()) {
      return err({ kind: "invalid", message: `${path} is not a regular file.` });
    }
    if (posix && (info.uid !== process.getuid?.() || (info.mode & 0o077) !== 0)) {
      return err({
        kind: "invalid",
        message: `${path} must belong to you and be readable only by you.`,
      });
    }
    return ok(true);
  } catch (error) {
    return error instanceof Error && "code" in error && error.code === "ENOENT"
      ? ok(false)
      : err(failure(error));
  }
}

function failure(error: unknown): CredentialError {
  if (error instanceof ForeignTable) {
    return foreignTable();
  }
  const code = errcode(error);
  if (code === busy) {
    return { kind: "unavailable", message: "Another process kept the credential store busy." };
  }
  if (code === constraint) {
    return { kind: "invalid", message: "The credential breaks the rules of the store." };
  }
  return { kind: "unavailable", message: error instanceof Error ? error.message : String(error) };
}

function errcode(error: unknown): number | undefined {
  return error instanceof Error && "errcode" in error && typeof error.errcode === "number"
    ? error.errcode & 0xff
    : undefined;
}
