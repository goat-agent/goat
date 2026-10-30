export function isTimeZone(name: string): boolean {
  try {
    Temporal.Now.plainDateISO(name);
    return true;
  } catch {
    return false;
  }
}

export function dateIn(timeZone: string, at: Date): string {
  return at.toTemporalInstant().toZonedDateTimeISO(timeZone).toPlainDate().toString();
}
