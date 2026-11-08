import { z } from "zod";
import { apiBase } from "./client";

export function listen<T>(
  path: string,
  types: readonly string[],
  schema: z.ZodType<T>,
  handlers: { onEvent: (event: T) => void; onRefused: () => void },
): () => void {
  const source = new EventSource(`${apiBase}${path}`, { withCredentials: true });
  const frame = z
    .string()
    .transform((text, context) => {
      try {
        return JSON.parse(text) as unknown;
      } catch {
        context.addIssue({ code: "custom", message: "not JSON" });
        return z.NEVER;
      }
    })
    .pipe(schema);
  const handle = (event: MessageEvent) => {
    const parsed = frame.safeParse(event.data);
    if (parsed.success) handlers.onEvent(parsed.data);
  };
  for (const type of types) source.addEventListener(type, handle);
  source.addEventListener("error", () => {
    if (source.readyState === EventSource.CLOSED) handlers.onRefused();
  });
  return () => {
    source.close();
  };
}
