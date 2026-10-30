import { sql } from "drizzle-orm";
import { index, pgTable, text, timestamp, uuid } from "drizzle-orm/pg-core";
import { agents } from "./agents";

export const files = pgTable(
  "files",
  {
    id: uuid()
      .primaryKey()
      .default(sql`uuidv7()`),
    // Deleting the agent only unlinks its files; the sweeper removes the stored bytes, then the row.
    agentId: uuid().references(() => agents.id, { onDelete: "set null" }),
    name: text().notNull(),
    mediaType: text().notNull(),
    sentAt: timestamp({ withTimezone: true }),
    createdAt: timestamp({ withTimezone: true }).notNull().defaultNow(),
  },
  (table) => [
    index().on(table.agentId),
    index()
      .on(table.createdAt)
      .where(sql`${table.sentAt} is null or ${table.agentId} is null`),
  ],
);
