import { describe, expect, test } from "bun:test";
import { InvalidRecurrence, nextRun, parseRule } from "../src/lib/jobs/recurrence";

const at = (zoned: string) => Temporal.ZonedDateTime.from(zoned).toInstant();
const start = (time: string) => Temporal.PlainDateTime.from(time);

function runs(rrule: string | null, from: string, timeZone: string, count = 3): string[] {
  const schedule = { start: start(from.slice(0, 16)), rrule };
  const found: string[] = [];
  let after = at(`${from}[${timeZone}]`).subtract({ minutes: 1 });
  for (let i = 0; i < count; i++) {
    const next = nextRun(schedule, timeZone, after);
    if (!next) break;
    found.push(next.toZonedDateTimeISO(timeZone).toString({ smallestUnit: "minute" }));
    after = next;
  }
  return found;
}

describe("recurrence", () => {
  test("runs a one-time schedule once", () => {
    expect(runs(null, "2026-11-01T09:00", "Asia/Seoul")).toEqual([
      "2026-11-01T09:00+09:00[Asia/Seoul]",
    ]);
  });

  test("expresses what cron cannot", () => {
    expect(runs("FREQ=MONTHLY;BYDAY=-1FR", "2026-11-27T17:00", "Asia/Seoul")).toEqual([
      "2026-11-27T17:00+09:00[Asia/Seoul]",
      "2026-12-25T17:00+09:00[Asia/Seoul]",
      "2027-01-29T17:00+09:00[Asia/Seoul]",
    ]);
    expect(runs("FREQ=WEEKLY;INTERVAL=2;BYDAY=TU", "2026-11-03T14:00", "Asia/Seoul")).toEqual([
      "2026-11-03T14:00+09:00[Asia/Seoul]",
      "2026-11-17T14:00+09:00[Asia/Seoul]",
      "2026-12-01T14:00+09:00[Asia/Seoul]",
    ]);
  });

  test("stops after COUNT", () => {
    expect(runs("FREQ=DAILY;COUNT=2", "2026-11-01T07:00", "Asia/Seoul", 5)).toHaveLength(2);
  });

  test("keeps the wall-clock time across daylight saving", () => {
    expect(runs("FREQ=DAILY", "2026-10-31T08:00", "America/New_York")).toEqual([
      "2026-10-31T08:00-04:00[America/New_York]",
      "2026-11-01T08:00-05:00[America/New_York]",
      "2026-11-02T08:00-05:00[America/New_York]",
    ]);
  });

  test("runs a skipped time at the next valid time and a repeated time once", () => {
    expect(runs("FREQ=DAILY", "2027-03-13T02:30", "America/New_York")).toEqual([
      "2027-03-13T02:30-05:00[America/New_York]",
      "2027-03-14T03:30-04:00[America/New_York]",
      "2027-03-15T02:30-04:00[America/New_York]",
    ]);
    expect(runs("FREQ=DAILY", "2026-10-31T01:30", "America/New_York")).toEqual([
      "2026-10-31T01:30-04:00[America/New_York]",
      "2026-11-01T01:30-04:00[America/New_York]",
      "2026-11-02T01:30-05:00[America/New_York]",
    ]);
  });

  test("follows the user to another time zone", () => {
    const schedule = { start: start("2026-11-01T08:00"), rrule: "FREQ=DAILY" };
    const after = at("2026-11-05T00:00[UTC]");
    expect(nextRun(schedule, "Asia/Seoul", after)?.toString()).toBe("2026-11-05T23:00:00Z");
    expect(nextRun(schedule, "Europe/London", after)?.toString()).toBe("2026-11-05T08:00:00Z");
  });

  test("rejects rules it cannot or should not run", () => {
    expect(() => parseRule("every day")).toThrow(InvalidRecurrence);
    expect(() => parseRule("FREQ=MINUTELY")).toThrow(InvalidRecurrence);
    expect(parseRule("RRULE:FREQ=WEEKLY;BYDAY=MO").options.byweekday).toEqual([0]);
  });
});
