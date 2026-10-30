import type { ModelMessage } from "ai";
import { and, desc, eq } from "drizzle-orm";
import { db } from "../db";
import { messages } from "../db/schema/agents";

export type Line = { role: "user" | "assistant"; text: string };

/** The text a person sees in a message: the user's own words, or the agent's reply. */
export function textOf(message: ModelMessage): string {
  if (message.role === "user") {
    const { content } = message;
    // The server puts its own parts first, so the user's text, if any, is the last part.
    const last = typeof content === "string" ? content : content.at(-1);
    return typeof last === "string" ? last : last?.type === "text" ? last.text : "";
  }
  if (message.role !== "assistant") return "";
  return typeof message.content === "string"
    ? message.content
    : message.content.flatMap((part) => (part.type === "text" ? [part.text] : [])).join("");
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
