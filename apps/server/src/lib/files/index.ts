import type { ModelMessage, TextPart, ToolContent, UserContent } from "ai";
import { and, asc, eq, inArray, isNull, lt, or, sql } from "drizzle-orm";
import { db, type Transaction } from "../../db";
import { files } from "../../db/schema/files";
import { isFinalReply } from "../conversation";
import { logger } from "../logger";
import * as storage from "./s3";

export type StoredFile = typeof files.$inferSelect;

/** What a message or a tool result holds in place of a file's bytes. */
export type FileRef = Pick<StoredFile, "id" | "name" | "mediaType">;

type ModelPart =
  | { type: "text"; text: string }
  | { type: "file"; data: { type: "data"; data: Uint8Array }; mediaType: string; filename: string };

type Format = {
  mediaType: string;
  maxBytes: number;
  matches: (bytes: Uint8Array, name: string) => boolean;
  toModel: (file: FileRef, bytes: Uint8Array) => ModelPart;
};

export class FileRejected extends Error {}
export class FileNotFound extends Error {}

const MB = 1024 * 1024;
const SCHEME = "goat-file:";
const UNSENT_TTL = sql`interval '1 day'`;
const SWEEP_INTERVAL_MS = 10 * 60_000;
const SWEEP_BATCH = 100;
const NAME_MAX = 255;

const log = logger.child({ module: "files" });

function signature(offset: number, ascii: string): (bytes: Uint8Array) => boolean {
  const expected = Uint8Array.from(ascii, (char) => char.charCodeAt(0));
  return (bytes) => expected.every((byte, i) => bytes[offset + i] === byte);
}

const TEXT_CONTROLS = new Set([0x09, 0x0a, 0x0c, 0x0d]);

function isText(bytes: Uint8Array): boolean {
  if (bytes.some((byte) => (byte < 0x20 && !TEXT_CONTROLS.has(byte)) || byte === 0x7f)) {
    return false;
  }
  try {
    new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    return true;
  } catch {
    return false;
  }
}

function named(pattern: RegExp): (bytes: Uint8Array, name: string) => boolean {
  return (bytes, name) => pattern.test(name) && isText(bytes);
}

function asFile(file: FileRef, bytes: Uint8Array): ModelPart {
  return {
    type: "file",
    data: { type: "data", data: bytes },
    mediaType: file.mediaType,
    filename: file.name,
  };
}

function asText(file: FileRef, bytes: Uint8Array): ModelPart {
  return { type: "text", text: `File "${file.name}":\n\n${new TextDecoder().decode(bytes)}` };
}

// Claude takes images up to 10 MB once base64-encoded.
const image = { maxBytes: 7.5 * MB, toModel: asFile };
const text = { maxBytes: 1 * MB, toModel: asText };

// The type comes from the bytes, never from the client, and the first match wins. Text types
// share their bytes, so the file name tells them apart and plain text comes last.
const formats: Format[] = [
  { mediaType: "image/jpeg", ...image, matches: signature(0, "\xff\xd8\xff") },
  { mediaType: "image/png", ...image, matches: signature(0, "\x89PNG\r\n\x1a\n") },
  { mediaType: "image/gif", ...image, matches: signature(0, "GIF8") },
  {
    mediaType: "image/webp",
    ...image,
    matches: (bytes) => signature(0, "RIFF")(bytes) && signature(8, "WEBP")(bytes),
  },
  {
    mediaType: "application/pdf",
    maxBytes: 32 * MB,
    matches: signature(0, "%PDF-"),
    toModel: asFile,
  },
  { mediaType: "text/markdown", ...text, matches: named(/\.(md|markdown)$/i) },
  { mediaType: "text/csv", ...text, matches: named(/\.csv$/i) },
  { mediaType: "text/plain", ...text, matches: isText },
];

export const maxUploadBytes = Math.max(...formats.map(({ maxBytes }) => maxBytes));

function formatOf(mediaType: string): Format | undefined {
  return formats.find((format) => format.mediaType === mediaType);
}

export async function upload(agentId: string, file: File): Promise<StoredFile> {
  const bytes = await file.bytes();
  const name = file.name.trim().slice(0, NAME_MAX) || "file";
  const format = bytes.length > 0 ? formats.find(({ matches }) => matches(bytes, name)) : undefined;
  if (!format) throw new FileRejected("unsupported file type");
  if (bytes.length > format.maxBytes) throw new FileRejected("file too large");

  const { mediaType } = format;
  // The row goes first so the sweeper always knows about bytes in storage, even if this upload dies.
  const [stored] = await db.insert(files).values({ agentId, name, mediaType }).returning();
  if (!stored) throw new Error("file insert returned no row");
  await storage.put(stored.id, bytes, mediaType);
  log.info({ fileId: stored.id, agentId, mediaType, bytes: bytes.length }, "uploaded");
  return stored;
}

export async function find(agentId: string, id: string): Promise<StoredFile | undefined> {
  const [stored] = await db
    .select()
    .from(files)
    .where(and(eq(files.id, id), eq(files.agentId, agentId)));
  return stored;
}

export function read(id: string): Promise<Uint8Array> {
  return storage.get(id);
}

