import type { ToolSet } from "ai";
import { searchTools } from "./search";
import { timeTools } from "./time";
import { webTools } from "./web";

export const tools = { ...webTools, ...timeTools, ...searchTools };

export const deferredTools: ToolSet = {};
