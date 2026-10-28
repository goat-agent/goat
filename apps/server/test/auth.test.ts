import { beforeEach, describe, expect, test } from "bun:test";
import { bearer, client, resetDatabase, setName, signIn, status } from "./helpers";

beforeEach(resetDatabase);

describe("auth", () => {
  test("rejects requests without a session", async () => {
    const response = await client.agents.$get();
    expect(status(response)).toBe(401);
  });

  test("signs in with an email code and uses the bearer token", async () => {
    const token = await signIn("minho@example.com");
    const response = await client.agents.$get(undefined, bearer(token));
    expect(response.status).toBe(200);
  });

  test("keeps the agent funnel closed until the user has a name", async () => {
    const token = await signIn("minho@example.com");
    expect(status(await client.agents.$post({ json: {} }, bearer(token)))).toBe(409);

    await setName(token, "민호");
    expect(status(await client.agents.$post({ json: {} }, bearer(token)))).toBe(201);
  });
});
