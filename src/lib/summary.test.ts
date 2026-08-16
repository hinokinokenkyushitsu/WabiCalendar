import { describe, expect, it } from "vitest";

import { barWidths, formatDuration, formatRatio, summarise } from "./summary";
import type { CalEvent } from "../types/calendar";
import type { Outcome, Session } from "../types/session";
import type { Phase } from "../types/timer";

/** Mid-July, so no populated zone changes its clocks and the cases travel. */
const MONDAY = new Date(2026, 6, 20);
const NEXT_MONDAY = new Date(2026, 6, 27);

let counter = 0;

function event(
  day: number,
  fromHour: number,
  toHour: number,
  extra: Partial<CalEvent> = {},
): CalEvent {
  counter += 1;
  return {
    id: `e${counter}`,
    uid: `e${counter}`,
    summary: "写论文",
    start: new Date(2026, 6, day, fromHour),
    end: new Date(2026, 6, day, toHour),
    recurring: false,
    allDay: false,
    ...extra,
  };
}

function session(
  day: number,
  hour: number,
  actualSec: number,
  kind: Phase = "work",
  outcome: Outcome = "completed",
): Session {
  counter += 1;
  return {
    id: `s${counter}`,
    kind,
    plannedSec: 1500,
    actualSec,
    start: new Date(2026, 6, day, hour),
    end: new Date(2026, 6, day, hour, Math.round(actualSec / 60)),
    outcome,
    label: null,
  };
}

describe("summarise", () => {
  it("adds up planned blocks and counted focus", () => {
    const summary = summarise(
      [event(20, 9, 11), event(22, 14, 15)],
      [session(20, 9, 1500), session(20, 10, 1500)],
      MONDAY,
      NEXT_MONDAY,
    );

    expect(summary.plannedSec).toBe(3 * 3600);
    expect(summary.focusedSec).toBe(3000);
    expect(summary.ratio).toBeCloseTo(3000 / 10800);
  });

  it("counts only the part of a block that falls inside the week", () => {
    // Starts on the Sunday before and runs into the Monday morning.
    const straddling = event(19, 22, 0, { end: new Date(2026, 6, 20, 2) });

    const summary = summarise([straddling], [], MONDAY, NEXT_MONDAY);

    expect(summary.plannedSec).toBe(2 * 3600);
  });

  it("leaves all-day events out, since one would drown the week", () => {
    const allDay = event(22, 0, 0, { end: new Date(2026, 6, 23), allDay: true });

    const summary = summarise([allDay, event(22, 9, 10)], [], MONDAY, NEXT_MONDAY);

    expect(summary.plannedSec).toBe(3600);
  });

  it("does not count breaks as focus", () => {
    const summary = summarise(
      [event(20, 9, 10)],
      [session(20, 9, 1500), session(20, 9, 300, "break")],
      MONDAY,
      NEXT_MONDAY,
    );

    expect(summary.focusedSec).toBe(1500);
  });

  /** The app itself said it could not vouch for that stretch. */
  it("does not count an invalidated session as focus", () => {
    const summary = summarise(
      [event(20, 9, 10)],
      [session(20, 9, 600, "work", "invalidated")],
      MONDAY,
      NEXT_MONDAY,
    );

    expect(summary.focusedSec).toBe(0);
  });

  it("counts an abandoned session for as far as it got", () => {
    const summary = summarise(
      [event(20, 9, 10)],
      [session(20, 9, 700, "work", "aborted")],
      MONDAY,
      NEXT_MONDAY,
    );

    expect(summary.focusedSec).toBe(700);
  });

  it("ignores sessions from other weeks", () => {
    const summary = summarise(
      [event(20, 9, 10)],
      [session(19, 9, 1500), session(20, 9, 1500), session(27, 9, 1500)],
      MONDAY,
      NEXT_MONDAY,
    );

    expect(summary.focusedSec).toBe(1500);
  });

  /** A week with no plan has no completion rate; 0% would read as a failure. */
  it("has no ratio when nothing was planned", () => {
    const summary = summarise([], [session(20, 9, 1500)], MONDAY, NEXT_MONDAY);

    expect(summary.plannedSec).toBe(0);
    expect(summary.focusedSec).toBe(1500);
    expect(summary.ratio).toBeNull();
  });

  /** Outworking the plan happened; the number should say so. */
  it("does not cap the ratio at 100%", () => {
    const summary = summarise(
      [event(20, 9, 10)],
      [session(20, 9, 1800), session(20, 10, 1800), session(20, 11, 1800)],
      MONDAY,
      NEXT_MONDAY,
    );

    expect(summary.ratio).toBeCloseTo(1.5);
  });

  it("is empty for an empty week", () => {
    expect(summarise([], [], MONDAY, NEXT_MONDAY)).toEqual({
      plannedSec: 0,
      focusedSec: 0,
      ratio: null,
    });
  });
});

describe("formatDuration", () => {
  it("drops the hour part below an hour", () => {
    expect(formatDuration(45 * 60)).toBe("45m");
    expect(formatDuration(0)).toBe("0m");
  });

  it("pads the minutes so the numbers line up in a column", () => {
    expect(formatDuration(8 * 3600 + 15 * 60)).toBe("8h15m");
    expect(formatDuration(8 * 3600 + 5 * 60)).toBe("8h05m");
    expect(formatDuration(3600)).toBe("1h00m");
  });

  it("rounds down, so an unfinished minute is not claimed", () => {
    expect(formatDuration(119)).toBe("1m");
  });
});

describe("formatRatio", () => {
  it("renders a percentage", () => {
    expect(formatRatio(0.66)).toBe("66%");
    expect(formatRatio(1.5)).toBe("150%");
  });

  it("says nothing rather than zero when there is no plan", () => {
    expect(formatRatio(null)).toBe("—");
  });
});

describe("barWidths", () => {
  it("gives the longer of the two the full row", () => {
    const widths = barWidths({ plannedSec: 10800, focusedSec: 5400, ratio: 0.5 });

    expect(widths.planned).toBe(100);
    expect(widths.focused).toBe(50);
  });

  it("lets the actual bar overrun the plan", () => {
    const widths = barWidths({ plannedSec: 3600, focusedSec: 7200, ratio: 2 });

    expect(widths.planned).toBe(50);
    expect(widths.focused).toBe(100);
  });

  it("draws nothing rather than dividing by zero", () => {
    expect(barWidths({ plannedSec: 0, focusedSec: 0, ratio: null })).toEqual({
      planned: 0,
      focused: 0,
    });
  });
});
