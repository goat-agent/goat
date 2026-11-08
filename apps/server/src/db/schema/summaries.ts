import type { LanguageModelUsage } from "ai";
import {
  bigint,
  boolean,
  index,
  integer,
  jsonb,
  pgTable,
  text,
  timestamp,
  uuid,
} from "drizzle-orm/pg-core";
import { agents } from "./agents";

export type SummaryNotes = {
  connections?: string;
  schedules?: string;
  memory?: string;
  date: string;
};

/** Replaces the messages up to `uptoMessageId` in what the model is sent. The latest per agent wins. */
export const summaries = pgTable(
  "summaries",
  {
    id: bigint({ mode: "number" }).primaryKey().generatedAlwaysAsIdentity(),
    agentId: uuid()
      .notNull()
      .references(() => agents.id, { onDelete: "cascade" }),
    uptoMessageId: bigint({ mode: "number" }).notNull(),
    text: text().notNull(),
    // The state restated after the cut, because the messages that announced it are gone.
    notes: jsonb().$type<SummaryNotes>().notNull(),
    // Computer window ids (w3) up to the cut, so later ones never reuse them.
    computerIdHigh: integer().notNull(),
    // False when summarizing kept failing and the text only marks what is missing.
    complete: boolean().notNull(),
    model: text().notNull(),
    usage: jsonb().$type<LanguageModelUsage>(),
    createdAt: timestamp({ withTimezone: true }).notNull().defaultNow(),
  },
  (table) => [index().on(table.agentId, table.id)],
);
