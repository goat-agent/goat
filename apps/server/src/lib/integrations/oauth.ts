import { createHash, randomBytes } from "node:crypto";
import { z } from "zod";
import { env } from "../../env";
import type { Credentials } from "./credentials";
import type { Identity, OAuth } from "./integration";

const Metadata = z.object({
  issuer: z.string(),
  authorization_endpoint: z.url(),
  token_endpoint: z.url(),
  revocation_endpoint: z.url().optional(),
  registration_endpoint: z.url().optional(),
  token_endpoint_auth_methods_supported: z.array(z.string()).optional(),
});

type Metadata = z.infer<typeof Metadata>;

const TokenResponse = z.object({
  access_token: z.string().min(1),
  refresh_token: z.string().optional(),
  expires_in: z.coerce.number().positive().optional(),
  scope: z.string().optional(),
  id_token: z.string().optional(),
});

const TokenError = z.object({ error: z.string() });

const Claims = z.object({
  sub: z.string(),
  email: z.string().optional(),
  name: z.string().optional(),
});

const REQUEST_TIMEOUT_MS = 15_000;

export class GrantRejected extends Error {}

type Client = { id: string; secret?: string };

const discovered = new Map<string, Promise<Metadata>>();

export function redirectUri(integrationId: string): string {
  return new URL(`/integrations/${integrationId}/callback`, env.BETTER_AUTH_URL).href;
}

export function createVerifier(): string {
  return randomBytes(32).toString("base64url");
}

export function createState(): string {
  return randomBytes(32).toString("base64url");
}

export async function authorizationUrl(
  auth: OAuth,
  client: Client,
  request: { integrationId: string; state: string; verifier: string; resource?: string },
): Promise<string> {
  const metadata = await discover(auth.issuer);
  const url = new URL(metadata.authorization_endpoint);
  const params: Record<string, string> = {
    response_type: "code",
    client_id: client.id,
    redirect_uri: redirectUri(request.integrationId),
    state: request.state,
    code_challenge: createHash("sha256").update(request.verifier).digest("base64url"),
    code_challenge_method: "S256",
    // Asks which account to use, so a second account or a broken one can be chosen.
    prompt: "select_account",
    ...(auth.scopes.length > 0 ? { scope: auth.scopes.join(" ") } : {}),
    ...(request.resource ? { resource: request.resource } : {}),
    ...auth.authorizeParams,
  };
  for (const [name, value] of Object.entries(params)) url.searchParams.set(name, value);
  return url.href;
}

export async function exchangeCode(
  auth: OAuth,
  client: Client,
  request: { integrationId: string; code: string; verifier: string; resource?: string },
): Promise<{ credentials: Credentials; claims?: Identity["claims"] }> {
  const response = await requestToken(auth, client, {
    grant_type: "authorization_code",
    code: request.code,
    redirect_uri: redirectUri(request.integrationId),
    code_verifier: request.verifier,
    ...(request.resource ? { resource: request.resource } : {}),
  });
  return {
    credentials: credentialsFrom(response),
    ...(response.id_token ? { claims: claimsOf(response.id_token) } : {}),
  };
}

export async function refresh(
  auth: OAuth,
  client: Client,
  credentials: Credentials,
  resource?: string,
): Promise<Credentials> {
  if (!credentials.refreshToken)
    throw new GrantRejected("the token expired and cannot be refreshed");
  const response = await requestToken(auth, client, {
    grant_type: "refresh_token",
    refresh_token: credentials.refreshToken,
    ...(resource ? { resource } : {}),
  });
  // Services that do not rotate refresh tokens omit them from the response.
  return {
    ...credentialsFrom(response),
    refreshToken: response.refresh_token ?? credentials.refreshToken,
  };
}

