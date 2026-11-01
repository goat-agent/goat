import { Frequency, RRule } from "rrule";

export class InvalidRecurrence extends Error {}

// A schedule that wakes the model every minute would burn money for nothing.
const TOO_FREQUENT = new Set([Frequency.SECONDLY, Frequency.MINUTELY]);

/** Throws `InvalidRecurrence` unless the rule is an RFC 5545 RRULE goat can run. */
export function parseRule(rule: string): RRule {
  let options;
  try {
    options = RRule.parseString(rule.replace(/^RRULE:/i, ""));
  } catch (error) {
    throw new InvalidRecurrence(`Not an RRULE: ${rule}`, { cause: error });
  }
  if (options.freq === undefined) throw new InvalidRecurrence(`The RRULE needs FREQ: ${rule}`);
  if (TOO_FREQUENT.has(options.freq)) {
    throw new InvalidRecurrence("Schedules can repeat at most hourly.");
  }
  return new RRule(options);
}

/**
 * The first run strictly after `after`, or undefined when none is left. `start` is wall-clock
 * time, so the rule is evaluated on wall-clock times and only the result is placed in
 * `timeZone`: "every day at 8" stays at 8 across daylight saving changes and moves with the user.
 */
export function nextRun(
  schedule: { start: Temporal.PlainDateTime; rrule: string | null },
  timeZone: string,
  after: Temporal.Instant,
): Temporal.Instant | undefined {
  const local = after.toZonedDateTimeISO(timeZone).toPlainDateTime();
  if (!schedule.rrule) {
    return Temporal.PlainDateTime.compare(schedule.start, local) > 0
      ? place(schedule.start, timeZone)
      : undefined;
  }
  const rule = new RRule({
    ...parseRule(schedule.rrule).origOptions,
    dtstart: floating(schedule.start),
  });
  const next = rule.after(floating(local), false);
  return next ? place(fromFloating(next), timeZone) : undefined;
}

// rrule works on Dates; wall-clock times ride in them as if they were UTC.
function floating(time: Temporal.PlainDateTime): Date {
  return new Date(Date.UTC(time.year, time.month - 1, time.day, time.hour, time.minute));
}

function fromFloating(date: Date): Temporal.PlainDateTime {
  return Temporal.PlainDateTime.from(date.toISOString().slice(0, 16));
}

// A time skipped by daylight saving runs at the next valid time; a repeated one runs once.
function place(time: Temporal.PlainDateTime, timeZone: string): Temporal.Instant {
  return time.toZonedDateTime(timeZone, { disambiguation: "compatible" }).toInstant();
}
