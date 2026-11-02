import type { LanguageModelUsage, ModelMessage, UserModelMessage } from "ai";
import { sql } from "drizzle-orm";
import { bigint, boolean, index, jsonb, pgTable, text, timestamp, uuid } from "drizzle-orm/pg-core";
import { type AvatarParts, partCounts } from "../../lib/avatars/drawings";
import { users } from "./auth";

export const agents = pgTable(
  "agents",
  {
    id: uuid()
      .primaryKey()
      .default(sql`uuidv7()`),
    userId: text()
      .notNull()
      .references(() => users.id, { onDelete: "cascade" }),
    name: text().notNull(),
    instructions: text().notNull().default(""),
    avatarId: uuid(),
    // Random per row, so the migration gives agents made earlier their own faces too.
    avatarParts: jsonb()
      .$type<AvatarParts>()
      .notNull()
      .default(
        sql.raw(
          `jsonb_build_object('background', floor(random() * ${partCounts.background})::int, 'horns', floor(random() * ${partCounts.horns})::int, 'eyes', floor(random() * ${partCounts.eyes})::int)`,
        ),
      ),
    connectionsNote: text(),
    schedulesNote: text(),
    memoryNote: text(),
    // What the user asked to forget since the latest summary was written; the next step rewrites it.
    summaryRedactions: text()
      .array()
      .notNull()
      .default(sql`'{}'::text[]`),
    runOwner: uuid(),
    runLeaseUntil: timestamp({ withTimezone: true }),
    createdAt: timestamp({ withTimezone: true }).notNull().defaultNow(),
  },
  (table) => [
    index().on(table.userId),
    index()
      .on(table.runLeaseUntil)
      .where(sql`${table.runLeaseUntil} is not null`),
  ],
);

export const messages = pgTable(
  "messages",
  {
    id: bigint({ mode: "number" }).primaryKey().generatedAlwaysAsIdentity(),
    agentId: uuid()
      .notNull()
      .references(() => agents.id, { onDelete: "cascade" }),
    message: jsonb().$type<ModelMessage>().notNull(),
    hidden: boolean().notNull().default(false),
    model: text(),
    usage: jsonb().$type<LanguageModelUsage>(),
    createdAt: timestamp({ withTimezone: true }).notNull().defaultNow(),
  },
  (table) => [index().on(table.agentId, table.id)],
);

export const queuedMessages = pgTable(
  "queued_messages",
  {
    id: bigint({ mode: "number" }).primaryKey().generatedAlwaysAsIdentity(),
    agentId: uuid()
      .notNull()
      .references(() => agents.id, { onDelete: "cascade" }),
    message: jsonb().$type<UserModelMessage>().notNull(),
    hidden: boolean().notNull().default(false),
    createdAt: timestamp({ withTimezone: true }).notNull().defaultNow(),
  },
  (table) => [index().on(table.agentId)],
);
