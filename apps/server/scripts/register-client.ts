// Registers goat with a server that supports dynamic client registration (RFC 7591).
import { z } from "zod";
import { canva } from "../src/lib/integrations/canva";
import { granola } from "../src/lib/integrations/granola";
import type { Integration } from "../src/lib/integrations/integration";
import { linear } from "../src/lib/integrations/linear";
import { notion } from "../src/lib/integrations/notion";
import { discover, redirectUri } from "../src/lib/integrations/oauth";
import { readwise } from "../src/lib/integrations/readwise";
import { todoist } from "../src/lib/integrations/todoist";

const registrable: Record<string, Integration> = {
  linear,
  notion,
  todoist,
  granola,
  readwise,
  canva,
};

const Registered = z.object({ client_id: z.string() });

const id = process.argv[2];
const integration = id ? registrable[id] : undefined;
if (!id || !integration) {
  throw new Error(`usage: bun run register-client <${Object.keys(registrable).join("|")}>`);
}

const { registration_endpoint: registrationEndpoint } = await discover(integration.auth.issuer);
if (!registrationEndpoint) throw new Error(`${id} does not advertise client registration`);

const response = await fetch(registrationEndpoint, {
  method: "POST",
  headers: { "content-type": "application/json" },
  body: JSON.stringify({
    client_name: "Goat",
    redirect_uris: [redirectUri(id)],
    grant_types: ["authorization_code", "refresh_token"],
    response_types: ["code"],
    token_endpoint_auth_method: "none",
  }),
});
if (!response.ok)
  throw new Error(`registration failed with ${response.status}: ${await response.text()}`);

const { client_id: clientId } = Registered.parse(await response.json());
process.stdout.write(`${id.toUpperCase()}_CLIENT_ID=${clientId}\n`);
