import { tool } from "ai";
import { z } from "zod";
import * as files from "../../files";

export const fileTools = {
  read_file: tool({
    description:
      "Open a file the user shared earlier in this conversation, using the id from its note.",
    inputSchema: z.object({ id: z.uuid().describe("The file id from the note") }),
    contextSchema: z.object({ agentId: z.uuid() }),
    execute: async ({ id }, { context }) => {
      const file = await files.find(context.agentId, id);
      if (!file?.sentAt) return { error: "No file with that id in this conversation." };
      return { id: file.id, name: file.name, mediaType: file.mediaType };
    },
    // The result holds a reference, not the bytes, so the stored conversation stays small; the
    // run swaps in the bytes before each model call.
    toModelOutput: ({ output }) =>
      output.error !== undefined
        ? { type: "error-text", value: output.error }
        : { type: "content", value: [files.reference(output)] },
  }),
};
