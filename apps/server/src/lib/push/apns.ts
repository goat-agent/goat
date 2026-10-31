import { connect } from "node:http2";
import { env } from "../../env";
import type { Push } from "./index";

export const isConfigured = Boolean(env.APNS_KEY && env.APNS_KEY_ID && env.APNS_TEAM_ID);

const ORIGIN =
  env.APNS_ENVIRONMENT === "production"
    ? "https://api.push.apple.com"
    : "https://api.sandbox.push.apple.com";

// Apple refuses provider tokens older than an hour and throttles ones renewed every few minutes.
const TOKEN_LIFETIME_MS = 50 * 60_000;
const UNREGISTERED = new Set(["BadDeviceToken", "Unregistered", "DeviceTokenNotForTopic"]);

let providerToken: { value: string; issuedAt: number } | undefined;

/** "unregistered" means the device token will never work again and should be forgotten. */
export async function send(
  deviceToken: string,
  push: Push,
): Promise<"sent" | "unregistered" | "failed"> {
  const payload = JSON.stringify({
    aps: {
      alert: { title: push.title, body: push.body },
      sound: "default",
      "thread-id": push.agentId,
      "interruption-level": "active",
    },
    agentId: push.agentId,
  });
  const headers = {
    ":method": "POST",
    ":path": `/3/device/${deviceToken}`,
    authorization: `bearer ${await bearer()}`,
    "apns-topic": env.APNS_TOPIC,
    "apns-push-type": "alert",
    "apns-priority": "10",
    "apns-collapse-id": push.collapseId,
  };

  const session = connect(ORIGIN);
  try {
    const { status, body } = await new Promise<{ status: number; body: string }>(
      (resolve, reject) => {
        session.on("error", reject);
        const request = session.request(headers);
        let code = 0;
        let text = "";
        request.setEncoding("utf8");
        request.on("response", (response) => {
          code = Number(response[":status"]);
        });
        request.on("data", (chunk: string) => {
          text += chunk;
        });
        request.on("end", () => resolve({ status: code, body: text }));
        request.on("error", reject);
        request.end(payload);
      },
    );
    if (status === 200) return "sent";
    const reason = parseReason(body);
    return status === 410 || UNREGISTERED.has(reason) ? "unregistered" : "failed";
  } finally {
    session.close();
  }
}

async function bearer(): Promise<string> {
  if (providerToken && Date.now() - providerToken.issuedAt < TOKEN_LIFETIME_MS) {
    return providerToken.value;
  }
  const issuedAt = Date.now();
  const header = encode({ alg: "ES256", kid: env.APNS_KEY_ID });
  const claims = encode({ iss: env.APNS_TEAM_ID, iat: Math.floor(issuedAt / 1000) });
  const signature = await crypto.subtle.sign(
    { name: "ECDSA", hash: "SHA-256" },
    await signingKey(),
    new TextEncoder().encode(`${header}.${claims}`),
  );
  const value = `${header}.${claims}.${Buffer.from(signature).toString("base64url")}`;
  providerToken = { value, issuedAt };
  return value;
}

function signingKey(): Promise<CryptoKey> {
  // Environment files keep the .p8 key on one line with escaped newlines.
  const pem = (env.APNS_KEY ?? "").replaceAll("\\n", "\n");
  const der = Buffer.from(pem.replace(/-----[^-]+-----|\s/g, ""), "base64");
  return crypto.subtle.importKey("pkcs8", der, { name: "ECDSA", namedCurve: "P-256" }, false, [
    "sign",
  ]);
}

function encode(value: object): string {
  return Buffer.from(JSON.stringify(value)).toString("base64url");
}

function parseReason(body: string): string {
  try {
    const parsed: unknown = JSON.parse(body);
    return typeof parsed === "object" && parsed && "reason" in parsed ? String(parsed.reason) : "";
  } catch {
    return "";
  }
}
