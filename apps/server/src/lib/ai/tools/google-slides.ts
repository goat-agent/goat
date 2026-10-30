import { tool } from "ai";
import { z } from "zod";
import { callGoogle, GoogleConnection, googleScopes } from "./google";

const API = "https://slides.googleapis.com/v1/presentations";

const Presentation = z.object({ presentationId: z.string() });

export const googleSlidesTools = {
  google_slides_create: tool({
    description: "Google Slides: create a presentation. Read presentations with google_drive_read.",
    inputSchema: z.object({ title: z.string() }),
    contextSchema: GoogleConnection,
    metadata: { scope: googleScopes.presentations },
    execute: async ({ title }, { context, abortSignal }) => {
      const presentation = Presentation.parse(
        await callGoogle(context.fetch, API, {
          method: "POST",
          body: { title },
          signal: abortSignal,
        }),
      );
      const id = presentation.presentationId;
      return { presentationId: id, link: `https://docs.google.com/presentation/d/${id}/edit` };
    },
  }),
};
