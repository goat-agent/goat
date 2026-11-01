import type { ModelMessage } from "ai";
import { and, desc, eq, inArray, sql, type SQL } from "drizzle-orm";
import { db, type Transaction } from "../../db";
import { agents, messages } from "../../db/schema/agents";
import { rounds } from "../../db/schema/rounds";
import { textOf } from "../conversation";
import { logger } from "../logger";
import { embedQuery, embeddingModel } from "./embeddings";
import { normalize, tokensOf } from "./text";

export type Found = Pick<typeof rounds.$inferSelect, "id" | "startedAt" | "userText" | "replyText">;

type Scope = { agentId: string; cutId: number; after: Date | undefined; before: Date | undefined };

const MAX_CHARS = 8_000;
const MAX_TOKENS = 12;
const CANDIDATES = 200;
const NEIGHBORS = 50;
const RRF_K = 60;
// A token that is only a distant trigram match scores low; below this a round is not a lexical hit.
const LEXICAL_FLOOR = 0.3;
// The lexical list counts in full only when its best round has nearly every query word whole.
// Measured on a 36-day Korean chat: from 0.8 it ties with meaning alone; below 0.5 it costs rank 1.
const RAMP = { from: 0.8, to: 1.0 };

const log = logger.child({ module: "search" });

function capped(text: string): string {
  return text.length > MAX_CHARS ? text.slice(0, MAX_CHARS) : text;
}

function searchTextOf(userText: string, replyText: string): string {
  return normalize(`${userText}\n${replyText}`);
}

/** Starts a round for each visible user message that has text. `rows` are the inserted messages. */
export async function recordUserMessages(
  tx: Transaction,
  agentId: string,
  rows: readonly { id: number; message: ModelMessage; hidden: boolean; createdAt: Date }[],
): Promise<void> {
  const values = rows.flatMap(({ id, message, hidden, createdAt }) => {
    const userText = capped(textOf(message).trim());
    if (hidden || !userText) return [];
    return [
      {
        agentId,
        firstMessageId: id,
        lastMessageId: id,
        startedAt: createdAt,
        endedAt: createdAt,
        userText,
        replyText: "",
        searchText: searchTextOf(userText, ""),
      },
    ];
  });
  if (values.length > 0) await tx.insert(rounds).values(values);
}

/**
 * Adds what the agent said in `response` to the round of the user message it answers, and marks
 * the round for embedding again. `inserted` are the ids of the rows just saved.
 */
export async function recordReply(
  tx: Transaction,
  agentId: string,
  inserted: readonly number[],
  response: readonly ModelMessage[],
): Promise<void> {
  const reply = response
    .flatMap((message) => (message.role === "assistant" ? [textOf(message)] : []))
    .filter(Boolean)
    .join("\n\n");
  if (!reply) return;
  const firstNew = Math.min(...inserted);
  const lastNew = Math.max(...inserted);

  const [user] = await tx
    .select({ id: messages.id })
    .from(messages)
    .where(
      and(
        eq(messages.agentId, agentId),
        sql`${messages.id} < ${firstNew}`,
        sql`${messages.message}->>'role' = 'user'`,
      ),
    )
    .orderBy(desc(messages.id))
    .limit(1);
  const [round] = await tx
    .select()
    .from(rounds)
    .where(eq(rounds.agentId, agentId))
    .orderBy(desc(rounds.firstMessageId))
    .limit(1)
    .for("update");

  // A hidden user message, such as a connection note, starts no round of its own, so what the
  // agent answers to it is kept apart from the round before it.
  if (round && user && round.firstMessageId === user.id) {
    const replyText = capped(round.replyText ? `${round.replyText}\n\n${reply}` : reply);
    await tx
      .update(rounds)
      .set({
        replyText,
        searchText: searchTextOf(round.userText, replyText),
        lastMessageId: lastNew,
        endedAt: sql`now()`,
        ...staleEmbedding,
      })
      .where(eq(rounds.id, round.id));
    return;
  }
  const replyText = capped(reply);
  await tx.insert(rounds).values({
    agentId,
    firstMessageId: user?.id ?? firstNew,
    lastMessageId: lastNew,
    startedAt: sql`now()`,
    endedAt: sql`now()`,
    userText: "",
    replyText,
    searchText: searchTextOf("", replyText),
  });
}

/** One round for a delivered report: the schedule's title as the user's side, the report as the reply. */
export async function recordReport(
  tx: Transaction,
  agentId: string,
  report: { markerId: number; replyId: number; title: string; text: string; at: Date },
): Promise<void> {
  const userText = capped(report.title);
  const replyText = capped(report.text);
  await tx.insert(rounds).values({
    agentId,
    firstMessageId: report.markerId,
    lastMessageId: report.replyId,
    startedAt: report.at,
    endedAt: report.at,
    userText,
    replyText,
    searchText: searchTextOf(userText, replyText),
  });
}

const staleEmbedding = {
  embedding: null,
  embeddedModel: null,
  embedClaimedUntil: null,
  embedAttempts: 0,
  updatedAt: sql`now()`,
};

