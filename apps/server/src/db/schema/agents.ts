import type { LanguageModelUsage, ModelMessage, UserModelMessage } from "ai";
import { sql } from "drizzle-orm";
import { bigint, boolean, index, jsonb, pgTable, text, timestamp, uuid } from "drizzle-orm/pg-core";
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
    connectionsNote: text(),
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
