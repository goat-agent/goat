import { expect, test } from "bun:test";
import { openapiFile } from "../scripts/openapi-file";
import { app } from "../src/index";
import { document } from "../src/lib/openapi";

test("the OpenAPI document the iOS app builds from matches the routes", async () => {
  const committed = await Bun.file(openapiFile).text();
  expect(committed).toBe(`${JSON.stringify(app.getOpenAPI31Document(document), null, 2)}\n`);
});
