import { sql } from "drizzle-orm";
import {
  bigint,
  index,
  integer,
  pgTable,
  text,
  timestamp,
  uniqueIndex,
  uuid,
  vector,
} from "drizzle-orm/pg-core";
import { agents } from "./agents";

export const EMBEDDING_DIMENSIONS = 768;

/**
 * One user message and the agent's reply to it: the unit conversation search finds. Written in
 * the same transactions as the messages, so a round never lags behind the conversation.
 */
export const rounds = pgTable(
  "rounds",
  {
    id: bigint({ mode: "number" }).primaryKey().generatedAlwaysAsIdentity(),
    agentId: uuid()
      .notNull()
      .references(() => agents.id, { onDelete: "cascade" }),
    firstMessageId: bigint({ mode: "number" }).notNull(),
    lastMessageId: bigint({ mode: "number" }).notNull(),
    startedAt: timestamp({ withTimezone: true }).notNull(),
    endedAt: timestamp({ withTimezone: true }).notNull(),
    userText: text().notNull(),
    replyText: text().notNull(),
    // userText and replyText as `normalize` writes them, for the trigram index.
    searchText: text().notNull(),
    embedding: vector({ dimensions: EMBEDDING_DIMENSIONS }),
    embeddedModel: text(),
    // While in the future, another worker holds the row or its last attempt failed.
    embedClaimedUntil: timestamp({ withTimezone: true }),
    embedAttempts: integer().notNull().default(0),
    // Every change to the texts sets it, so a worker can tell its embedding is out of date.
    updatedAt: timestamp({ withTimezone: true }).notNull().defaultNow(),
  },
  (table) => [
    uniqueIndex().on(table.agentId, table.firstMessageId),
    index().on(table.agentId, table.startedAt),
    index().using("gin", table.searchText.op("gin_trgm_ops")),
    index()
      .using("hnsw", table.embedding.op("vector_cosine_ops"))
      .where(sql`${table.embedding} is not null`),
  ],
);
