import { createHash } from "node:crypto";
import type {
  AgentMessage,
  AgentPart,
  Input,
  Json,
  MediaPart,
  Message,
  ReasoningPart,
  TextPart,
  ToolResultPart,
} from "../provider/index.ts";
import { asObject, type JsonObject, stringOf, withoutKeys } from "./json.ts";

export interface Shape {
  readonly namespaced: boolean;
  readonly maxOutputTokens: boolean;
  readonly summaries: boolean;
  readonly serviceTier?: string;
  readonly conversationId?: string;
}

const validId = /^[\w-]{1,64}$/u;
const invalidIdCharacters = /[^\w-]/gu;
const namespace = { name: "functions", description: "Functions available in this conversation." };

const extensions: Readonly<Record<string, string>> = {
  "application/pdf": "pdf",
  "text/plain": "txt",
  "text/markdown": "md",
  "text/csv": "csv",
  "text/tab-separated-values": "tsv",
  "text/html": "html",
  "application/json": "json",
  "application/xml": "xml",
  "application/rtf": "rtf",
  "application/msword": "doc",
  "application/vnd.openxmlformats-officedocument.wordprocessingml.document": "docx",
  "application/vnd.oasis.opendocument.text": "odt",
  "application/vnd.ms-excel": "xls",
  "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet": "xlsx",
  "application/vnd.ms-powerpoint": "ppt",
  "application/vnd.openxmlformats-officedocument.presentationml.presentation": "pptx",
};

export function request(
  model: string,
  input: Input,
  messages: readonly Message[],
  shape: Shape,
): JsonObject {
  const functions = (input.tools ?? []).map((tool) => ({
    type: "function",
    name: tool.name,
    description: tool.description,
    parameters: tool.inputSchema,
    strict: false,
  }));
  const tools =
    shape.namespaced && functions.length > 0
      ? [{ type: "namespace", ...namespace, tools: functions }]
      : functions;
  const reasoning = reasoningOf(input.effort, shape.summaries);
  return {
    model,
    input: messages.flatMap((message) => itemsOf(message)),
    stream: true,
    store: false,
    include: ["reasoning.encrypted_content"],
    ...(tools.length > 0 ? { tools } : {}),
    ...(Object.keys(reasoning).length > 0 ? { reasoning } : {}),
    ...(input.outputSchema === undefined
      ? {}
      : {
          text: {
            format: {
              type: "json_schema",
              name: "output",
              schema: input.outputSchema,
              strict: true,
            },
          },
        }),
    ...(shape.maxOutputTokens && input.maxOutputTokens !== undefined
      ? { max_output_tokens: input.maxOutputTokens }
      : {}),
    ...(shape.conversationId === undefined
      ? {}
      : { prompt_cache_key: cacheKey(shape.conversationId) }),
    ...(shape.serviceTier === undefined ? {} : { service_tier: shape.serviceTier }),
  };
}

export function wireId(id: string): string {
  if (validId.test(id)) {
    return id;
  }
  const hash = createHash("sha256").update(id).digest("hex").slice(0, 8);
  return `${id.replaceAll(invalidIdCharacters, "_").slice(0, 55)}_${hash}`;
}

function cacheKey(id: string): string {
  return id.length <= 64 ? id : createHash("sha256").update(id).digest("hex");
}

function reasoningOf(effort: string | undefined, summaries: boolean): JsonObject {
  if (effort === "none") {
    return { effort };
  }
  return {
    ...(effort === undefined ? {} : { effort }),
    ...(summaries ? { summary: "auto" } : {}),
  };
}

function itemsOf(message: Message): readonly JsonObject[] {
  if (message.role === "system") {
    return [
      {
        type: "message",
        role: "developer",
        content: message.parts.map((part) => ({ type: "input_text", text: part.text })),
      },
    ];
  }
  if (message.role === "user") {
    return [
      { type: "message", role: "user", content: message.parts.map((part) => contentOf(part)) },
    ];
  }
  if (message.role === "tool") {
    return message.parts.map((part) => resultItem(part));
  }
  return agentItems(message);
}

function agentItems(message: AgentMessage): readonly JsonObject[] {
  return runsOf(message.parts).flatMap((run) => runItems(run));
}