export function open(id: string): Blob {
  return storage.open(id);
}

/** Marks the files as sent with a message; throws unless every id is a file of this agent. */
export async function attach(
  tx: Transaction,
  agentId: string,
  ids: readonly string[],
): Promise<StoredFile[]> {
  if (ids.length === 0) return [];
  const rows = await tx
    .update(files)
    .set({ sentAt: sql`coalesce(${files.sentAt}, now())` })
    .where(and(eq(files.agentId, agentId), inArray(files.id, [...ids])))
    .returning();
  const attached = ids.flatMap((id) => rows.filter((row) => row.id === id));
  if (attached.length !== ids.length) throw new FileNotFound("file not found");
  return attached;
}

export function reference(file: FileRef): {
  type: "file";
  data: { type: "url"; url: URL };
  mediaType: string;
  filename: string;
} {
  return {
    type: "file",
    data: { type: "url", url: new URL(`${SCHEME}${file.id}`) },
    mediaType: file.mediaType,
    filename: file.name,
  };
}

export function referenced(part: {
  type: string;
  data?: unknown;
  mediaType?: string;
  filename?: string;
}): FileRef | undefined {
  if (part.type !== "file" || !isRef(part.data)) return undefined;
  return {
    id: String(part.data.url).slice(SCHEME.length),
    name: part.filename ?? "",
    mediaType: part.mediaType ?? "",
  };
}

// Stored messages come back from JSON, so the URL is a string by then.
function isRef(data: unknown): data is { type: "url"; url: URL | string } {
  if (typeof data !== "object" || data === null || !("url" in data)) return false;
  return String(data.url).startsWith(SCHEME);
}

export function attachments(content: UserContent): FileRef[] {
  return typeof content === "string" ? [] : content.flatMap((part) => referenced(part) ?? []);
}

/**
 * Gives the model whole files only for the current turn, everything after the agent's last final
 * reply. Earlier files become a note pointing at read_file: a conversation never ends and is
 * resent on every step, so keeping old files whole would grow every request without bound.
 */
export async function forModel(
  history: readonly ModelMessage[],
  cache: Map<string, Uint8Array>,
): Promise<ModelMessage[]> {
  const turnStart = history.findLastIndex(isFinalReply) + 1;
  const load = async (file: FileRef): Promise<ModelPart> => {
    let bytes = cache.get(file.id);
    if (!bytes) {
      bytes = await read(file.id);
      cache.set(file.id, bytes);
    }
    const format = formatOf(file.mediaType);
    if (!format) throw new Error(`no format for stored file ${file.id}`);
    return format.toModel(file, bytes);
  };

  return Promise.all(
    history.map(async (message, index) => {
      const resolve = async (file: FileRef): Promise<ModelPart> =>
        index >= turnStart ? load(file) : note(file);
      if (message.role === "user" && typeof message.content !== "string") {
        const content: UserContent = await Promise.all(
          message.content.map(async (part) => {
            const file = referenced(part);
            return file ? resolve(file) : part;
          }),
        );
        return { ...message, content };
      }
      if (message.role === "tool") {
        const content: ToolContent = await Promise.all(
          message.content.map(async (part) => {
            if (part.type !== "tool-result" || part.output.type !== "content") return part;
            const value = await Promise.all(
              part.output.value.map(async (item) => {
                const file = referenced(item);
                return file ? resolve(file) : item;
              }),
            );
            return { ...part, output: { ...part.output, value } };
          }),
        );
        return { ...message, content };
      }
      return message;
    }),
  );
}

function note(file: FileRef): TextPart {
  return {
    type: "text",
    text: `[File "${file.name}" (${file.mediaType}) was shared earlier. To see it again, call read_file with id ${file.id}.]`,
  };
}

let sweepTimer: Timer | undefined;

export function startSweeping(): void {
  const schedule = () => {
    const jittered = SWEEP_INTERVAL_MS * (0.5 + Math.random());
    sweepTimer = setTimeout(() => {
      sweep()
        .catch((error: unknown) => log.error({ err: error }, "sweep failed"))
        .finally(schedule);
    }, jittered);
  };
  schedule();
}

export function stopSweeping(): void {
  clearTimeout(sweepTimer);
}

/** Deletes files no agent holds and files never sent; bytes first, so a failure is retried. */
export async function sweep(): Promise<void> {
  const isDoomed = or(
    isNull(files.agentId),
    and(isNull(files.sentAt), lt(files.createdAt, sql`now() - ${UNSENT_TTL}`)),
  );
  const candidates = await db
    .select({ id: files.id })
    .from(files)
    .where(isDoomed)
    .orderBy(asc(files.createdAt))
    .limit(SWEEP_BATCH);

  let swept = 0;
  for (const { id } of candidates) {
    // The row lock makes a send that races the sweep wait, then find the file gone.
    await db.transaction(async (tx) => {
      const [row] = await tx
        .select({ id: files.id })
        .from(files)
        .where(and(eq(files.id, id), isDoomed))
        .for("update", { skipLocked: true });
      if (!row) return;
      await storage.remove(id);
      await tx.delete(files).where(eq(files.id, id));
      swept += 1;
    });
  }
  if (swept > 0) log.info({ count: swept }, "swept");
}
