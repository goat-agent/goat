import { and, asc, eq, inArray, sql } from "drizzle-orm";
import { db, type Transaction } from "../../db";
import { agents } from "../../db/schema/agents";
import { users } from "../../db/schema/auth";
import { memoryEntries, memoryPages } from "../../db/schema/memory";
import { dateIn } from "../dates";
import * as search from "../search";

const MAX_CURRENT_ME_ENTRIES = 60;

type Kind = typeof memoryPages.$inferSelect.kind;
type Entry = typeof memoryEntries.$inferSelect;
type Page = { kind: Kind; title: string };
type Who = { userId: string; timeZone: string };
type Writer = Who & { agentId: string };

const GROUPS = [
  { group: "people", kind: "person", label: "People" },
  { group: "places", kind: "place", label: "Places" },
  { group: "topics", kind: "topic", label: "Topics" },
] as const;

export class MemoryError extends Error {}

/** The page names the model uses: "me", or a group and a title such as "people/민지 (동생)". */
function parsePage(name: string): Page {
  const trimmed = name.trim();
  if (trimmed === "me") return { kind: "me", title: "me" };
  const match = /^(people|places|topics)\/(.+)$/s.exec(trimmed);
  const group = GROUPS.find((candidate) => candidate.group === match?.[1]);
  const title = match?.[2]?.trim();
  if (!group || !title) {
    throw new MemoryError(
      `Unknown page "${name}". Use "me", "people/<title>", "places/<title>", or "topics/<title>".`,
    );
  }
  return { kind: group.kind, title };
}

function nameOf(page: Page): string {
  const group = GROUPS.find((candidate) => candidate.kind === page.kind);
  return group ? `${group.group}/${page.title}` : "me";
}

function isPast(entry: Pick<Entry, "pastAt" | "until">, today: string): boolean {
  return entry.pastAt !== null || (entry.until !== null && entry.until < today);
}

/** Entries of a page as the model reads them, past ones included. */
export async function read(who: Who, name: string): Promise<string> {
  const page = parsePage(name);
  const found = await findPage(db, who.userId, page);
  if (!found) return "No such page.";
  const entries = await db
    .select()
    .from(memoryEntries)
    .where(eq(memoryEntries.pageId, found.id))
    .orderBy(asc(memoryEntries.id));
  if (entries.length === 0) return "No entries.";
  const today = dateIn(who.timeZone, new Date());
  return entries
    .map((entry) => {
      const facts = [`saved ${dateIn(who.timeZone, entry.createdAt)}`];
      if (entry.until) facts.push(`until ${entry.until}`);
      if (isPast(entry, today)) facts.push("past");
      return `${entry.id}: ${entry.text} (${facts.join(", ")})`;
    })
    .join("\n");
}

export async function save(
  writer: Writer,
  input: { page: string; text: string; replaces: readonly number[]; until: string | undefined },
): Promise<number> {
  const page = parsePage(input.page);
  return write(writer, async (tx) => {
    const found =
      (await findPage(tx, writer.userId, page)) ??
      (await tx
        .insert(memoryPages)
        .values({ userId: writer.userId, ...page })
        .returning()
        .then((rows) => rows[0]));
    if (!found) throw new Error("page insert returned no row");

    const today = dateIn(writer.timeZone, new Date());
    const onPage = await tx.select().from(memoryEntries).where(eq(memoryEntries.pageId, found.id));
    for (const id of input.replaces) {
      if (!onPage.some((entry) => entry.id === id)) {
        throw new MemoryError(
          `Entry ${id} is not on ${nameOf(page)}. Open the page to see its ids.`,
        );
      }
    }
    if (page.kind === "me") {
      const current = onPage.filter((entry) => !isPast(entry, today));
      const replaced = current.filter((entry) => input.replaces.includes(entry.id));
      if (current.length - replaced.length + 1 > MAX_CURRENT_ME_ENTRIES) {
        throw new MemoryError(
          `me is full (${MAX_CURRENT_ME_ENTRIES} current entries). Make room: merge entries about the same thing into one with replaces, or forget an outdated one, only as many as you need, then save again. Keep standing instructions.`,
        );
      }
    }

    const [entry] = await tx
      .insert(memoryEntries)
      .values({
        pageId: found.id,
        text: input.text,
        until: input.until ?? null,
        sourceAgentId: writer.agentId,
      })
      .returning({ id: memoryEntries.id });
    if (!entry) throw new Error("entry insert returned no row");
    if (input.replaces.length > 0) {
      await tx
        .update(memoryEntries)
        .set({ pastAt: sql`now()`, replacedBy: entry.id })
        .where(
          and(inArray(memoryEntries.id, [...input.replaces]), sql`${memoryEntries.pastAt} is null`),
        );
    }
    await tx
      .update(memoryPages)
      .set({ updatedAt: sql`now()` })
      .where(eq(memoryPages.id, found.id));
    return entry.id;
  });
}

