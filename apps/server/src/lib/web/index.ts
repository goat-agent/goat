import * as firecrawl from "./firecrawl";

export type SearchResult = { title: string; url: string; snippet: string };

export type Page = {
  title: string;
  url: string;
  content: string;
  truncated: boolean;
  nextOffset?: number;
};

/** Pages longer than this are returned in parts, so one page cannot flood the conversation. */
const PAGE_CHARS = 20_000;

export function search(query: string, signal?: AbortSignal): Promise<SearchResult[]> {
  return firecrawl.search(query, signal);
}

export async function fetch(url: string, offset = 0, signal?: AbortSignal): Promise<Page> {
  const page = await firecrawl.scrape(url, signal);
  const end = offset + PAGE_CHARS;
  const truncated = end < page.markdown.length;
  return {
    title: page.title,
    url: page.url,
    content: page.markdown.slice(offset, end),
    truncated,
    ...(truncated ? { nextOffset: end } : {}),
  };
}
