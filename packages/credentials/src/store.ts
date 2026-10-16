import { homedir } from "node:os";
import { join } from "node:path";
import { z } from "zod";
import { err, ok, type Result } from "@goat/result";
import type { Credential, CredentialError, Method } from "./credential.ts";
import { read, write } from "./database.ts";

export interface Entry {
  readonly name: string;
  readonly provider: string;
  readonly method: string;
}

export interface Credentials<T> {
  list(): Promise<Result<readonly Entry[], CredentialError>>;
  set(name: string, credential: Credential): Promise<CredentialError | undefined>;
  delete(name: string): Promise<CredentialError | undefined>;
  connect(name: string): Promise<Result<T, CredentialError>>;
}

export interface Options<T> {
  readonly methods: readonly Method<T>[];
  readonly path?: string;
}

const namePattern = /^[a-z0-9][a-z0-9-]{0,63}$/u;
const kindPattern = /^[A-Za-z0-9-]{1,64}$/u;
const kind = z.string().regex(kindPattern);
const entries = z.array(
  z.object({ name: z.string().regex(namePattern), provider: kind, method: kind }),
);
const stored = z.object({ provider: kind, method: kind, fields: z.string() });
const fieldsShape = z.record(z.string(), z.json());

export function openCredentials<T>(options: Options<T>): Credentials<T> {
  const path = options.path ?? join(homedir(), ".goat", "credentials.db");
  const { methods } = options;
  return {
    list: async () => list(path),
    set: async (name, credential) => set(path, methods, name, credential),
    delete: async (name) => remove(path, name),
    connect: async (name) => connect(path, methods, name),
  };
}

async function list(path: string): Promise<Result<readonly Entry[], CredentialError>> {
  const rows = await read(path, (sql) =>
    sql.all("SELECT name, provider, method FROM credentials ORDER BY name"),
  );
  if (rows === undefined) {
    return ok([]);
  }
  if (!rows.ok) {
    return rows;
  }
  const parsed = entries.safeParse(rows.value);
  return parsed.success ? ok(parsed.data) : err(malformedStore());
}

async function set<T>(
  path: string,
  methods: readonly Method<T>[],
  name: string,
  credential: Credential,
): Promise<CredentialError | undefined> {
  const problem = checkName(name);
  if (problem !== undefined) {
    return problem;
  }
  if (!kindPattern.test(credential.provider) || !kindPattern.test(credential.method)) {
    return {
      kind: "invalid",
      message: "A provider or method is 1 to 64 letters, digits and hyphens.",
    };
  }
  const connected = connectWith(methods, name, credential);
  if (!connected.ok) {
    return connected.error;
  }
  const saved = await write(path, (sql) =>
    sql.run(
      `INSERT INTO credentials (name, provider, method, fields) VALUES (?, ?, ?, ?)
       ON CONFLICT (name) DO UPDATE SET
         provider = excluded.provider, method = excluded.method, fields = excluded.fields`,
      name,
      credential.provider,
      credential.method,
      JSON.stringify(credential.fields),
    ),
  );
  return saved.ok ? undefined : saved.error;
}

async function remove(path: string, name: string): Promise<CredentialError | undefined> {
  const problem = checkName(name);
  if (problem !== undefined) {
    return problem;
  }
  const exists = await read(path, (sql) =>
    sql.get("SELECT 1 FROM credentials WHERE name = ?", name),
  );
  if (exists === undefined || (exists.ok && exists.value === undefined)) {
    return missing(name);
  }
  if (!exists.ok) {
    return exists.error;
  }
  const deleted = await write(path, (sql) =>
    sql.run("DELETE FROM credentials WHERE name = ?", name),
  );
  if (!deleted.ok) {
    return deleted.error;
  }
  return deleted.value === 0 ? missing(name) : undefined;
}

async function connect<T>(
  path: string,
  methods: readonly Method<T>[],
  name: string,
): Promise<Result<T, CredentialError>> {
  const problem = checkName(name);
  if (problem !== undefined) {
    return err(problem);
  }
  const row = await read(path, (sql) =>
    sql.get("SELECT provider, method, fields FROM credentials WHERE name = ?", name),
  );
  if (row === undefined) {
    return err(missing(name));
  }
  if (!row.ok) {
    return row;
  }
  if (row.value === undefined) {
    return err(missing(name));
  }
  const credential = parse(row.value);
  return credential === undefined
    ? err({ kind: "malformed", message: `The credential "${name}" cannot be read.` })
    : connectWith(methods, name, credential);
}

function parse(row: unknown): Credential | undefined {
  const columns = stored.safeParse(row);
  if (!columns.success) {
    return undefined;
  }
  try {
    const fields = fieldsShape.safeParse(JSON.parse(columns.data.fields));
    return fields.success
      ? { provider: columns.data.provider, method: columns.data.method, fields: fields.data }
      : undefined;
  } catch {
    return undefined;
  }
}

function connectWith<T>(
  methods: readonly Method<T>[],
  name: string,
  credential: Credential,
): Result<T, CredentialError> {
  const method = methods.find(
    (candidate) =>
      candidate.provider === credential.provider && candidate.method === credential.method,
  );
  if (method === undefined) {
    return err({
      kind: "unsupported",
      message: `The credential "${name}" is ${credential.provider} ${credential.method}, which this product cannot use.`,
    });
  }
  const connected = method.connect(credential.fields);
  return connected.ok
    ? connected
    : err({ kind: "invalid", message: `The credential "${name}" is invalid: ${connected.error}` });
}

function checkName(name: string): CredentialError | undefined {
  return namePattern.test(name)
    ? undefined
    : {
        kind: "invalid",
        message: "A credential name is 1 to 64 lowercase letters, digits and hyphens.",
      };
}

function missing(name: string): CredentialError {
  return { kind: "missing", message: `No credential is named "${name}".` };
}

function malformedStore(): CredentialError {
  return { kind: "malformed", message: "The credential store holds rows it cannot read." };
}
