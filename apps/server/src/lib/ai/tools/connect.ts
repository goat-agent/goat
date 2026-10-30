import { tool } from "ai";
import { z } from "zod";
import { matchIntegration } from "../../integrations/connections";

export const connectTools = {
  connect: tool({
    description:
      "Show the user a button in the conversation to connect a service, or to reconnect one whose connection broke. The user finishes connecting in the app; you will be told once it is connected.",
    inputSchema: z.object({
      integration: z.string().describe("The service's id, as tool_search shows it"),
    }),
    execute: ({ integration }) => {
      if (!matchIntegration(integration)) {
        throw new Error(`${integration} is not a service Goat can connect.`);
      }
      return { shown: true };
    },
  }),
};
