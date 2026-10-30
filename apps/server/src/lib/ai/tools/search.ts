import { asSchema, type JSONSchema7, tool, type ToolSet } from "ai";
import { z } from "zod";
import type { Catalog } from "../../integrations/catalog";

const LIMIT = 5;
const K1 = 1.2;
const B = 0.75;

type Entry = {
  name: string;
  description: string | undefined;
  inputSchema: JSONSchema7;
  terms: string[];
};

const Searchable = z.object({ search: z.custom<Catalog["search"]>() });
const Callable = z.object({ resolve: z.custom<Catalog["resolve"]>() });

export const searchTools = {
  tool_search: tool({
    description:
      "Find more tools by what they do, such as email, calendar, or code hosting. Returns up to five tools with their input schemas; run one with tool_call.",
    inputSchema: z.object({
      query: z.string().min(1).max(500).describe("What the tool should do, in English keywords"),
    }),
    contextSchema: Searchable,
    execute: async ({ query }, { context }) => {
      const catalog = await Promise.all(
        Object.entries(await context.search()).map(([name, target]) => describe(name, target)),
      );
      return {
        tools: rank(catalog, query).map(({ name, description, inputSchema }) => ({
          name,
          description,
          inputSchema,
        })),
      };
    },
  }),
  tool_call: tool({
    description:
      "Run a tool found with tool_search. Tools found with tool_search cannot be called directly by name; always call them through tool_call.",
    inputSchema: z.object({
      name: z.string(),
      account: z
        .string()
        .optional()
        .describe("Which connected account to use, when the service has more than one"),
      input: z.record(z.string(), z.unknown()),
    }),
    contextSchema: Callable,
    execute: async ({ name, account, input }, options) => {
      const target = await options.context.resolve(name, account);
      if (!target.execute) throw new Error(`No tool named ${name}; find it with tool_search.`);

      const checked = await asSchema(target.inputSchema).validate?.(input);
      if (checked?.success === false) throw new Error(`Invalid input: ${checked.error.message}`);

      const output: unknown = await target.execute(checked ? checked.value : input, options);
      return output;
    },
  }),
};

async function describe(name: string, target: ToolSet[string]): Promise<Entry> {
  const description = typeof target.description === "string" ? target.description : undefined;
  const inputSchema = await asSchema(target.inputSchema).jsonSchema;
  const parameters = JSON.stringify(inputSchema.properties ?? {});
  return {
    name,
    description,
    inputSchema,
    terms: terms(`${name} ${description ?? ""} ${parameters}`),
  };
}

function rank(entries: Entry[], query: string): Entry[] {
  const wanted = terms(query);
  const averageLength =
    entries.reduce((sum, entry) => sum + entry.terms.length, 0) / entries.length;
  const idf = (term: string) => {
    const containing = entries.filter((entry) => entry.terms.includes(term)).length;
    return Math.log(1 + (entries.length - containing + 0.5) / (containing + 0.5));
  };
  const score = (entry: Entry) =>
    wanted.reduce((sum, term) => {
      const frequency = entry.terms.filter((candidate) => candidate === term).length;
      const length = K1 * (1 - B + (B * entry.terms.length) / averageLength);
      return sum + (idf(term) * frequency * (K1 + 1)) / (frequency + length);
    }, 0);

  return entries
    .map((entry) => ({ entry, score: score(entry) }))
    .filter((scored) => scored.score > 0)
    .toSorted((a, b) => b.score - a.score)
    .slice(0, LIMIT)
    .map((scored) => scored.entry);
}

function terms(text: string): string[] {
  return text
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .filter(Boolean);
}
