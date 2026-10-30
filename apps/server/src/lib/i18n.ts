import { parseAccept } from "hono/utils/accept";

const locales = ["en", "ko"] as const;

export type Locale = (typeof locales)[number];

function isLocale(language: string): language is Locale {
  return locales.some((locale) => locale === language);
}

/** The supported language the request's `Accept-Language` ranks highest, or English. */
export function localeOf(headers: Headers | undefined): Locale {
  const languages = parseAccept(headers?.get("accept-language") ?? "")
    .filter(({ q }) => q > 0)
    .map(({ type }) => type.split("-")[0]?.toLowerCase() ?? "");
  return languages.find(isLocale) ?? "en";
}