/** Deletes one entry, or a whole page; exactly one of `id` and `page` is given. */
export async function forget(
  writer: Writer,
  input: { id: number | undefined; page: string | undefined },
): Promise<string> {
  if ((input.id === undefined) === (input.page === undefined)) {
    throw new MemoryError("Give either an entry id or a page, not both.");
  }
  return write(writer, async (tx) => {
    if (input.page !== undefined) {
      const page = parsePage(input.page);
      const found = await findPage(tx, writer.userId, page);
      if (!found) return "No such page.";
      const entries = await tx
        .select({ text: memoryEntries.text })
        .from(memoryEntries)
        .where(eq(memoryEntries.pageId, found.id));
      await tx.delete(memoryPages).where(eq(memoryPages.id, found.id));
      await redact(
        tx,
        writer.userId,
        entries.map(({ text }) => text),
        nameOf(page),
      );
      return "Forgotten.";
    }
    const [entry] = await tx
      .select({
        id: memoryEntries.id,
        text: memoryEntries.text,
        pageId: memoryPages.id,
        kind: memoryPages.kind,
      })
      .from(memoryEntries)
      .innerJoin(memoryPages, eq(memoryPages.id, memoryEntries.pageId))
      .where(and(eq(memoryEntries.id, input.id ?? 0), eq(memoryPages.userId, writer.userId)));
    if (!entry) return "No such entry.";
    await tx.delete(memoryEntries).where(eq(memoryEntries.id, entry.id));
    const [left] = await tx
      .select({ id: memoryEntries.id })
      .from(memoryEntries)
      .where(eq(memoryEntries.pageId, entry.pageId))
      .limit(1);
    if (!left && entry.kind !== "me") {
      await tx.delete(memoryPages).where(eq(memoryPages.id, entry.pageId));
    }
    await redact(tx, writer.userId, [entry.text]);
    return "Forgotten.";
  });
}

// Forgotten text must not come back from the rounds search finds, or from the summaries the
// user's agents already hold. Only entry texts are blanked in rounds; the page name is no one's
// words, so it goes to the summary rewrite alone.
async function redact(
  tx: Transaction,
  userId: string,
  texts: readonly string[],
  page?: string,
): Promise<void> {
  await search.blank(tx, userId, texts);
  const subjects = page === undefined ? texts : [page, ...texts];
  if (subjects.length === 0) return;
  await tx
    .update(agents)
    .set({
      summaryRedactions: sql`array_cat(${agents.summaryRedactions}, array[${sql.join(
        subjects.map((subject) => sql`${subject}`),
        sql`, `,
      )}]::text[])`,
    })
    .where(
      and(
        eq(agents.userId, userId),
        sql`exists (select 1 from summaries where summaries.agent_id = ${agents.id})`,
      ),
    );
}

export async function rename(writer: Writer, name: string, title: string): Promise<string> {
  const page = parsePage(name);
  const renamed: Page = { kind: page.kind, title: title.trim() };
  if (page.kind === "me") throw new MemoryError('"me" cannot be renamed.');
  if (!renamed.title) throw new MemoryError("Give the new title.");
  return write(writer, async (tx) => {
    const found = await findPage(tx, writer.userId, page);
    if (!found) return "No such page.";
    const taken = await findPage(tx, writer.userId, renamed);
    if (taken && taken.id !== found.id) {
      throw new MemoryError(`${nameOf(renamed)} already exists. Save to that page instead.`);
    }
    await tx
      .update(memoryPages)
      .set({ title: renamed.title, updatedAt: sql`now()` })
      .where(eq(memoryPages.id, found.id));
    return `Renamed to ${nameOf(renamed)}.`;
  });
}

