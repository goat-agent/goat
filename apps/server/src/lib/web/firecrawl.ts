import { z } from "zod";
import { env } from "../../env";
import type { SearchResult } from "./index";

const API = "https://api.firecrawl.dev/v2";
const SEARCH_LIMIT = 5;

const SearchResponse = z.object({
  data: z.object({
    web: z
      .array(
        z.object({
          url: z.string(),
          title: z.string().optional(),
          description: z.string().optional(),
        }),
      )
      .default([]),
  }),
});

const ScrapeResponse = z.object({
  data: z.object({
    markdown: z.string().default(""),
    metadata: z.object({
      title: z.union([z.string(), z.array(z.string())]).optional(),
      url: z.string().optional(),
    }),
  }),
});

export type Scraped = { title: string; url: string; markdown: string };

export async function search(query: string, signal?: AbortSignal): Promise<SearchResult[]> {
  const { data } = SearchResponse.parse(
    await post("/search", { query, limit: SEARCH_LIMIT }, signal),
  );
  return data.web.map((result) => ({
    title: result.title ?? result.url,
    url: result.url,
    snippet: result.description ?? "",
  }));
}

export async function scrape(url: string, signal?: AbortSignal): Promise<Scraped> {
  const { data } = ScrapeResponse.parse(
    await post("/scrape", { url, formats: ["markdown"] }, signal),
  );
  const title = Array.isArray(data.metadata.title) ? data.metadata.title[0] : data.metadata.title;
  return { title: title ?? url, url: data.metadata.url ?? url, markdown: data.markdown };
}

async function post(path: string, body: object, signal?: AbortSignal): Promise<unknown> {
  const response = await fetch(`${API}${path}`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${env.FIRECRAWL_API_KEY}`,
      "content-type": "application/json",
    },
    body: JSON.stringify(body),
    signal: signal ?? null,
  });
  if (!response.ok) throw new Error(`Firecrawl ${path} failed with ${response.status}`);
  return response.json();
}
