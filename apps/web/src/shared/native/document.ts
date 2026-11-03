import { z } from "zod";
import { contract } from "./contract";

/** Where the Mac app's Bridge package reads the contract from. */
export const bridgeFile = new URL(
  "../../../../macos/Bridge/Sources/Bridge/openapi.json",
  import.meta.url,
);

/**
 * The contract as an OpenAPI document with only components, the form Swift OpenAPI Generator
 * reads. There are no paths: the messages travel over WebKit, not HTTP.
 */
export function bridgeDocument(): string {
  const { schemas } = z.toJSONSchema(contract, { uri: (id) => `#/components/schemas/${id}` });
  const components = Object.fromEntries(
    Object.entries(schemas).map(([id, { $id: _id, $schema: _schema, ...schema }]) => [id, schema]),
  );
  const document = {
    openapi: "3.1.0",
    info: { title: "Goat native bridge", version: "1" },
    paths: {},
    components: { schemas: components },
  };
  return `${JSON.stringify(document, null, 2)}\n`;
}
