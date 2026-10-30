import { drizzle } from "drizzle-orm/postgres-js";
import postgres from "postgres";
import { env } from "../env";
import * as agentsSchema from "./schema/agents";
import * as authSchema from "./schema/auth";
import * as connectionsSchema from "./schema/connections";
import * as filesSchema from "./schema/files";

export const client = postgres(env.DATABASE_URL, {
  // A process paused inside a transaction would otherwise hold its row locks forever.
  connection: { idle_in_transaction_session_timeout: 30_000 },
});

export const db = drizzle({
  client,
  schema: { ...authSchema, ...agentsSchema, ...connectionsSchema, ...filesSchema },
  casing: "snake_case",
});

export type Transaction = Parameters<Parameters<typeof db.transaction>[0]>[0];
