import { type AnthropicLanguageModelOptions, createAnthropic } from "@ai-sdk/anthropic";
import { createOpenAI } from "@ai-sdk/openai";
import { createProviderRegistry } from "ai";
import { env } from "../../env";

const registry = createProviderRegistry({
  anthropic: createAnthropic({ apiKey: env.ANTHROPIC_API_KEY }),
  openai: createOpenAI({ apiKey: env.OPENAI_API_KEY }),
});

export const model = registry.languageModel("anthropic:claude-opus-5-5");

export const providerOptions = {
  anthropic: {
    effort: "medium",
    fallbacks: "default",
    cacheControl: { type: "ephemeral" },
  } satisfies AnthropicLanguageModelOptions,
};
