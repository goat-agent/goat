import type { MouseEvent } from "react";
import type { z } from "zod";
import { events, Host, requests } from "./contract";

type Requests = typeof requests;
type Events = typeof events;

declare global {
  interface Window {
    goatHost?: unknown;
    webkit?: {
      messageHandlers?: Record<string, { postMessage: (message: unknown) => Promise<unknown> }>;
    };
  }
}

// The Mac app answers on this WebKit handler and puts the host on the page before any script.
const handler = window.webkit?.messageHandlers?.goat;

/** The native app showing this page, or undefined in a browser. */
export const host: z.infer<typeof Host> | undefined = handler
  ? Host.parse(window.goatHost)
  : undefined;

/** Asks the native app; its answer is checked against the contract. */
export function request<M extends keyof Requests>(
  method: M,
  params: z.input<Requests[M]["params"]>,
): Promise<z.infer<Requests[M]["result"]>>;
export async function request(method: keyof Requests, params: unknown): Promise<unknown> {
  if (!handler) throw new Error(`no native app to answer ${method}`);
  // A WebKit message handler takes only the message; this is not window.postMessage.
  // oxlint-disable-next-line unicorn/require-post-message-target-origin
  const answer = await handler.postMessage({ method, params });
  return requests[method].result.parse(answer);
}

/**
 * Runs `listener` each time the native app sends `event`, which arrives as a `goat:<event>` DOM
 * event. Returns a function that stops listening.
 */
export function onNativeEvent<E extends keyof Events>(
  event: E,
  listener: (payload: z.infer<Events[E]>) => void,
): () => void {
  function receive(dispatched: Event) {
    if (!(dispatched instanceof CustomEvent)) return;
    const payload = events[event].safeParse(dispatched.detail);
    if (payload.success) listener(payload.data);
  }
  window.addEventListener(`goat:${event}`, receive);
  return () => window.removeEventListener(`goat:${event}`, receive);
}

/**
 * The page has no title bar above it in the native window, so its bars stand in for one: their
 * empty parts move the window, and a double click does what the system says a title bar does.
 */
export function moveWindow(event: MouseEvent<HTMLElement>) {
  if (!host || event.button !== 0 || !(event.target instanceof Element)) return;
  if (event.target.closest("a, button, input, textarea, select, [role=button]")) return;
  event.preventDefault();
  request(event.detail === 2 ? "doubleClickTitleBar" : "moveWindow", {}).catch(() => {
    // A window that did not move needs nothing more.
  });
}