export async function revoke(auth: OAuth, client: Client, credentials: Credentials): Promise<void> {
  const metadata = await discover(auth.issuer);
  if (!metadata.revocation_endpoint) return;
  const token = credentials.refreshToken ?? credentials.accessToken;
  const hint = credentials.refreshToken ? "refresh_token" : "access_token";
  const response = await post(metadata, metadata.revocation_endpoint, client, {
    token,
    token_type_hint: hint,
  });
  if (!response.ok) throw new Error(`revocation failed with ${response.status}`);
}

async function requestToken(
  auth: OAuth,
  client: Client,
  params: Record<string, string>,
): Promise<z.infer<typeof TokenResponse>> {
  const metadata = await discover(auth.issuer);
  const response = await post(metadata, metadata.token_endpoint, client, params);
  const body: unknown = await response.json();

  // Some services report errors with a 200 status, so the body decides.
  const failure = TokenError.safeParse(body);
  if (failure.success) {
    const message = `token request failed: ${failure.data.error}`;
    throw failure.data.error === "invalid_grant" ? new GrantRejected(message) : new Error(message);
  }
  if (!response.ok) throw new Error(`token request failed with ${response.status}`);
  return TokenResponse.parse(body);
}

function post(
  metadata: Metadata,
  endpoint: string,
  client: Client,
  params: Record<string, string>,
): Promise<Response> {
  const body = new URLSearchParams({ ...params, client_id: client.id });
  const headers: Record<string, string> = {
    accept: "application/json",
    "content-type": "application/x-www-form-urlencoded",
  };
  if (client.secret) {
    const methods = metadata.token_endpoint_auth_methods_supported;
    if (!methods || methods.includes("client_secret_post")) {
      body.set("client_secret", client.secret);
    } else {
      const basic = `${encodeURIComponent(client.id)}:${encodeURIComponent(client.secret)}`;
      headers.authorization = `Basic ${Buffer.from(basic).toString("base64")}`;
    }
  }
  return fetch(endpoint, {
    method: "POST",
    headers,
    body,
    signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
  });
}

function credentialsFrom(response: z.infer<typeof TokenResponse>): Credentials {
  return {
    type: "oauth",
    accessToken: response.access_token,
    ...(response.refresh_token ? { refreshToken: response.refresh_token } : {}),
    ...(response.expires_in
      ? { expiresAt: new Date(Date.now() + response.expires_in * 1000).toISOString() }
      : {}),
    ...(response.scope ? { scope: response.scope } : {}),
  };
}

// The ID token comes straight from the token endpoint over TLS, which OpenID Connect accepts in
// place of checking its signature.
function claimsOf(idToken: string): Identity["claims"] {
  const payload = idToken.split(".")[1];
  if (!payload) throw new Error("malformed ID token");
  return Claims.parse(JSON.parse(Buffer.from(payload, "base64url").toString("utf8")));
}

export function discover(issuer: string): Promise<Metadata> {
  const cached = discovered.get(issuer);
  if (cached) return cached;
  const pending = fetchMetadata(issuer).catch((error: unknown) => {
    discovered.delete(issuer);
    throw error;
  });
  discovered.set(issuer, pending);
  return pending;
}

async function fetchMetadata(issuer: string): Promise<Metadata> {
  const url = new URL(issuer);
  const path = url.pathname === "/" ? "" : url.pathname;
  // RFC 8414 inserts the well-known segment before the issuer's path; OpenID Connect appends it.
  const candidates = [
    `${url.origin}/.well-known/oauth-authorization-server${path}`,
    `${issuer.replace(/\/$/, "")}/.well-known/openid-configuration`,
  ];
  for (const candidate of candidates) {
    const response = await fetch(candidate, {
      headers: { accept: "application/json" },
      signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
    });
    if (!response.ok) continue;
    const metadata = Metadata.parse(await response.json());
    if (metadata.issuer !== issuer)
      throw new Error(`metadata for ${issuer} names ${metadata.issuer}`);
    return metadata;
  }
  throw new Error(`no authorization server metadata for ${issuer}`);
}

