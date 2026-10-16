import { createHash } from "node:crypto";
import type {
  AgentPart,
  Input,
  Json,
  MediaPart,
  Message,
  SystemMessage,
  TextPart,
  ToolResultPart,
} from "@goat/provider";
import { budgets, type Profile } from "./catalog.ts";
import { asObject, type JsonObject } from "./json.ts";
import { block } from "./wire.ts";

export interface Request {
  readonly body: JsonObject;
  readonly betas: readonly string[];
}

type WireMessage = {
  readonly role: "user" | "assistant" | "system";
  readonly content: readonly JsonObject[];
};

interface Reasoning {
  readonly thinking?: JsonObject;
  readonly effort?: string;
  readonly betas: readonly string[];
}

const validId = /^[\w-]+$/u;
const invalidIdCharacters = /[^\w-]/gu;
const longCache = { type: "ephemeral", ttl: "1h" } as const;
const bindingBeta = "thinking-binding-controls-2026-08-01";
const minimumBudget = 1024;

export function request(
  model: string,
  input: Input,
  messages: readonly Message[],
  profile: Profile,
): Request {
  const leading = messages.findIndex((message) => message.role !== "system");
  const system = messages
    .slice(0, leading === -1 ? messages.length : leading)
    .filter((message): message is SystemMessage => message.role === "system");
  const conversation = new Conversation(profile.systemMessages);
  for (const message of messages.slice(system.length)) {
    conversation.add(message);
  }
  const maxTokens = input.maxOutputTokens ?? profile.info.maxOutputTokens;
  const reasoning = reasoningOf(profile, input.effort, maxTokens);
  const outputConfig = {
    ...(reasoning.effort === undefined ? {} : { effort: reasoning.effort }),
    ...(input.outputSchema === undefined
      ? {}
      : { format: { type: "json_schema", schema: input.outputSchema } }),
  };
  const systemBlocks = withLongCache(
    system.flatMap((message) => message.parts.map((part) => textBlock(part))),
  );
  const tools = withLongCache(
    (input.tools ?? []).map((tool) => ({
      name: tool.name,
      description: tool.description,
      input_schema: tool.inputSchema,
      eager_input_streaming: true,
    })),
  );
  return {
    body: {
      model,
      max_tokens: maxTokens,
      stream: true,
      ...(systemBlocks.length > 0 ? { system: systemBlocks } : {}),
      messages: conversation.finish(),
      ...(tools.length > 0 ? { tools } : {}),
      ...(reasoning.thinking === undefined ? {} : { thinking: reasoning.thinking }),
      ...(Object.keys(outputConfig).length > 0 ? { output_config: outputConfig } : {}),
      cache_control: { type: "ephemeral" },
    },
    betas: reasoning.betas,
  };
}

export function wireId(id: string): string {
  if (validId.test(id)) {
    return id;
  }
  const hash = createHash("sha256").update(id).digest("hex").slice(0, 8);
  return `${id.replaceAll(invalidIdCharacters, "_")}_${hash}`;
}

class Conversation {
  readonly #systemMessages: boolean;
  readonly #messages: WireMessage[] = [];
  #pending: readonly JsonObject[] = [];

  constructor(systemMessages: boolean) {
    this.#systemMessages = systemMessages;
  }

  add(message: Message): void {
    if (message.role === "system") {
      this.#pending = [...this.#pending, ...message.parts.map((part) => textBlock(part))];
    } else if (message.role === "agent") {
      this.#flush();
      this.#append(
        "assistant",
        message.parts.flatMap((part) => agentBlocks(part)),
      );
    } else if (message.role === "user") {
      this.#append(
        "user",
        message.parts.map((part) => contentBlock(part)),
      );
    } else {
      this.#append(
        "user",
        message.parts.map((part) => resultBlock(part)),
      );
    }
  }

  finish(): readonly WireMessage[] {
    this.#flush();
    return this.#messages;
  }

  #flush(): void {
    const pending = this.#pending;
    this.#pending = [];
    if (pending.length === 0) {
      return;
    }
    if (this.#systemMessages && this.#messages.at(-1)?.role === "user") {
      this.#messages.push({ role: "system", content: pending });
    } else {
      this.#append("user", pending);
    }
  }

  #append(role: "user" | "assistant", content: readonly JsonObject[]): void {
    if (content.length === 0) {
      return;
    }
    const last = this.#messages.at(-1);
    if (last?.role === role) {
      this.#messages[this.#messages.length - 1] = { role, content: [...last.content, ...content] };
    } else {
      this.#messages.push({ role, content });
    }
  }
}

