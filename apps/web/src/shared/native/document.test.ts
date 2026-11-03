import { readFileSync } from "node:fs";
import { expect, test } from "vitest";
import { bridgeDocument, bridgeFile } from "./document";

test("the bridge document the Mac app builds from matches the contract", () => {
  expect(readFileSync(bridgeFile, "utf8")).toBe(bridgeDocument());
});