/**
 * The note an agent gets when the memory changed without it, or `undefined` if nothing changed.
 * Remembers what it sent in `agents.memory_note`.
 */
export async function noteChange(
  tx: Transaction,
  agentId: string,
  who: Who,
  previous: string | null,
): Promise<string | undefined> {
  const note = await render(tx, who);
  if (previous === null && note.isEmpty) return undefined;
  if (note.text === previous) return undefined;
  await tx.update(agents).set({ memoryNote: note.text }).where(eq(agents.id, agentId));
  return note.text;
}

/** The note for a run that has no conversation to keep up to date, or `undefined` when empty. */
export async function noteFor(who: Who): Promise<string | undefined> {
  const note = await render(db, who);
  return note.isEmpty ? undefined : note.text;
}

// Writes by different agents of one user queue behind each other, so the 60-entry cap and page
// creation cannot race. An agent's own writes bring its note up to date unless it was already
// behind, in which case it still gets the whole change on its next message.
async function write<T>(writer: Writer, change: (tx: Transaction) => Promise<T>): Promise<T> {
  return db.transaction(async (tx) => {
    await tx.execute(
      sql`select pg_advisory_xact_lock(hashtextextended(${`memory:${writer.userId}`}, 0))`,
    );
    const before = await render(tx, writer);
    const result = await change(tx);
    const [agent] = await tx
      .select({ memoryNote: agents.memoryNote })
      .from(agents)
      .where(eq(agents.id, writer.agentId));
    if (!agent) throw new Error("agent no longer exists");
    const wasCurrent =
      agent.memoryNote === before.text || (agent.memoryNote === null && before.isEmpty);
    if (wasCurrent) {
      const after = await render(tx, writer);
      await tx
        .update(agents)
        .set({ memoryNote: agent.memoryNote === null && after.isEmpty ? null : after.text })
        .where(eq(agents.id, writer.agentId));
    }
    return result;
  });
}

async function findPage(
  executor: Transaction | typeof db,
  userId: string,
  page: Page,
): Promise<typeof memoryPages.$inferSelect | undefined> {
  const [found] = await executor
    .select()
    .from(memoryPages)
    .where(
      and(
        eq(memoryPages.userId, userId),
        eq(memoryPages.kind, page.kind),
        sql`lower(${memoryPages.title}) = lower(${page.title})`,
      ),
    );
  return found;
}

const collator = new Intl.Collator("ko");

async function render(
  executor: Transaction | typeof db,
  who: Who,
): Promise<{ text: string; isEmpty: boolean }> {
  const [user] = await executor
    .select({ name: users.name })
    .from(users)
    .where(eq(users.id, who.userId));
  const name = user?.name ?? "the user";
  const pages = await executor
    .select({ id: memoryPages.id, kind: memoryPages.kind, title: memoryPages.title })
    .from(memoryPages)
    .where(eq(memoryPages.userId, who.userId));
  const mePage = pages.find((page) => page.kind === "me");
  const today = dateIn(who.timeZone, new Date());
  const mine = mePage
    ? await executor
        .select()
        .from(memoryEntries)
        .where(eq(memoryEntries.pageId, mePage.id))
        .orderBy(asc(memoryEntries.id))
    : [];

  const lines: string[] = [];
  const current = mine.filter((entry) => !isPast(entry, today));
  if (current.length > 0) {
    lines.push('"me" entries (id: fact):');
    for (const entry of current) {
      lines.push(`- ${entry.id}: ${entry.text}${entry.until ? ` (until ${entry.until})` : ""}`);
    }
  }
  const groups = GROUPS.flatMap(({ kind, label }) => {
    const titles = pages
      .filter((page) => page.kind === kind)
      .map((page) => page.title)
      .toSorted(collator.compare);
    return titles.length > 0 ? [`${label} (${titles.length}): ${titles.join(", ")}`] : [];
  });
  if (groups.length > 0) {
    lines.push("Other pages, opened with memory_read as kind/title, e.g. people/민지 (동생):");
    lines.push(...groups);
  }
  if (lines.length === 0) return { text: `Memory of ${name}: empty.`, isEmpty: true };
  return {
    text: [`Memory of ${name} (shared by all of ${name}'s agents).`, ...lines].join("\n"),
    isEmpty: false,
  };
}
