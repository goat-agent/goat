import { writeFileSync } from "node:fs";
import { bridgeDocument, bridgeFile } from "../src/shared/native/document";

writeFileSync(bridgeFile, bridgeDocument());
