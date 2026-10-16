import { z } from "zod";

const loopback = new Set(["localhost", "127.0.0.1", "[::1]"]);

export const secureUrl = z.url({ protocol: /^https?$/u, abort: true }).refine((value) => {
  const url = new URL(value);
  return url.protocol === "https:" || loopback.has(url.hostname);
}, "Use https, or http only for localhost.");