/** Replaces `terms` in every round of the user's agents, so forgotten text cannot be found again. */
export async function blank(
  tx: Transaction,
  userId: string,
  terms: readonly string[],
): Promise<void> {
  const owned = tx.select({ id: agents.id }).from(agents).where(eq(agents.userId, userId));
  for (const term of terms.map((candidate) => candidate.trim()).filter(Boolean)) {
    const pattern = new RegExp(term.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "gi");
    const like = `%${term.replace(/[\\%_]/g, "\\$&")}%`;
    const found = await tx
      .select({ id: rounds.id, userText: rounds.userText, replyText: rounds.replyText })
      .from(rounds)
      .where(
        and(
          inArray(rounds.agentId, owned),
          sql`(${rounds.userText} ilike ${like} or ${rounds.replyText} ilike ${like})`,
        ),
      );
    for (const round of found) {
      const userText = round.userText.replace(pattern, "[removed]");
      const replyText = round.replyText.replace(pattern, "[removed]");
      await tx
        .update(rounds)
        .set({
          userText,
          replyText,
          searchText: searchTextOf(userText, replyText),
          ...staleEmbedding,
        })
        .where(eq(rounds.id, round.id));
    }
  }
}

function scopeSql({ agentId, cutId, after, before }: Scope): SQL {
  const parts = [sql`r.agent_id = ${agentId}`, sql`r.first_message_id <= ${cutId}`];
  if (after) parts.push(sql`r.started_at >= ${after.toISOString()}::timestamptz`);
  if (before) parts.push(sql`r.started_at < ${before.toISOString()}::timestamptz`);
  return sql.join(parts, sql` and `);
}

type Scored = { id: number; score: number };

async function lexical(
  tx: Transaction,
  scope: Scope,
  tokens: readonly string[],
): Promise<Scored[]> {
  const list = sql.join(
    tokens.map((token) => sql`${token}`),
    sql`, `,
  );
  // The `<%` filter lets the trigram index find candidates; its threshold is set below.
  const candidate = sql.join(
    tokens.map((token) => sql`${token} <% r.search_text`),
    sql` or `,
  );
  const rows = await tx.execute<{ id: string; score: string }>(sql`
    select r.id, (
      select avg(greatest(
        word_similarity(t, r.search_text),
        case when position(' ' || t || ' ' in ' ' || r.search_text || ' ') > 0 then 1.0 else 0.0 end
      ))
      from unnest(array[${list}]::text[]) as t
    ) as score
    from rounds r
    where ${scopeSql(scope)} and (${candidate})
    order by score desc
    limit ${CANDIDATES}
  `);
  return rows
    .map((row) => ({ id: Number(row.id), score: Number(row.score) }))
    .filter((row) => row.score >= LEXICAL_FLOOR);
}

async function nearest(tx: Transaction, scope: Scope, query: readonly number[]): Promise<number[]> {
  const vector = `[${query.join(",")}]`;
  const rows = await tx.execute<{ id: string }>(sql`
    select id from (
      select r.id, r.embedding <=> ${vector}::vector as distance
      from rounds r
      where ${scopeSql(scope)} and r.embedding is not null and r.embedded_model = ${embeddingModel}
      order by distance
      limit ${NEIGHBORS}
    ) nearest
    order by distance
  `);
  return rows.map((row) => Number(row.id));
}

/**
 * The rounds that best answer `query`, best first, among those of `scope`. Lexical and semantic
 * matches are combined by reciprocal rank; the lexical side counts for less the weaker its best
 * match is, so a loosely worded question is answered by meaning and an exact name by the name.
 */
export async function search(input: Scope & { query: string; limit: number }): Promise<Found[]> {
  const tokens = tokensOf(input.query).slice(0, MAX_TOKENS);
  if (tokens.length === 0) return [];

  let query: number[] | undefined;
  try {
    query = await embedQuery(input.query.trim());
  } catch (error) {
    log.warn({ err: error }, "query was not embedded; using words only");
  }

  const ids = await db.transaction(async (tx) => {
    await tx.execute(sql`select set_config('pg_trgm.word_similarity_threshold', '0.3', true)`);
    await tx.execute(sql`select set_config('hnsw.iterative_scan', 'relaxed_order', true)`);
    const words = await lexical(tx, input, tokens);
    const meanings = query ? await nearest(tx, input, query) : [];
    return fuse(words, meanings).slice(0, input.limit);
  });
  if (ids.length === 0) return [];

  const found = await db
    .select({
      id: rounds.id,
      startedAt: rounds.startedAt,
      userText: rounds.userText,
      replyText: rounds.replyText,
    })
    .from(rounds)
    .where(inArray(rounds.id, ids));
  return ids.flatMap((id) => found.filter((round) => round.id === id));
}

function fuse(words: readonly Scored[], meanings: readonly number[]): number[] {
  const best = words[0]?.score ?? 0;
  // Without vectors, such as when embedding gave up, the words are all there is.
  const weight =
    meanings.length === 0 ? 1 : clamp((best - RAMP.from) / (RAMP.to - RAMP.from), 0, 1);
  const points = new Map<number, number>();
  const add = (id: number, value: number) => points.set(id, (points.get(id) ?? 0) + value);
  words.forEach(({ id }, rank) => {
    add(id, weight / (RRF_K + rank + 1));
  });
  meanings.forEach((id, rank) => {
    add(id, 1 / (RRF_K + rank + 1));
  });
  return [...points].toSorted((a, b) => b[1] - a[1]).map(([id]) => id);
}

function clamp(value: number, low: number, high: number): number {
  return Math.min(high, Math.max(low, value));
}
