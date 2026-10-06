export type Fetch = (input: string | URL | Request, init?: RequestInit) => Promise<Response>;

export interface Fixture {
  readonly status: number;
  readonly headers: Readonly<Record<string, string>>;
  readonly chunks: readonly string[];
}

export type Chunking = "recorded" | "bytes" | "whole";

export interface RecordedRequest {
  readonly url: string;
  readonly method: string;
  readonly body: string;
  readonly signal: AbortSignal | undefined;
}

export interface RecordedFetch {
  readonly fetch: Fetch;
  readonly requests: readonly RecordedRequest[];
}

export interface Recording {
  readonly fetch: Fetch;
  readonly fixtures: () => Promise<readonly Fixture[]>;
}

export function recordedFetch(
  fixtures: readonly Fixture[],
  chunking: Chunking = "recorded",
): RecordedFetch {
  const requests: RecordedRequest[] = [];
  const fetch: Fetch = async (input, init) => {
    const signal = init?.signal ?? undefined;
    const fixture = fixtures[requests.length];
    requests.push({
      url: urlOf(input),
      method: init?.method ?? "GET",
      body: await bodyOf(init),
      signal,
    });
    if (fixture === undefined) {
      throw new Error(`No recorded response for request ${String(requests.length)}.`);
    }
    signal?.throwIfAborted();
    return new Response(replayBody(fixture, chunking, signal), {
      status: fixture.status,
      headers: fixture.headers,
    });
  };
  return { fetch, requests };
}

export function recordFetch(
  base: Fetch,
  keepHeader: (name: string) => boolean = isStandardHeader,
): Recording {
  const pending: Promise<Fixture>[] = [];
  const fetch: Fetch = async (input, init) => {
    const response = await base(input, init);
    const [forCaller, forRecording] = response.body?.tee() ?? [null, null];
    const headers = Object.fromEntries(
      [...response.headers.keys()]
        .filter((name) => keepHeader(name))
        .map((name) => [name, response.headers.get(name) ?? ""]),
    );
    pending.push(
      readChunks(forRecording).then((chunks) => ({ status: response.status, headers, chunks })),
    );
    return new Response(forCaller, { status: response.status, headers: response.headers });
  };
  return { fetch, fixtures: async () => Promise.all(pending) };
}

function urlOf(input: string | URL | Request): string {
  if (typeof input === "string") {
    return input;
  }
  return input instanceof URL ? input.href : input.url;
}

async function bodyOf(init: RequestInit | undefined): Promise<string> {
  if (init?.body === undefined || init.body === null) {
    return "";
  }
  return typeof init.body === "string" ? init.body : new Response(init.body).text();
}

function isStandardHeader(name: string): boolean {
  return ["content-type", "retry-after"].includes(name.toLowerCase());
}

async function readChunks(stream: ReadableStream<Uint8Array> | null): Promise<readonly string[]> {
  if (stream === null) {
    return [];
  }
  const decoder = new TextDecoder();
  const chunks: string[] = [];
  for await (const bytes of stream) {
    chunks.push(decoder.decode(bytes, { stream: true }));
  }
  const rest = decoder.decode();
  return rest.length > 0 ? [...chunks, rest] : chunks;
}

function replayBody(
  fixture: Fixture,
  chunking: Chunking,
  signal: AbortSignal | undefined,
): ReadableStream<Uint8Array> {
  const pieces = split(fixture.chunks, chunking);
  let next = 0;
  return new ReadableStream<Uint8Array>({
    start(controller) {
      signal?.addEventListener(
        "abort",
        () => {
          controller.error(signal.reason);
        },
        { once: true },
      );
    },
    pull(controller) {
      const piece = pieces[next];
      next += 1;
      if (piece === undefined) {
        controller.close();
      } else {
        controller.enqueue(piece);
      }
    },
  });
}

function split(chunks: readonly string[], chunking: Chunking): readonly Uint8Array[] {
  const encoder = new TextEncoder();
  if (chunking === "recorded") {
    return chunks.map((chunk) => encoder.encode(chunk));
  }
  const whole = encoder.encode(chunks.join(""));
  return chunking === "whole" ? [whole] : Array.from(whole, (byte) => Uint8Array.of(byte));
}
