import { afterEach, beforeEach, expect, test } from "bun:test";
import { chmod, mkdir, mkdtemp, readdir, rm, stat, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { err, ok } from "@goat/result";
import type { Credential, CredentialError, Method } from "./credential.ts";
import { openCredentials } from "./store.ts";

const keyMethod: Method<string> = {
  provider: "test",
  method: "key",
  connect: (fields) =>
    typeof fields["key"] === "string" ? ok(`client for ${fields["key"]}`) : err("key is missing"),
};

const work: Credential = { provider: "test", method: "key", fields: { key: "secret" } };

let root = "";
let path = "";

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), "goat-credentials-"));
  path = join(root, "goat", "credentials.db");
});

afterEach(async () => {
  await rm(root, { recursive: true, force: true });
});

function store(): ReturnType<typeof openCredentials<string>> {
  return openCredentials({ methods: [keyMethod], path });
}

function insert(name: string, provider: string, method: string, fields: string): void {
  const database = new DatabaseSync(path);
  try {
    database
      .prepare("INSERT INTO credentials VALUES (?, ?, ?, ?)")
      .run(name, provider, method, fields);
  } finally {
    database.close();
  }
}

async function kindOf(name: string): Promise<string | undefined> {
  const connected = await store().connect(name);
  return connected.ok ? undefined : connected.error.kind;
}

test("lists nothing and creates nothing before anything is saved", async () => {
  expect(await store().list()).toEqual(ok([]));
  expect(await kindOf("work")).toBe("missing");
  expect((await store().delete("work"))?.kind).toBe("missing");
  expect(await Bun.file(path).exists()).toBe(false);
});

test("saves a credential in a database only the user can open", async () => {
  expect(await store().set("work", work)).toBeUndefined();
  expect((await stat(path)).mode & 0o777).toBe(0o600);
  expect((await stat(join(root, "goat"))).mode & 0o777).toBe(0o700);
  const files = await readdir(join(root, "goat"));
  const modes = await Promise.all(
    files.map(async (file) => (await stat(join(root, "goat", file))).mode & 0o777),
  );
  expect(modes.every((mode) => mode === 0o600)).toBe(true);
});

test("connects a saved credential through its method", async () => {
  await store().set("work", work);
  expect(await store().connect("work")).toEqual(ok("client for secret"));
});

test("lists credentials by name without reading their fields", async () => {
  await store().set("work", work);
  await store().set("home", work);
  expect(await store().list()).toEqual(
    ok([
      { name: "home", provider: "test", method: "key" },
      { name: "work", provider: "test", method: "key" },
    ]),
  );
});

test("replaces a credential saved under the same name", async () => {
  await store().set("work", work);
  await store().set("work", { ...work, fields: { key: "rotated" } });
  expect(await store().connect("work")).toEqual(ok("client for rotated"));
});

test("refuses what the method refuses and saves nothing", async () => {
  expect(await store().set("work", { ...work, fields: {} })).toEqual({
    kind: "invalid",
    message: 'The credential "work" is invalid: key is missing',
  });
  expect(await store().list()).toEqual(ok([]));
});

test("refuses methods the product does not know", async () => {
  expect(await store().set("work", { ...work, method: "oauth" })).toEqual({
    kind: "unsupported",
    message: 'The credential "work" is test oauth, which this product cannot use.',
  });
  await store().set("work", work);
  insert("bot", "discord", "botToken", '{"token":"t"}');
  expect(await kindOf("bot")).toBe("unsupported");
  const listed = await store().list();
  expect(listed.ok ? listed.value.map((entry) => entry.name) : []).toEqual(["bot", "work"]);
});

test("refuses names that are not short lowercase slugs, without echoing them", async () => {
  const names = ["", "Work", "a/b", "-a", "a.b", "a b", "a".repeat(65)];
  const problems = await Promise.all(
    names.map(async (name) => [
      await store().set(name, work),
      await store().delete(name),
      await kindOf(name),
    ]),
  );
  const invalid: CredentialError = {
    kind: "invalid",
    message: "A credential name is 1 to 64 lowercase letters, digits and hyphens.",
  };
  expect(problems).toEqual(names.map(() => [invalid, invalid, "invalid"]));
  expect(await store().set("a".repeat(64), work)).toBeUndefined();
});

test("refuses providers and methods outside the rules, without echoing them", async () => {
  expect(await store().set("work", { ...work, provider: "evil\u001B[2J" })).toEqual({
    kind: "invalid",
    message: "A provider or method is 1 to 64 letters, digits and hyphens.",
  });
});

test("leaves no replaced or deleted secret in the store's files", async () => {
  await store().set("work", { ...work, fields: { key: "sk-OLDSECRET" } });
  await store().set("work", { ...work, fields: { key: "sk-NEWSECRET" } });
  await store().delete("work");
  const files = await readdir(join(root, "goat"));
  const contents = await Promise.all(
    files.map(async (file) => Bun.file(join(root, "goat", file)).text()),
  );
  expect(contents.some((text) => text.includes("SECRET"))).toBe(false);
});

test("deletes a credential", async () => {
  await store().set("work", work);
  await store().set("home", work);
  expect(await store().delete("work")).toBeUndefined();
  expect(await kindOf("work")).toBe("missing");
  expect((await store().delete("work"))?.kind).toBe("missing");
});

