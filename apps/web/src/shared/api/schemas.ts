import { z } from "zod";
import type {
  Agent as AgentType,
  Message as MessageType,
  SentFile as SentFileType,
} from "./client";

// Event streams and multipart uploads arrive as untyped JSON. These schemas check them, and
// `satisfies` keeps each one in step with the type generated from the OpenAPI document.

export const SentFile = z.object({
  id: z.string(),
  name: z.string(),
  mediaType: z.string(),
}) satisfies z.ZodType<SentFileType>;

export const Message = z.object({
  id: z.number(),
  role: z.string(),
  text: z.string(),
  files: z.array(SentFile).optional(),
  connect: z.array(z.object({ integrationId: z.string(), name: z.string() })).optional(),
  steps: z.array(z.string()).optional(),
  createdAt: z.string(),
}) satisfies z.ZodType<MessageType>;

export const Agent = z.object({
  id: z.string(),
  name: z.string(),
  instructions: z.string(),
  avatarId: z.string().optional(),
  status: z.string(),
  lastMessage: Message.optional(),
  createdAt: z.string(),
}) satisfies z.ZodType<AgentType>;

export const NewMessage = z.object({ message: Message });

export const AgentsEvent = z.object({ agents: z.array(Agent) });
