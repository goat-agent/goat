import { mock } from "bun:test";
import { drizzle } from "drizzle-orm/postgres-js";
import { migrate } from "drizzle-orm/postgres-js/migrator";
import postgres from "postgres";
import {
  FakeResend,
  fakeFirecrawl,
  fakeIntegrations,
  fakeModel,
  fakeStorage,
  fakeTranscribe,
  fakeVoice,
} from "./fakes";

const url = new URL(process.env.DATABASE_URL ?? "");
const database = url.pathname.slice(1);
if (!/^[a-z0-9_]+_test$/.test(database)) {
  throw new Error(`refusing to reset "${database}": the test database name must end with _test`);
}

const admin = postgres({ ...parse(url), database: "postgres", onnotice: () => undefined });
await admin.unsafe(`drop database if exists ${database} with (force)`);
await admin.unsafe(`create database ${database}`);
await admin.end();

const client = postgres(url.href, { max: 1, onnotice: () => undefined });
await migrate(drizzle({ client }), {
  migrationsFolder: new URL("../drizzle", import.meta.url).pathname,
});
await client.end();

await mock.module("../src/lib/ai/providers", () => ({
  model: fakeModel,
  providerOptions: {},
  transcribe: fakeTranscribe,
}));
await mock.module("../src/lib/calls/openai", () => fakeVoice);
await mock.module("resend", () => ({ Resend: FakeResend }));
await mock.module("../src/lib/web/firecrawl", () => fakeFirecrawl);
await mock.module("../src/lib/files/s3", () => fakeStorage);
await mock.module("../src/lib/integrations/index", () => ({ integrations: fakeIntegrations }));

function parse(target: URL) {
  return {
    host: target.hostname,
    port: Number(target.port),
    username: decodeURIComponent(target.username),
    password: decodeURIComponent(target.password),
  };
}
