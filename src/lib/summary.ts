/**
 * The week's headline numbers: what was planned, what actually happened, and
 * how much of the plan the pomodoros covered.
 *
 * Pure, like `layout.ts`, and for the same reason — this is the claim the whole
 * app is making, so it is the part most worth pinning down in tests.
 */

import type { CalEvent } from "../types/calendar";
import { isFocus, type Session } from "../types/session";

const SEC_PER_MINUTE = 60;

export interface WeekSummary {
  /** Planned seconds inside the window. */
  plannedSec: number;
  /** Seconds of work the timer actually counted. */
  focusedSec: number;
  /**
   * `focusedSec / plannedSec`, or `null` when nothing was planned.
   *
   * Null rather than 0 or Infinity: a week with no plan has no completion rate,
   * and showing "0%" for one would read as a failure that never happened.
   * Deliberately *not* capped at 1 — outworking the plan is a real thing that
   * happened and the number should say so.
   */
  ratio: number | null;
}

/** Seconds of `[start, end)` that fall inside `[from, to)`. */
function overlapSec(start: Date, end: Date, from: Date, to: Date): number {
  const low = Math.max(start.getTime(), from.getTime());
  const high = Math.min(end.getTime(), to.getTime());
  return Math.max(0, (high - low) / 1000);
}

/**
 * Add up one week.
 *
 * The two halves are selected differently, because the two kinds of number mean
 * different things:
 *
 * - A planned block is *clipped* to the window. An event may legitimately run
 *   for three days, and only the part inside this week was planned for it.
 * - A session is taken whole if it *started* inside the window, because
 *   `actualSec` is counted time and there is no honest way to cut it in half —
 *   a session that was paused does not spread its counted seconds evenly over
 *   the wall clock. A pomodoro straddling local midnight on the Monday is the
 *   only case this can misplace, by a few minutes, into the week it began in.
 *
 * All-day events are left out entirely: a single one would add 24 hours and
 * drown every real block in the week.
 */
export function summarise(
  events: readonly CalEvent[],
  sessions: readonly Session[],
  from: Date,
  to: Date,
): WeekSummary {
  let plannedSec = 0;
  for (const event of events) {
    if (event.allDay) {
      continue;
    }
    plannedSec += overlapSec(event.start, event.end, from, to);
  }

  let focusedSec = 0;
  for (const session of sessions) {
    if (!isFocus(session) || session.start < from || session.start >= to) {
      continue;
    }
    focusedSec += session.actualSec;
  }

  return {
    plannedSec,
    focusedSec,
    ratio: plannedSec === 0 ? null : focusedSec / plannedSec,
  };
}

/**
 * "8h15m", "45m", "0m" — a duration at a glance rather than to the second.
 *
 * Rounded down to the minute: claiming an hour the user has not finished would
 * be the one direction that flatters.
 */
export function formatDuration(totalSec: number): string {
  const minutes = Math.floor(Math.max(0, totalSec) / SEC_PER_MINUTE);
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return hours === 0 ? `${rest}m` : `${hours}h${String(rest).padStart(2, "0")}m`;
}

/** "66%", or "—" when there is no plan to measure against. */
export function formatRatio(ratio: number | null): string {
  return ratio === null ? "—" : `${Math.round(ratio * 100)}%`;
}

/**
 * The two summary bars as percentages of the row, scaled so the longer one
 * fills it.
 *
 * Scaled against each other rather than against the 168 hours in a week, which
 * would leave both slivers too short to compare. An empty week gives two zeroes
 * rather than a division by zero.
 */
export function barWidths(summary: WeekSummary): { planned: number; focused: number } {
  const longest = Math.max(summary.plannedSec, summary.focusedSec);
  if (longest === 0) {
    return { planned: 0, focused: 0 };
  }
  return {
    planned: (summary.plannedSec / longest) * 100,
    focused: (summary.focusedSec / longest) * 100,
  };
}
