import { z } from "zod";
import { type Method, secureUrl } from "../credentials/index.ts";
import type { Provider } from "../provider/index.ts";
import { err, ok } from "../result/index.ts";
import { apiKey } from "./provider.ts";

const apiKeyFields = z.object({
  key: z.string().regex(/^[!-~]+$/u),
  baseUrl: secureUrl.exactOptional(),
});

export const methods: readonly Method<Provider>[] = [
  {
    provider: "anthropic",
    method: "apiKey",
    connect: (fields) => {
      const options = apiKeyFields.safeParse(fields);
      return options.success ? ok(apiKey(options.data)) : err(z.prettifyError(options.error));
    },
  },
];
