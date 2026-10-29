import { app } from "../src/index";
import { document } from "../src/lib/openapi";
import { openapiFile } from "./openapi-file";

await Bun.write(openapiFile, `${JSON.stringify(app.getOpenAPI31Document(document), null, 2)}\n`);
