import { connectTools } from "./connect";
import { searchTools } from "./search";
import { timeTools } from "./time";
import { webTools } from "./web";

export const tools = { ...webTools, ...timeTools, ...searchTools, ...connectTools };
