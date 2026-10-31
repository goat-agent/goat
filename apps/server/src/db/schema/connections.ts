import { sql } from "drizzle-orm";
import { boolean, pgTable, text, timestamp, unique, uuid } from "drizzle-orm/pg-core";
import { agents } from "./agents";
import { users } from "./auth";

export const connections = pgTable(
  "connections",
  {
    id: uuid()
      .primaryKey()
      .default(sql`uuidv7()`),
    userId: text()
      .notNull()
      .references(() => users.id, { onDelete: "cascade" }),
    integrationId: text().notNull(),
    accountId: text().notNull(),
    accountName: text().notNull(),
    credentials: text().notNull(),
    needsReconnect: boolean().notNull().default(false),
    createdAt: timestamp({ withTimezone: true }).notNull().defaultNow(),
  },
  (table) => [unique().on(table.userId, table.integrationId, table.accountId)],
);

export const connectionAttempts = pgTable("connection_attempts", {
  state: text().primaryKey(),
  userId: text()
    .notNull()
    .references(() => users.id, { onDelete: "cascade" }),
  integrationId: text().notNull(),
  agentId: uuid().references(() => agents.id, { onDelete: "cascade" }),
  codeVerifier: text().notNull(),
  returnsToWeb: boolean().notNull().default(false),
  createdAt: timestamp({ withTimezone: true }).notNull().defaultNow(),
});
