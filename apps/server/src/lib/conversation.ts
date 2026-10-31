import type { ModelMessage } from "ai";
import { and, desc, eq } from "drizzle-orm";
import { db } from "../db";
import { messages } from "../db/schema/agents";

export type Line = { role: "user" | "assistant"; text: string };

const BUBBLE_BREAK = /^[ \t]*---[ \t]*$/;
const CODE_FENCE = /^[ \t]*(`{3,}|~{3,})/;

/** The text a person sees in a message: the user's own words, or the agent's reply. */
export function textOf(message: ModelMessage): string {
  if (message.role === "user") {
    const { content } = message;
    // The server puts its own parts first, so the user's text, if any, is the last part.
    const last = typeof content === "string" ? content : content.at(-1);
    return typeof last === "string" ? last : last?.type === "text" ? last.text : "";
  }
  return bubblesOf(message).join("\n\n");
}

/** True for an assistant message that calls no tool, which ends the agent's turn. */
export function isFinalReply(message: ModelMessage): boolean {
  return (
    message.role === "assistant" &&
    (typeof message.content === "string" ||
      message.content.every((part) => part.type !== "tool-call"))
  );
}

/** The agent's reply as chat bubbles, split where it wrote a line holding only `---`. */
export function bubblesOf(message: ModelMessage): string[] {
  if (message.role !== "assistant") return [];
  const text =
    typeof message.content === "string"
      ? message.content
      : message.content.flatMap((part) => (part.type === "text" ? [part.text] : [])).join("");
  const bubbles: string[][] = [[]];
  let fence: string | undefined;
  for (const line of text.split("\n")) {
    const marker = CODE_FENCE.exec(line)?.[1];
    if (fence === undefined && marker) fence = marker;
    else if (fence !== undefined && marker?.startsWith(fence)) fence = undefined;
    if (fence === undefined && BUBBLE_BREAK.test(line)) bubbles.push([]);
    else bubbles.at(-1)?.push(line);
  }
  return bubbles.map((lines) => lines.join("\n").trim()).filter(Boolean);
}

/** The latest lines with text, oldest first, out of the last `limit` messages. */
export async function recentLines(agentId: string, limit: number): Promise<Line[]> {
  const rows = await db
    .select({ message: messages.message })
    .from(messages)
    .where(and(eq(messages.agentId, agentId), eq(messages.hidden, false)))
    .orderBy(desc(messages.id))
    .limit(limit);
  return rows.toReversed().flatMap(({ message }) => {
    const text = textOf(message);
    return text && (message.role === "user" || message.role === "assistant")
      ? [{ role: message.role, text }]
      : [];
  });
}
