import {
  err,
  type ModelInfo,
  ok,
  type Provider,
  type ProviderError,
  type Result,
} from "../provider/index.ts";
import { catalogProfile, catalogProfiles } from "./catalog.ts";
import { Client, type Credentials, type Fetch } from "./client.ts";
import { type Route, Settings, streamTurn } from "./turn.ts";
import { modelList, planModelList } from "./wire.ts";

export interface Options {
  readonly baseUrl?: string;
  readonly fetch?: Fetch;
}

export interface ApiKeyOptions extends Options {
  readonly key: string;
  readonly serviceTier?: "flex" | "fast";
}

export interface ChatGPTOptions extends Options {
  readonly token: string | (() => Promise<string>);
}

interface Account {
  readonly priced: boolean;
  readonly route: Route;
  readonly list: (client: Client) => Promise<Result<readonly string[], ProviderError>>;
}

const providerId = "openai";

export function apiKey(options: ApiKeyOptions): Provider {
  const { serviceTier } = options;
  return create(options, () => Promise.resolve(ok(options.key)), {
    priced: true,
    route: {
      namespaced: false,
      maxOutputTokens: true,
      ...(serviceTier === undefined ? {} : { serviceTier }),
    },
    list: async (client) =>
      listed(client, "models", (value) => {
        const parsed = modelList.safeParse(value);
        return parsed.success ? parsed.data.data.map((model) => model.id) : undefined;
      }),
  });
}

export function chatgpt(options: ChatGPTOptions): Provider {
  const { token } = options;
  return create(
    options,
    async () => {
      try {
        return ok(typeof token === "string" ? token : await token());
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        return err({ kind: "auth", message });
      }
    },
    {
      priced: false,
      route: { namespaced: true, maxOutputTokens: false },
      list: async (client) =>
        listed(client, "models", (value) => {
          const parsed = planModelList.safeParse(value);
          return parsed.success
            ? parsed.data.models
                .filter((model) => (model.visibility ?? "list") === "list")
                .map((model) => model.slug)
            : undefined;
        }),
    },
  );
}

function create(options: Options, credentials: Credentials, account: Account): Provider {
  const client = new Client(
    options.baseUrl ?? "https://api.openai.com/v1",
    options.fetch ?? fetch,
    credentials,
  );
  const settings = new Settings();
  return {
    id: providerId,
    model: (id) => ({
      id,
      provider: providerId,
      info: () => {
        const profile = catalogProfile(id, account.priced);
        return Promise.resolve(
          profile === undefined
            ? err({ kind: "unsupported", message: `${id} is not in the catalog.` })
            : ok(profile.info),
        );
      },
      stream: (input, call) =>
        streamTurn(
          {
            client,
            producer: { provider: providerId, model: id },
            route: account.route,
            settings,
            profile: () => Promise.resolve(ok(catalogProfile(id, account.priced))),
          },
          input,
          call ?? {},
        ),
    }),
    models: async () => {
      const ids = await account.list(client);
      if (!ids.ok) {
        return ok(catalogProfiles(account.priced).map((profile) => profile.info));
      }
      return ok(
        ids.value.flatMap((id): readonly ModelInfo[] => {
          const profile = catalogProfile(id, account.priced);
          return profile === undefined ? [] : [profile.info];
        }),
      );
    },
  };
}

async function listed(
  client: Client,
  path: string,
  ids: (value: unknown) => readonly string[] | undefined,
): Promise<Result<readonly string[], ProviderError>> {
  const response = await client.get(path);
  if (!response.ok) {
    return response;
  }
  const found = ids(response.value);
  return found === undefined
    ? err({ kind: "malformed", message: "OpenAI listed models in an unexpected shape." })
    : ok(found);
}
