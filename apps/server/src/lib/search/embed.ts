import { and, eq, inArray, isNull, lt, or, sql } from "drizzle-orm";
import { db } from "../../db";
import { rounds } from "../../db/schema/rounds";
import { logger } from "../logger";
import { embedDocuments, embeddingModel } from "./embeddings";

const BATCH = 8;
const IDLE_MS = 5_000;
const INPUT_CHARS = 1_500;

// Tests lower `settleSeconds` to embed what they just wrote.
export const timing = { settleSeconds: 30, claimSeconds: 120, backoffSeconds: 60, maxAttempts: 5 };

const log = logger.child({ module: "embedding" });
const shutdown = new AbortController();
let task: Promise<void> | undefined;

export function startEmbedding(): void {
  task ??= loop();
}

export async function stopEmbedding(): Promise<void> {
  shutdown.abort();
  await task;
}

async function loop(): Promise<void> {
  while (!shutdown.signal.aborted) {
    let embedded = 0;
    try {
      embedded = await embedPending();
    } catch (error) {
      log.error({ err: error }, "embedding pass failed");
    }
    if (embedded === 0) await sleep(IDLE_MS * (0.5 + Math.random()));
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, ms);
    shutdown.signal.addEventListener(
      "abort",
      () => {
        clearTimeout(timer);
        resolve();
      },
      { once: true },
    );
  });
}

/**
 * Embeds one batch of rounds that have no current vector and have not changed lately, and
 * returns how many it wrote. Rounds are claimed first and embedded outside any transaction.
 */
export async function embedPending(): Promise<number> {
  const claimed = await claim();
  if (claimed.length === 0) return 0;

  let vectors: number[][];
  try {
    vectors = await embedDocuments(
      claimed.map(({ userText, replyText }) => inputOf(userText, replyText)),
    );
  } catch (error) {
    await fail(claimed);
    log.error({ err: error, rounds: claimed.length }, "embedding failed");
    return 0;
  }

  let written = 0;
  for (const [index, round] of claimed.entries()) {
    const vector = vectors[index];
    if (!vector) continue;
    // A round that changed since the claim is left for the next pass.
    const rows = await db
      .update(rounds)
      .set({
        embedding: vector,
        embeddedModel: embeddingModel,
        embedClaimedUntil: null,
        embedAttempts: 0,
      })
      .where(and(eq(rounds.id, round.id), sql`${rounds.updatedAt}::text = ${round.version}`))
      .returning({ id: rounds.id });
    written += rows.length;
  }
  log.info({ claimed: claimed.length, written }, "embedded");
  return written;
}

function inputOf(userText: string, replyText: string): string {
  return `user: ${userText.slice(0, INPUT_CHARS)}\nagent: ${replyText.slice(0, INPUT_CHARS)}`;
}

// `version` is updated_at as text: a Date would drop the microseconds the comparison needs.
type Claimed = { id: number; userText: string; replyText: string; version: string };

async function claim(): Promise<Claimed[]> {
  const due = db
    .select({ id: rounds.id })
    .from(rounds)
    .where(
      and(
        or(
          isNull(rounds.embedding),
          sql`${rounds.embeddedModel} is distinct from ${embeddingModel}`,
        ),
        lt(rounds.embedAttempts, timing.maxAttempts),
        lt(rounds.updatedAt, sql`now() - ${timing.settleSeconds} * interval '1 second'`),
        or(isNull(rounds.embedClaimedUntil), lt(rounds.embedClaimedUntil, sql`now()`)),
        sql`(${rounds.userText} <> '' or ${rounds.replyText} <> '')`,
      ),
    )
    .orderBy(rounds.id)
    .limit(BATCH)
    .for("update", { skipLocked: true });
  return db
    .update(rounds)
    .set({ embedClaimedUntil: sql`now() + ${timing.claimSeconds} * interval '1 second'` })
    .where(inArray(rounds.id, due))
    .returning({
      id: rounds.id,
      userText: rounds.userText,
      replyText: rounds.replyText,
      version: sql<string>`${rounds.updatedAt}::text`,
    });
}

// Each failure doubles the wait; the fifth leaves the round with words-only search.
async function fail(claimed: readonly Claimed[]): Promise<void> {
  for (const round of claimed) {
    await db
      .update(rounds)
      .set({
        embedAttempts: sql`${rounds.embedAttempts} + 1`,
        embedClaimedUntil: sql`now() + ${timing.backoffSeconds} * power(2, ${rounds.embedAttempts}) * interval '1 second'`,
      })
      .where(and(eq(rounds.id, round.id), sql`${rounds.updatedAt}::text = ${round.version}`));
  }
}
