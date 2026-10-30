import { github } from "./github";
import { google } from "./google";
import type { Integration } from "./integration";
import { linear } from "./linear";
import { notion } from "./notion";
import { slack } from "./slack";

const defined: Record<string, Integration> = { github, linear, notion, slack, google };

export const integrations: Record<string, Integration> = Object.fromEntries(
  Object.entries(defined).filter(([, integration]) => integration.auth.client),
);
