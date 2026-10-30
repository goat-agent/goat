import type { ToolSet } from "ai";
import { searchTools } from "./search";
import { time } from "./time";
import { webTools } from "./web";

const deferredTools: ToolSet = {};

export const tools = { ...webTools, time, ...searchTools(deferredTools) };
