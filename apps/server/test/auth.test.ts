import { beforeEach, describe, expect, test } from "bun:test";
import { bearer, client, codeFor, resetDatabase, setName, signIn, status } from "./helpers";

beforeEach(resetDatabase);

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

  test("rejects a wrong code", async () => {
    await client.auth["email-code"].$post({ json: { email: "minho@example.com" } });
    const wrong = codeFor("minho@example.com") === "000000" ? "111111" : "000000";
    const response = await client.auth["sign-in"].$post({
      json: { email: "minho@example.com", code: wrong },
    });
    expect(status(response)).toBe(401);
  });

  test("keeps the agent funnel closed until the user has a name", async () => {
    const token = await signIn("minho@example.com");
    expect(status(await client.agents.$post({ json: {} }, bearer(token)))).toBe(409);

    await setName(token, "민호");
    expect(status(await client.agents.$post({ json: {} }, bearer(token)))).toBe(201);
  });
});
