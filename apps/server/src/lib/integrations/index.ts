import { canva } from "./canva";
import { dropbox } from "./dropbox";
import { github } from "./github";
import { google } from "./google";
import { granola } from "./granola";
import type { Integration } from "./integration";
import { linear } from "./linear";
import { notion } from "./notion";
import { readwise } from "./readwise";
import { slack } from "./slack";
import { todoist } from "./todoist";

const defined: Record<string, Integration> = {
  github,
  linear,
  notion,
  slack,
  google,
  todoist,
  granola,
  readwise,
  dropbox,
  canva,
};

export const integrations: Record<string, Integration> = Object.fromEntries(
  Object.entries(defined).filter(([, integration]) => integration.auth.client),
);
