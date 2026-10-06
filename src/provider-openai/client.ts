import { err, ok, type ProviderError, type Result } from "../provider/index.ts";
import { malformed, networkError, responseError } from "./error.ts";

export type Fetch = (input: string | URL | Request, init?: RequestInit) => Promise<Response>;

export type Credentials = () => Promise<Result<string, ProviderError>>;

const headerValue = /^[!-~]+$/u;

export class Client {
  readonly #baseUrl: string;
  readonly #fetch: Fetch;
  readonly #credentials: Credentials;

  constructor(baseUrl: string, fetch: Fetch, credentials: Credentials) {
    this.#baseUrl = baseUrl.replace(/\/+$/u, "");
    this.#fetch = fetch;
    this.#credentials = credentials;
  }

  async post(
    path: string,
    body: string,
    signal: AbortSignal,
  ): Promise<Result<Response, ProviderError>> {
    return this.#send(path, signal, {
      method: "POST",
      body,
      headers: { "content-type": "application/json", accept: "text/event-stream" },
    });
  }

  async get(path: string, signal?: AbortSignal): Promise<Result<unknown, ProviderError>> {
    const response = await this.#send(path, signal, { method: "GET", headers: {} });
    if (!response.ok) {
      return response;
    }
    try {
      const text = await response.value.text();
      if (!response.value.ok) {
        return err(responseError(response.value.status, response.value.headers, text));
      }
      return ok(JSON.parse(text));
    } catch (error) {
      return err(error instanceof SyntaxError ? malformed(error.message) : networkError(error));
    }
  }

  async #send(
    path: string,
    signal: AbortSignal | undefined,
    init: {
      readonly method: "GET" | "POST";
      readonly body?: string;
      readonly headers: Readonly<Record<string, string>>;
    },
  ): Promise<Result<Response, ProviderError>> {
    const credential = await untilAborted(this.#credentials(), signal);
    if (!credential.ok) {
      return credential;
    }
    if (!headerValue.test(credential.value)) {
      return err({
        kind: "auth",
        message: "The credential contains characters that cannot be sent in an HTTP header.",
      });
    }
    try {
      return ok(
        await this.#fetch(`${this.#baseUrl}/${path}`, {
          method: init.method,
          headers: { authorization: `Bearer ${credential.value}`, ...init.headers },
          ...(init.body === undefined ? {} : { body: init.body }),
          ...(signal === undefined ? {} : { signal }),
        }),
      );
    } catch (error) {
      return err(networkError(error));
    }
  }
}

async function untilAborted<T>(
  promise: Promise<Result<T, ProviderError>>,
  signal: AbortSignal | undefined,
): Promise<Result<T, ProviderError>> {
  if (signal === undefined) {
    return promise;
  }
  const aborted = new Promise<Result<T, ProviderError>>((resolve) => {
    const settle = (): void => {
      resolve(err(networkError(signal.reason)));
    };
    if (signal.aborted) {
      settle();
    } else {
      signal.addEventListener("abort", settle, { once: true });
    }
  });
  return Promise.race([promise, aborted]);
}
