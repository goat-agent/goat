import { tool } from "ai";
import { z } from "zod";

export const time = tool({
  description: "Returns the current date and time in the user's time zone.",
  inputSchema: z.object({}),
  contextSchema: z.object({ timeZone: z.string() }),
  execute: (_, { context }) => {
    return {
      now: Temporal.Now.zonedDateTimeISO(context.timeZone).toString({ smallestUnit: "minute" }),
    };
  },
});
