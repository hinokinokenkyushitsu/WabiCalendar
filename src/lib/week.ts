/**
 * Calendar arithmetic for the week view. All of it in local wall-clock time,
 * because that is what the grid draws.
 */

export const MINUTES_PER_DAY = 24 * 60;
export const DAYS_PER_WEEK = 7;
export const MINUTES_PER_WEEK = MINUTES_PER_DAY * DAYS_PER_WEEK;

/** What a drag snaps to, and the shortest block the grid will make. */
export const SLOT_MINUTES = 15;

/** Local midnight of the day `date` falls in. */
export function startOfDay(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate());
}

/**
 * `days` later, at local midnight.
 *
 * Built from the calendar fields rather than by adding milliseconds, so the two
 * days a year that are not 24 hours long still land on midnight.
 */
export function addDays(date: Date, days: number): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate() + days);
}

/** Local midnight of the Monday on or before `date`. */
export function startOfWeek(date: Date): Date {
  const day = date.getDay();
  // getDay() calls Sunday 0; we want it at the end of the week, not the start.
  const sinceMonday = (day + 6) % 7;
  return addDays(date, -sinceMonday);
}

/** The 8 local midnights bounding the 7 days of the week starting `weekStart`. */
export function dayBoundaries(weekStart: Date): Date[] {
  return Array.from({ length: DAYS_PER_WEEK + 1 }, (_, i) => addDays(weekStart, i));
}

/** `day` at `minutes` past its local midnight. */
export function atMinutes(day: Date, minutes: number): Date {
  return new Date(day.getFullYear(), day.getMonth(), day.getDate(), 0, minutes);
}

/** Wall-clock minutes since local midnight. */
export function minutesIntoDay(at: Date): number {
  return at.getHours() * 60 + at.getMinutes() + at.getSeconds() / 60;
}

const MS_PER_DAY = 24 * 60 * 60 * 1000;

/**
 * An instant as minutes from `weekStart`, the grid's own coordinate system.
 *
 * Wall-clock rather than elapsed, so it stays aligned with the hour labels
 * across a daylight-saving change. The day index comes from rounding the gap
 * between two local midnights, which lands on a whole number even when one of
 * those days was 23 or 25 hours long.
 *
 * Instants outside the week are fine, and come back negative or past
 * `MINUTES_PER_WEEK`: an event can start on the Sunday before the one shown.
 */
export function toWeekMinute(at: Date, weekStart: Date): number {
  const index = Math.round((startOfDay(at).getTime() - startOfDay(weekStart).getTime()) / MS_PER_DAY);
  return index * MINUTES_PER_DAY + minutesIntoDay(at);
}

/** The inverse, resolved against the real calendar. */
export function fromWeekMinute(minute: number, weekStart: Date): Date {
  const index = Math.floor(minute / MINUTES_PER_DAY);
  return atMinutes(addDays(weekStart, index), minute - index * MINUTES_PER_DAY);
}

export function snapMinutes(minutes: number): number {
  return Math.round(minutes / SLOT_MINUTES) * SLOT_MINUTES;
}

export function clamp(value: number, low: number, high: number): number {
  return Math.min(Math.max(value, low), high);
}

export function isSameDay(a: Date, b: Date): boolean {
  return (
    a.getFullYear() === b.getFullYear() &&
    a.getMonth() === b.getMonth() &&
    a.getDate() === b.getDate()
  );
}

const WEEKDAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

export function weekdayLabel(index: number): string {
  return WEEKDAYS[index] ?? "";
}

export function formatTime(at: Date): string {
  const hours = String(at.getHours()).padStart(2, "0");
  const minutes = String(at.getMinutes()).padStart(2, "0");
  return `${hours}:${minutes}`;
}

// Spelled out here rather than taken from `toLocaleString`, so the header reads
// the same whatever locale the machine happens to be set to.
const MONTHS = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
];

function monthOf(date: Date): string {
  return `${MONTHS[date.getMonth()]} ${date.getFullYear()}`;
}

/** "July 2026" — the months the visible week actually covers. */
export function monthLabel(days: readonly Date[]): string {
  const first = days[0];
  const last = days[DAYS_PER_WEEK - 1];
  if (first === undefined || last === undefined) {
    return "";
  }
  if (first.getFullYear() === last.getFullYear() && first.getMonth() === last.getMonth()) {
    return monthOf(first);
  }
  return `${monthOf(first)} – ${monthOf(last)}`;
}
