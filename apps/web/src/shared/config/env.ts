import { z } from "zod";

export const env = z
  .object({
    // A full URL, or a path on this site for dev:mock, whose service worker answers it.
    VITE_API_URL: z.union([z.url(), z.string().startsWith("/")]),
  })
  .parse(import.meta.env);
