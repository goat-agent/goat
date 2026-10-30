import { z } from "zod";
import { env } from "../../env";
import type { Line } from "../conversation";
import type { Locale } from "../i18n";
import { logger } from "../logger";

const SESSIONS_URL = "https://api.openai.com/v1/live/sessions";
const REQUEST_TIMEOUT_MS = 15_000;

const voices: Record<Locale, string> = { en: "marin", ko: "noeul" };

const log = logger.child({ module: "voice" });

export type Setup = { instructions: string; history: Line[]; locale: Locale };

export type Handlers = {
  /** Part of what the user is saying, in order. */
  onUserSpeech: (text: string) => void;
  /** The voice model wants the agent to handle what the user asked. */
  onTask: (taskId: string) => void;
  onClosed: () => void;
};

export type Session = {
  /** Tells the voice model what the agent has to say about a task; it speaks it in its own words. */
  report: (taskId: string, text: string) => void;
  close: () => void;
};

const Created = z.object({
  session: z.object({ id: z.string() }),
  transport: z.object({ sdp: z.string() }),
});

const ServerEvent = z.object({
  type: z.string(),
  delta: z.string().optional(),
  delegation: z.object({ id: z.string() }).optional(),
  reason: z.string().optional(),
  error: z.object({ type: z.string().optional(), code: z.string().optional() }).optional(),
});

const authorization = `Bearer ${env.OPENAI_API_KEY}`;

/** Starts a session from the device's WebRTC offer and returns the answer for the device. */
export async function start(offer: string, setup: Setup): Promise<{ id: string; answer: string }> {
  const response = await fetch(SESSIONS_URL, {
    method: "POST",
    headers: { authorization, "content-type": "application/json" },
    body: JSON.stringify({
      session: {
        model: "gpt-live-1",
        instructions: setup.instructions,
        audio: { output: { voice: voices[setup.locale] } },
        // The agent on our server does the work, not a model the voice service picks.
        delegation: { type: "client" },
        input: setup.history.map(({ role, text }) => ({
          type: "message",
          role,
          content: [{ type: role === "user" ? "input_text" : "output_text", text }],
        })),
        store: false,
      },
      transport: { type: "webrtc", sdp: offer },
    }),
    signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
  });
  if (!response.ok) throw new Error(`voice session refused with ${response.status}`);
  const created = Created.parse(await response.json());
  return { id: created.session.id, answer: created.transport.sdp };
}

/** Joins a running session from the server, beside the device, to hear and steer it. */
export function attach(id: string, handlers: Handlers): Session {
  // Without graceful_close the service leaves this socket open after the session ends.
  const url = `${SESSIONS_URL}/${encodeURIComponent(id)}/attach?graceful_close=true`;
  const socket = new WebSocket(url, { headers: { authorization } });
  const pending: string[] = [];

  const send = (event: object) => {
    const data = JSON.stringify(event);
    if (socket.readyState === WebSocket.OPEN) socket.send(data);
    else if (socket.readyState === WebSocket.CONNECTING) pending.push(data);
  };

  socket.addEventListener("open", () => {
    for (const data of pending.splice(0)) socket.send(data);
  });
  socket.addEventListener("message", ({ data }) => {
    if (typeof data !== "string") return;
    let json: unknown;
    try {
      json = JSON.parse(data);
    } catch {
      // The session sends JSON events only; anything else carries nothing to act on.
      return;
    }
    const event = ServerEvent.safeParse(json);
    if (!event.success) return;
    const { type, delta, delegation, reason, error } = event.data;
    if (type === "session.input_transcript.delta" && delta) handlers.onUserSpeech(delta);
    else if (type === "session.delegation.created" && delegation) handlers.onTask(delegation.id);
    else if (type === "session.closed") log.info({ sessionId: id, reason }, "voice session closed");
    else if (type === "error") log.warn({ sessionId: id, ...error }, "voice session error");
  });
  socket.addEventListener("close", () => handlers.onClosed());

  return {
    report: (taskId, text) => {
      send({ type: "session.commentary.append", delegation_id: taskId, content: text });
    },
    close: () => {
      send({ type: "session.close" });
    },
  };
}
