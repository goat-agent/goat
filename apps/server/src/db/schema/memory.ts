import { sql } from "drizzle-orm";
import {
  bigint,
  date,
  index,
  pgTable,
  text,
  timestamp,
  uniqueIndex,
  uuid,
} from "drizzle-orm/pg-core";
import { agents } from "./agents";
import { users } from "./auth";

/** What the user's agents remember: one memory per account, shared by all of its agents. */
export const memoryPages = pgTable(
  "memory_pages",
  {
    id: uuid()
      .primaryKey()
      .default(sql`uuidv7()`),
    userId: text()
      .notNull()
      .references(() => users.id, { onDelete: "cascade" }),
    kind: text({ enum: ["me", "person", "place", "topic"] }).notNull(),
    title: text().notNull(),
    updatedAt: timestamp({ withTimezone: true }).notNull().defaultNow(),
  },
  (table) => [
    uniqueIndex("memory_pages_user_id_kind_title_unique").on(
      table.userId,
      table.kind,
      sql`lower(${table.title})`,
    ),
  ],
);

export const memoryEntries = pgTable(
  "memory_entries",
  {
    // The id the model sees.
    id: bigint({ mode: "number" }).primaryKey().generatedAlwaysAsIdentity(),
    pageId: uuid()
      .notNull()
      .references(() => memoryPages.id, { onDelete: "cascade" }),
    text: text().notNull(),
    // The first day the fact is no longer true.
    until: date({ mode: "string" }),
    pastAt: timestamp({ withTimezone: true }),
    replacedBy: bigint({ mode: "number" }),
    sourceAgentId: uuid().references(() => agents.id, { onDelete: "set null" }),
    createdAt: timestamp({ withTimezone: true }).notNull().defaultNow(),
  },
  (table) => [index().on(table.pageId)],
);
