import { AsyncLocalStorage } from "node:async_hooks";
import { type AnthropicLanguageModelOptions, createAnthropic } from "@ai-sdk/anthropic";
import { createOpenAI } from "@ai-sdk/openai";
import { createProviderRegistry, transcribe as transcribeAudio } from "ai";
import { env } from "../../env";

// gpt-transcribe takes keyword hints, but the AI SDK drops them (vercel/ai#23487). Until
// vercel/ai#23494 ships, add them to the outgoing form here.
const transcriptionKeywords = new AsyncLocalStorage<string[]>();

function addKeywords(input: string | URL | Request, init?: RequestInit): Promise<Response> {
  const keywords = transcriptionKeywords.getStore();
  if (keywords && init?.body instanceof FormData) {
    for (const keyword of keywords) init.body.append("keywords[]", keyword);
  }
  return globalThis.fetch(input, init);
}

const registry = createProviderRegistry({
  anthropic: createAnthropic({ apiKey: env.ANTHROPIC_API_KEY }),
  openai: createOpenAI({
    apiKey: env.OPENAI_API_KEY,
    // The provider wants the global fetch's type, which in Bun also carries preconnect.
    fetch: Object.assign(addKeywords, { preconnect: globalThis.fetch.preconnect }),
  }),
});

export const model = registry.languageModel(env.AGENT_MODEL);

export const summarizer = registry.languageModel(env.SUMMARY_MODEL);

export const providerOptions = {
  anthropic: {
    effort: "medium",
    thinking: { type: "adaptive", blockBinding: { prefixMismatchBehavior: "drop_block" } },
    fallbacks: "default",
    cacheControl: { type: "ephemeral" },
  } satisfies AnthropicLanguageModelOptions,
};

const transcriptionModel = registry.transcriptionModel("openai:gpt-transcribe");

export type TranscriptionHints = {
  /** What the speaker is likely talking about, such as the recent conversation. */
  context: string;
  /** Names and terms the audio may contain. */
  keywords: string[];
};

export async function transcribe(audio: Uint8Array, hints: TranscriptionHints): Promise<string> {
  const { text } = await transcriptionKeywords.run(hints.keywords, () =>
    transcribeAudio({
      model: transcriptionModel,
      audio,
      providerOptions: { openai: hints.context ? { prompt: hints.context } : {} },
    }),
  );
  return text;
}
