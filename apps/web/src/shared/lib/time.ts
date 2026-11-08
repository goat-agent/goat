import { getLocale } from "@/shared/i18n/paraglide/runtime";

const DAY = 24 * 60 * 60 * 1000;

function startOfDay(date: Date): number {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
}

function daysAgo(date: Date, now = new Date()): number {
  return Math.round((startOfDay(now) - startOfDay(date)) / DAY);
}

export function listTime(iso: string): string {
  const date = new Date(iso);
  const locale = getLocale();
  const days = daysAgo(date);
  if (days === 0) return date.toLocaleTimeString(locale, { hour: "numeric", minute: "2-digit" });
  if (days === 1) return dayLabel(date);
  if (days < 7) return date.toLocaleDateString(locale, { weekday: "long" });
  return date.toLocaleDateString(locale, { month: "short", day: "numeric" });
}

export function dayLabel(date: Date): string {
  const locale = getLocale();
  const days = daysAgo(date);
  if (days <= 1) {
    const relative = new Intl.RelativeTimeFormat(locale, { numeric: "auto" }).format(-days, "day");
    return relative.charAt(0).toLocaleUpperCase(locale) + relative.slice(1);
  }
  const sameYear = date.getFullYear() === new Date().getFullYear();
  return date.toLocaleDateString(locale, {
    weekday: "long",
    month: "long",
    day: "numeric",
    ...(sameYear ? {} : { year: "numeric" }),
  });
}

export function isSameDay(a: Date, b: Date): boolean {
  return startOfDay(a) === startOfDay(b);
}

export function duration(seconds: number): string {
  const minutes = Math.floor(seconds / 60);
  return `${minutes}:${String(seconds % 60).padStart(2, "0")}`;
}
