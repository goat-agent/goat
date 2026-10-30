// Registers goat with a server that supports dynamic client registration (RFC 7591).
import { z } from "zod";
import { linear } from "../src/lib/integrations/linear";
import { notion } from "../src/lib/integrations/notion";
import { redirectUri } from "../src/lib/integrations/oauth";

const registrable = { linear, notion };

const Metadata = z.object({ registration_endpoint: z.url() });
const Registered = z.object({ client_id: z.string() });

const id = process.argv[2];
if (id !== "linear" && id !== "notion") {
  throw new Error(`usage: bun run register-client <${Object.keys(registrable).join("|")}>`);
}

const metadata = Metadata.parse(
  await (
    await fetch(new URL("/.well-known/oauth-authorization-server", registrable[id].auth.issuer))
  ).json(),
);
const response = await fetch(metadata.registration_endpoint, {
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
