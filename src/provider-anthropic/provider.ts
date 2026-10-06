import {
  err,
  type ModelInfo,
  ok,
  type Provider,
  type ProviderError,
  type Result,
} from "../provider/index.ts";
import { catalogProfile, catalogProfiles, liveProfile, type Profile } from "./catalog.ts";
import { Client, type Credentials, type Fetch } from "./client.ts";
import { malformed } from "./error.ts";
import { streamTurn } from "./turn.ts";
import { liveModel, modelList } from "./wire.ts";

export interface Options {
  readonly baseUrl?: string;
  readonly fetch?: Fetch;
}

export interface ApiKeyOptions extends Options {
  readonly key: string;
}

export interface OAuthOptions extends Options {
  readonly token: string | (() => Promise<string>);
}

const providerId = "anthropic";

export function apiKey(options: ApiKeyOptions): Provider {
  return create(options, () =>
    Promise.resolve(ok({ authorization: `Bearer ${options.key}`, betas: [] })),
  );
}

export function oauth(options: OAuthOptions): Provider {
  const { token } = options;
  return create(options, async () => {
    try {
      const value = typeof token === "string" ? token : await token();
      return ok({ authorization: `Bearer ${value}`, betas: ["oauth-2025-04-20"] });
    } catch (error) {
      return err({ kind: "auth", message: error instanceof Error ? error.message : String(error) });
    }
  });
}

function create(options: Options, credentials: Credentials): Provider {
  const client = new Client(
    options.baseUrl ?? "https://api.anthropic.com",
    options.fetch ?? fetch,
    credentials,
  );
  const fetched = new Map<string, Profile>();
  const profileOf = async (
    id: string,
    signal: AbortSignal | undefined,
  ): Promise<Result<Profile, ProviderError>> => {
    const known = catalogProfile(id) ?? fetched.get(id);
    if (known !== undefined) {
      return ok(known);
    }
    const response = await client.get(`models/${encodeURIComponent(id)}`, signal);
    if (!response.ok) {
      return response;
    }
    const parsed = liveModel.safeParse(response.value);
    if (!parsed.success) {
      return err(malformed(`Anthropic described ${id} in an unexpected shape.`));
    }
    const profile = liveProfile(parsed.data);
    fetched.set(id, profile);
    return ok(profile);
  };
  return {
    id: providerId,
    model: (id) => ({
      id,
      provider: providerId,
      info: async (call) => {
        const profile = await profileOf(id, call?.signal);
        return profile.ok ? ok(profile.value.info) : profile;
      },
      stream: (input, call) =>
        streamTurn(
          {
            client,
            producer: { provider: providerId, model: id },
            profile: async (signal) => profileOf(id, signal),
          },
          input,
          call ?? {},
        ),
    }),
    models: async (call) => models(client, call?.signal),
  };
}

async function models(
  client: Client,
  signal: AbortSignal | undefined,
): Promise<Result<readonly ModelInfo[], ProviderError>> {
  const response = await client.get("models?limit=1000", signal);
  const list = response.ok ? modelList.safeParse(response.value) : undefined;
  if (list?.success !== true) {
    return ok(catalogProfiles().map((profile) => profile.info));
  }
  return ok(
    list.data.data.flatMap((item) => {
      const model = liveModel.safeParse(item);
      return model.success ? [(catalogProfile(model.data.id) ?? liveProfile(model.data)).info] : [];
    }),
  );
}
