export interface OutgoingEvent {
  readonly type?: string;
  readonly data: string;
  readonly id?: string;
  readonly retry?: number;
}

const lineBreak = /\r\n|\r|\n/u;

export function encode(event: OutgoingEvent): string {
  const lines = [
    ...(event.type === undefined ? [] : [`event: ${singleLine(event.type, "type")}`]),
    ...(event.id === undefined ? [] : [`id: ${identifier(event.id)}`]),
    ...(event.retry === undefined ? [] : [`retry: ${String(delay(event.retry))}`]),
    ...event.data.split(lineBreak).map((line) => `data: ${line}`),
  ];
  return `${lines.join("\n")}\n\n`;
}

function singleLine(value: string, field: string): string {
  if (lineBreak.test(value)) {
    throw new RangeError(`An event ${field} must not contain a line break.`);
  }
  return value;
}

function identifier(id: string): string {
  if (id.includes("\0")) {
    throw new RangeError("An event id must not contain a NULL character.");
  }
  return singleLine(id, "id");
}

function delay(retry: number): number {
  if (!Number.isSafeInteger(retry) || retry < 0) {
    throw new RangeError("An event retry must be a non-negative integer of milliseconds.");
  }
  return retry;
}
