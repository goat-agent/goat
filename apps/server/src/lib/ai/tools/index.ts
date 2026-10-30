import { computerTools } from "./computer";
import { connectTools } from "./connect";
import { fileTools } from "./files";
import { searchTools } from "./search";
import { timeTools } from "./time";
import { webTools } from "./web";

export const tools = {
  ...webTools,
  ...timeTools,
  ...searchTools,
  ...connectTools,
  ...fileTools,
  ...computerTools,
};
