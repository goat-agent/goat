import type { ToolSet } from "ai";

export type Fetch = (input: string | URL | Request, init?: RequestInit) => Promise<Response>;

export type Account = { id: string; name: string };

export type Identity = {
  fetch: Fetch;
  claims?: { sub: string; email?: string; name?: string };
  callTool: (name: string, input?: Record<string, unknown>) => Promise<unknown>;
};

export type OAuth = {
  type: "oauth";
  issuer: string;
  client: { id: string; secret?: string } | undefined;
  scopes: readonly string[];
  authorizeParams?: Record<string, string>;
  /** Defaults to the ID token's subject and email. */
  identify?: (identity: Identity) => Promise<Account>;
};

export type Integration = {
  name: string;
  /** English: the model searches for tools in English. */
  description: string;
  auth: OAuth;
  mcp?: { url: string; tools: readonly string[] };
  tools?: ToolSet;
};
