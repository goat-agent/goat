const UNITS: readonly [RegExp, string][] = [
  [/(\d)\s*(?:gib|gb|기가바이트|기가)(?![a-z])/g, "$1gb"],
  [/(\d)\s*(?:mib|mb|메가바이트|메가)(?![a-z])/g, "$1mb"],
  [/(\d)\s*(?:tib|tb|테라바이트|테라)(?![a-z])/g, "$1tb"],
];

/**
 * Text as the trigram index and the query see it: composed, lowercase, words separated by single
 * spaces, and sizes written one way, so "3GB", "3 gb", and "3기가" are the same token.
 */
export function normalize(text: string): string {
  let folded = text.normalize("NFKC").toLowerCase();
  for (const [pattern, replacement] of UNITS) folded = folded.replace(pattern, replacement);
  return folded.replace(/[^\p{L}\p{N}]+/gu, " ").trim();
}

export function tokensOf(text: string): string[] {
  const normalized = normalize(text);
  return normalized ? [...new Set(normalized.split(" "))] : [];
}

/**
 * About `size` characters of `text` around the stretch that holds the most query tokens, or its
 * start when none match, as when a round was found by meaning alone.
 */
export function excerpt(text: string, tokens: readonly string[], size: number): string {
  if (text.length <= size) return text;
  const step = Math.floor(size / 6);
  let best = { start: 0, hits: 0 };
  for (let start = 0; start + size < text.length + step; start += step) {
    const window = normalize(text.slice(start, start + size));
    const hits = tokens.filter((token) => window.includes(token)).length;
    if (hits > best.hits) best = { start, hits };
  }
  const start = Math.max(0, Math.min(best.start, text.length - size));
  const end = Math.min(text.length, start + size);
  const piece = text.slice(start, end).trim();
  return `${start > 0 ? "…" : ""}${piece}${end < text.length ? "…" : ""}`;
}
