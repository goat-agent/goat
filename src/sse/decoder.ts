export interface ServerSentEvent {
  readonly type: string;
  readonly data: string;
  readonly id: string;
}

const lineBreak = /\r\n|\r|\n/gu;
const digits = /^\d+$/u;

export class Decoder {
  #started = false;
  #skipLineFeed = false;
  #pending = "";
  #type = "";
  #data: string[] = [];
  #id = "";
  #retry: number | undefined;

  get retry(): number | undefined {
    return this.#retry;
  }

  push(text: string): readonly ServerSentEvent[] {
    const chunk = this.#prepare(text);
    const events: ServerSentEvent[] = [];
    let start = 0;
    for (const match of chunk.matchAll(lineBreak)) {
      const line = this.#pending + chunk.slice(start, match.index);
      this.#pending = "";
      start = match.index + match[0].length;
      this.#skipLineFeed = match[0] === "\r" && start === chunk.length;
      const event = this.#line(line);
      if (event !== undefined) {
        events.push(event);
      }
    }
    this.#pending += chunk.slice(start);
    return events;
  }

  #prepare(text: string): string {
    let chunk = text;
    if (this.#skipLineFeed && chunk.length > 0) {
      this.#skipLineFeed = false;
      chunk = chunk.startsWith("\n") ? chunk.slice(1) : chunk;
    }
    if (!this.#started && chunk.length > 0) {
      this.#started = true;
      chunk = chunk.startsWith("\uFEFF") ? chunk.slice(1) : chunk;
    }
    return chunk;
  }

  #line(line: string): ServerSentEvent | undefined {
    if (line.length === 0) {
      return this.#dispatch();
    }
    if (line.startsWith(":")) {
      return undefined;
    }
    const colon = line.indexOf(":");
    const field = colon === -1 ? line : line.slice(0, colon);
    const raw = colon === -1 ? "" : line.slice(colon + 1);
    this.#field(field, raw.startsWith(" ") ? raw.slice(1) : raw);
    return undefined;
  }

  #field(field: string, value: string): void {
    if (field === "event") {
      this.#type = value;
    } else if (field === "data") {
      this.#data.push(value);
    } else if (field === "id" && !value.includes("\0")) {
      this.#id = value;
    } else if (field === "retry" && digits.test(value)) {
      this.#retry = Number(value);
    }
  }

  #dispatch(): ServerSentEvent | undefined {
    const data = this.#data;
    const type = this.#type;
    this.#data = [];
    this.#type = "";
    if (data.length === 0) {
      return undefined;
    }
    return { type: type === "" ? "message" : type, data: data.join("\n"), id: this.#id };
  }
}

export async function* decode(stream: ReadableStream<Uint8Array>): AsyncIterable<ServerSentEvent> {
  const text = new TextDecoder();
  const decoder = new Decoder();
  for await (const bytes of stream) {
    yield* decoder.push(text.decode(bytes, { stream: true }));
  }
  yield* decoder.push(text.decode());
}