test("enforces the rules in the database for writers in other languages", async () => {
  await store().set("work", work);
  const rows: readonly (readonly [string, string, string, string])[] = [
    ["Bad", "test", "key", "{}"],
    ["ok\u0000Bad", "test", "key", "{}"],
    ["ok", "test\u0000\u001B[2J", "key", "{}"],
    ["ok", "test", "key", '{}\u0000"'],
    ["ok", "te st", "key", "{}"],
    ["ok", "test", "api_key", "{}"],
    ["ok", "test", "key", "[]"],
    ["ok", "test", "key", "{"],
  ];
  for (const row of rows) {
    expect(() => {
      insert(...row);
    }).toThrow();
  }
});

test("refuses a store that is a link, that others can open, or in a folder others can write", async () => {
  await store().set("work", work);
  await chmod(path, 0o644);
  expect(await kindOf("work")).toBe("invalid");
  expect((await store().set("home", work))?.kind).toBe("invalid");
  await chmod(path, 0o600);
  await chmod(join(root, "goat"), 0o770);
  expect(await kindOf("work")).toBe("invalid");
  expect((await store().set("home", work))?.kind).toBe("invalid");
  await chmod(join(root, "goat"), 0o700);
  expect(await kindOf("work")).toBeUndefined();
  const link = join(root, "link.db");
  await symlink(path, link);
  const linked = openCredentials({ methods: [keyMethod], path: link });
  const connected = await linked.connect("work");
  expect(connected.ok ? undefined : connected.error.kind).toBe("invalid");
});

test("refuses a table that another writer created with other rules", async () => {
  await mkdir(join(root, "goat"), { mode: 0o700 });
  const database = new DatabaseSync(path);
  database.exec(
    "CREATE TABLE credentials (name TEXT PRIMARY KEY, provider TEXT NOT NULL, method TEXT NOT NULL, fields TEXT NOT NULL) STRICT",
  );
  database.close();
  await chmod(path, 0o600);
  insert("Bad\u001B[2J", "te st", "key", "{}");
  expect((await store().list()).ok).toBe(false);
  expect((await store().set("work", work))?.kind).toBe("malformed");
});

test("accepts the table from DESIGN.md whatever its indentation and line ends", async () => {
  const design = await Bun.file(join(import.meta.dir, "..", "DESIGN.md")).text();
  const statement = /```sql\n([^`]*)```/u.exec(design)?.[1] ?? "";
  await mkdir(join(root, "goat"), { mode: 0o700 });
  const database = new DatabaseSync(path);
  database.exec(statement.replaceAll("\n", "\r\n"));
  database.close();
  await chmod(path, 0o600);
  expect(await store().set("work", work)).toBeUndefined();
  expect(await store().connect("work")).toEqual(ok("client for secret"));
});

test("refuses a credentials table in any letter case", async () => {
  await mkdir(join(root, "goat"), { mode: 0o700 });
  const database = new DatabaseSync(path);
  database.exec(
    "CREATE TABLE Credentials (name TEXT PRIMARY KEY, provider TEXT NOT NULL, method TEXT NOT NULL, fields TEXT NOT NULL) STRICT",
  );
  database.close();
  await chmod(path, 0o600);
  const listed = await store().list();
  expect(listed.ok ? undefined : listed.error.kind).toBe("malformed");
});

test("refuses a link, pipe or folder at the store path when saving", async () => {
  await mkdir(join(root, "goat"), { mode: 0o700 });
  await symlink(join(root, "goat", "nowhere.db"), path);
  expect((await store().set("work", work))?.kind).toBe("invalid");
  expect(await Bun.file(join(root, "goat", "nowhere.db")).exists()).toBe(false);
});

test("returns an error instead of throwing when the store cannot be opened", async () => {
  await mkdir(join(root, "goat"), { mode: 0o700 });
  await writeFile(join(root, "goat", "credentials.db"), "not a database", { mode: 0o600 });
  expect((await store().set("work", work))?.kind).toBe("unavailable");
  expect((await store().list()).ok).toBe(false);
  const blocked = openCredentials({ methods: [keyMethod], path: join(path, "inside.db") });
  expect((await blocked.set("work", work))?.kind).toBe("unavailable");
});

test("loses no write when Bun and Node processes write at once", async () => {
  const script = join(root, "writer.mjs");
  await writeFile(
    script,
    `const { openCredentials } = await import(${JSON.stringify(join(import.meta.dir, "store.ts"))});
const method = { provider: "test", method: "key", connect: () => ({ ok: true, value: "c" }) };
const store = openCredentials({ methods: [method], path: process.argv[2] });
for (let i = 0; i < 10; i++) {
  for (const name of [\`w\${process.argv[3]}-\${i}\`, "shared"]) {
    const error = await store.set(name, { provider: "test", method: "key", fields: { i } });
    if (error !== undefined) throw new Error(error.message);
  }
}
`,
  );
  const exits = Array.from(
    { length: 8 },
    async (_, index) =>
      Bun.spawn([index % 2 === 0 ? "bun" : "node", script, path, String(index)], {
        stderr: "inherit",
      }).exited,
  );
  expect(await Promise.all(exits)).toEqual([0, 0, 0, 0, 0, 0, 0, 0]);
  const listed = await store().list();
  expect(listed.ok ? listed.value.length : 0).toBe(81);
});