function runsOf(parts: readonly AgentPart[]): readonly (readonly AgentPart[])[] {
  const runs: AgentPart[][] = [];
  for (const part of parts) {
    const last = runs.at(-1);
    const head = last?.[0];
    if (last !== undefined && head !== undefined && isSameItem(head, part)) {
      last.push(part);
    } else {
      runs.push([part]);
    }
  }
  return runs;
}

function isSameItem(head: AgentPart, part: AgentPart): boolean {
  const id = idOf(head);
  return id !== undefined && head.type === part.type && id === idOf(part);
}

function idOf(part: AgentPart): string | undefined {
  return part.type === "reasoning" || part.type === "text"
    ? stringOf(asObject(part.opaque), "id")
    : undefined;
}

function runItems(run: readonly AgentPart[]): readonly JsonObject[] {
  const [head] = run;
  if (head === undefined || idOf(head) === undefined) {
    return run.flatMap((part) => looseItems(part));
  }
  const reasoning = run.filter((part): part is ReasoningPart => part.type === "reasoning");
  if (reasoning.length > 0) {
    return reasoningItem(reasoning);
  }
  return [messageItem(run.filter((part): part is TextPart => part.type === "text"))];
}

function reasoningItem(parts: readonly ReasoningPart[]): readonly JsonObject[] {
  const opaque = parts.map((part) => asObject(part.opaque) ?? {});
  const encrypted = opaque.map((data) => stringOf(data, "encrypted_content")).findLast(Boolean);
  if (encrypted === undefined) {
    return [];
  }
  return [
    {
      ...withoutKeys(opaque[0] ?? {}, ["encrypted_content"]),
      type: "reasoning",
      summary: parts
        .filter((part) => part.text.length > 0)
        .map((part) => ({ type: "summary_text", text: part.text })),
      encrypted_content: encrypted,
    },
  ];
}

function messageItem(parts: readonly TextPart[]): JsonObject {
  return {
    ...withoutKeys(asObject(parts[0]?.opaque) ?? {}, ["annotations", "refusal"]),
    type: "message",
    role: "assistant",
    status: "completed",
    content: parts.map((part) => outputContent(part)),
  };
}

function outputContent(part: TextPart): JsonObject {
  const opaque = asObject(part.opaque);
  if (opaque?.["refusal"] === true) {
    return { type: "refusal", refusal: part.text };
  }
  return { type: "output_text", text: part.text, annotations: opaque?.["annotations"] ?? [] };
}

function looseItems(part: AgentPart): readonly JsonObject[] {
  if (part.type === "text") {
    return [{ type: "message", role: "assistant", content: part.text }];
  }
  if (part.type === "tool_call") {
    return [
      {
        ...asObject(part.opaque),
        type: "function_call",
        call_id: wireId(part.id),
        name: part.name,
        arguments: part.input,
      },
    ];
  }
  if (part.type === "opaque") {
    const data = asObject(part.data);
    return data === undefined ? [] : [data];
  }
  return [];
}

function resultItem(part: ToolResultPart): JsonObject {
  return { type: "function_call_output", call_id: wireId(part.callId), output: outputOf(part) };
}

function outputOf(part: ToolResultPart): Json {
  const [only, ...rest] = part.parts;
  if (only === undefined) {
    return "";
  }
  if (only.type === "text" && rest.length === 0) {
    return only.text;
  }
  return part.parts.map((child) => contentOf(child));
}

function contentOf(part: TextPart | MediaPart): JsonObject {
  if (part.type === "text") {
    return { type: "input_text", text: part.text };
  }
  const { mediaType, source } = part;
  if (mediaType.startsWith("image/")) {
    return {
      type: "input_image",
      image_url: source.kind === "url" ? source.url : `data:${mediaType};base64,${source.data}`,
      detail: "auto",
    };
  }
  if (source.kind === "url") {
    return { type: "input_file", file_url: source.url };
  }
  return {
    type: "input_file",
    filename: part.name ?? `file.${extensions[mediaType] ?? "bin"}`,
    file_data: `data:${mediaType};base64,${source.data}`,
  };
}
