import { z } from "zod";

export const env = z
  .object({
    VITE_API_URL: z.union([z.url(), z.string().startsWith("/")]),
  })
  .parse(import.meta.env);
