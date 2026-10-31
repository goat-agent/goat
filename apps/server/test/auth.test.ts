import { beforeEach, describe, expect, test } from "bun:test";
import { webOrigin } from "../src/env";
import { app } from "../src/index";
import { mailbox } from "./fakes";
import { bearer, client, codeFor, resetDatabase, signIn, status } from "./helpers";

beforeEach(resetDatabase);

function sendCode(email: string, acceptLanguage?: string) {
  return client.auth["email-code"].$post(
    { json: { email } },
    { headers: acceptLanguage ? { "accept-language": acceptLanguage } : {} },
  );
}

describe("auth", () => {
  test("rejects requests without a session", async () => {
    const response = await client.agents.$get();
    expect(status(response)).toBe(401);
  });

  test("signs in with an emailed code and returns a bearer token", async () => {
    const token = await signIn("minho@example.com");
    const response = await client.me.$get(undefined, bearer(token));
    if (!response.ok) throw new Error(`get me failed with ${response.status}`);
    expect(await response.json()).toMatchObject({ email: "minho@example.com", name: "" });
  });

  test("writes the code email in the language the device asks for", async () => {
    await sendCode("ko@example.com", "ko-KR, en;q=0.8");
    await sendCode("en@example.com", "fr-FR, ko;q=0, en;q=0.5");
    await sendCode("default@example.com");

    expect(mailbox.get("ko@example.com")).toStartWith("로그인 코드는");
    expect(mailbox.get("en@example.com")).toStartWith("Your sign-in code");
    expect(mailbox.get("default@example.com")).toStartWith("Your sign-in code");
  });

  test("rejects a wrong code", async () => {
    await client.auth["email-code"].$post({ json: { email: "minho@example.com" } });
    const wrong = codeFor("minho@example.com") === "000000" ? "111111" : "000000";
    const response = await client.auth["sign-in"].$post({
      json: { email: "minho@example.com", code: wrong },
    });
    expect(status(response)).toBe(401);
  });

  test("keeps a time zone the server knows and rejects any other", async () => {
    const token = await signIn("minho@example.com");
    const set = (timeZone: string) => client.me.$patch({ json: { timeZone } }, bearer(token));

    expect(status(await set("GMT+0900"))).toBe(400);
    expect(status(await client.me.$patch({ json: {} }, bearer(token)))).toBe(400);
    expect(status(await set("Asia/Seoul"))).toBe(200);

    const response = await client.me.$get(undefined, bearer(token));
    if (!response.ok) throw new Error(`get me failed with ${response.status}`);
    expect(await response.json()).toMatchObject({ timeZone: "Asia/Seoul" });
  });

  test("signing out ends the session so the token stops working", async () => {
    const token = await signIn("minho@example.com");
    expect(status(await client.auth["sign-out"].$post(undefined, bearer(token)))).toBe(204);
    expect(status(await client.me.$get(undefined, bearer(token)))).toBe(401);
  });

  test("gives a browser a session cookie that works without the token", async () => {
    await client.auth["email-code"].$post({ json: { email: "minho@example.com" } });
    const signedIn = await client.auth["sign-in"].$post({
      json: {
        email: "minho@example.com",
        code: codeFor("minho@example.com"),
        device: { installationId: crypto.randomUUID(), name: "Chrome · Mac", platform: "web" },
      },
    });
    expect(status(signedIn)).toBe(200);
    const cookie = sessionCookie(signedIn.headers);
    expect(signedIn.headers.getSetCookie().join()).toContain("HttpOnly");

    const me = await client.me.$get(undefined, { headers: { cookie } });
    expect(status(me)).toBe(200);
    const listed = await client.devices.$get(undefined, { headers: { cookie } });
    if (!listed.ok) throw new Error(`list devices failed with ${listed.status}`);
    expect((await listed.json()).devices).toMatchObject([{ platform: "web", isCurrent: true }]);

    const signedOut = await client.auth["sign-out"].$post(undefined, { headers: { cookie } });
    expect(status(signedOut)).toBe(204);
    expect(signedOut.headers.getSetCookie().join()).toContain("Max-Age=0");
    expect(status(await client.me.$get(undefined, { headers: { cookie } }))).toBe(401);
  });

  test("lets only the web app call with credentials from another origin", async () => {
    const preflight = await app.request("/me", {
      method: "OPTIONS",
      headers: {
        origin: webOrigin,
        "access-control-request-method": "GET",
      },
    });
    expect(preflight.headers.get("access-control-allow-origin")).toBe(webOrigin);
    expect(preflight.headers.get("access-control-allow-credentials")).toBe("true");
  });

  test("lets a user without a name create an agent", async () => {
    const token = await signIn("minho@example.com");
    expect(status(await client.agents.$post({ json: {} }, bearer(token)))).toBe(201);
  });
});

function sessionCookie(headers: Headers): string {
  const cookie = headers
    .getSetCookie()
    .map((line) => line.split(";")[0] ?? "")
    .find((pair) => pair.includes("session_token="));
  if (!cookie) throw new Error("no session cookie was set");
  return cookie;
}