function reasoningOf(profile: Profile, effort: string | undefined, maxTokens: number): Reasoning {
  const level = effort ?? profile.info.effort?.default;
  const { thinking } = profile;
  if (thinking.kind === "adaptive") {
    if (level === "none") {
      return { thinking: { type: "disabled" }, betas: [] };
    }
    const binding = profile.bindsThinking
      ? { block_binding: { prefix_mismatch_behavior: "drop_block" } }
      : {};
    return {
      thinking: { type: "adaptive", display: "summarized", ...binding },
      ...(level === undefined ? {} : { effort: level }),
      betas: profile.bindsThinking ? [bindingBeta] : [],
    };
  }
  if (thinking.kind === "none") {
    return { betas: [] };
  }
  const budget = Math.min(budgets[level ?? "none"] ?? 0, maxTokens - minimumBudget);
  return budget < minimumBudget
    ? { thinking: { type: "disabled" }, betas: [] }
    : { thinking: { type: "enabled", budget_tokens: budget }, betas: [] };
}

function agentBlocks(part: AgentPart): readonly JsonObject[] {
  if (part.type === "text") {
    return [{ ...asObject(part.opaque), type: "text", text: part.text }];
  }
  if (part.type === "reasoning") {
    const signature = signatureOf(part.opaque);
    return signature === undefined
      ? []
      : [{ ...asObject(part.opaque), type: "thinking", thinking: part.text, signature }];
  }
  if (part.type === "tool_call") {
    return [
      {
        ...asObject(part.opaque),
        type: "tool_use",
        id: wireId(part.id),
        name: part.name,
        input: objectOf(part.input),
      },
    ];
  }
  const data = asObject(part.data);
  return data === undefined ? [] : [data];
}

function resultBlock(part: ToolResultPart): JsonObject {
  return {
    type: "tool_result",
    tool_use_id: wireId(part.callId),
    ...(part.parts.length > 0 ? { content: part.parts.map((child) => contentBlock(child)) } : {}),
    ...(part.isError ? { is_error: true } : {}),
  };
}

function contentBlock(part: TextPart | MediaPart): JsonObject {
  return part.type === "text" ? textBlock(part) : mediaBlock(part);
}

function textBlock(part: TextPart): JsonObject {
  return { type: "text", text: part.text };
}

function mediaBlock(part: MediaPart): JsonObject {
  const { mediaType, source } = part;
  if (mediaType.startsWith("image/")) {
    return {
      type: "image",
      source:
        source.kind === "url"
          ? { type: "url", url: source.url }
          : { type: "base64", media_type: mediaType, data: source.data },
    };
  }
  const title = part.name === undefined ? {} : { title: part.name };
  if (source.kind === "url") {
    return { type: "document", source: { type: "url", url: source.url }, ...title };
  }
  const data =
    mediaType === "text/plain"
      ? { type: "text", media_type: mediaType, data: Buffer.from(source.data, "base64").toString() }
      : { type: "base64", media_type: mediaType, data: source.data };
  return { type: "document", source: data, ...title };
}

function withLongCache(blocks: readonly JsonObject[]): readonly JsonObject[] {
  return blocks.map((item, index) =>
    index === blocks.length - 1 ? { ...item, cache_control: longCache } : item,
  );
}

function signatureOf(opaque: Json | undefined): string | undefined {
  const signature = asObject(opaque)?.["signature"];
  return typeof signature === "string" && signature.length > 0 ? signature : undefined;
}

function objectOf(input: string): JsonObject {
  try {
    const parsed = block.safeParse(JSON.parse(input));
    return parsed.success ? parsed.data : {};
  } catch {
    return {};
  }
}
