import { err, ok, type ProviderError, type Result } from "@goat/provider";
import { malformed, networkError, responseError } from "./error.ts";

export type Fetch = (input: string | URL | Request, init?: RequestInit) => Promise<Response>;

interface Authorization {
  readonly authorization: string;
  readonly betas: readonly string[];
}

export type Credentials = () => Promise<Result<Authorization, ProviderError>>;

export interface Call {
  readonly method: "GET" | "POST";
  readonly body?: string;
  readonly betas?: readonly string[];
  readonly signal?: AbortSignal | undefined;
}

const headerValue = /^Bearer [!-~]+$/u;

export class Client {
  readonly #baseUrl: string;
  readonly #fetch: Fetch;
  readonly #credentials: Credentials;

  constructor(baseUrl: string, fetch: Fetch, credentials: Credentials) {
    this.#baseUrl = baseUrl.replace(/\/+$/u, "");
    this.#fetch = fetch;
    this.#credentials = credentials;
  }

  async send(path: string, call: Call): Promise<Result<Response, ProviderError>> {
    const credentials = await untilAborted(this.#credentials(), call.signal);
    if (!credentials.ok) {
      return credentials;
    }
    if (!headerValue.test(credentials.value.authorization)) {
      return err({
        kind: "auth",
        message: "The credential contains characters that cannot be sent in an HTTP header.",
      });
    }
    const betas = [...credentials.value.betas, ...(call.betas ?? [])];
    try {
      return ok(
        await this.#fetch(`${this.#baseUrl}/v1/${path}`, {
          method: call.method,
          headers: {
            "anthropic-version": "2023-06-01",
            authorization: credentials.value.authorization,
            "content-type": "application/json",
            ...(betas.length > 0 ? { "anthropic-beta": betas.join(",") } : {}),
          },
          ...(call.body === undefined ? {} : { body: call.body }),
          ...(call.signal === undefined ? {} : { signal: call.signal }),
        }),
      );
    } catch (error) {
      return err(networkError(error));
    }
  }

  async get(
    path: string,
    signal: AbortSignal | undefined,
  ): Promise<Result<unknown, ProviderError>> {
    const response = await this.send(path, { method: "GET", signal });
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
