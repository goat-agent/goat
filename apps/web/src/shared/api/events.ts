import { z } from "zod";
import { apiBase } from "./client";

/**
 * Opens a server event stream and hands each event, checked against `schema`, to `onEvent`. The
 * browser reconnects by itself after a dropped connection and sends Last-Event-ID when it does;
 * it gives up only when the server refuses the stream, such as after the session ended, and then
 * `onRefused` runs. Returns a function that closes the stream.
 */
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
