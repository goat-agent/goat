import type { Stop } from "@goat/provider";
import { eventFailure, type Failure } from "./error.ts";
import type { WireResponse } from "./wire.ts";

export type Outcome = { readonly stop: Stop } | { readonly failure: Failure };

export function outcomeOf(type: string, response: WireResponse, refused: boolean): Outcome {
  if (type === "response.failed") {
    return { failure: eventFailure(response.error) };
  }
  if (type === "response.completed") {
    return { stop: { reason: refused ? "refusal" : "done" } };
  }
  const reason = response.incomplete_details?.reason ?? "unknown";
  if (reason === "max_output_tokens") {
    return { stop: { reason: "length" } };
  }
  if (reason === "content_filter") {
    return { stop: { reason: "refusal" } };
  }
  return {
    stop: {
      reason: "error",
      error: { kind: "unknown", message: `OpenAI stopped the response early: ${reason}.` },
    },
  };
}
