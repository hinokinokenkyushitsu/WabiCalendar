import { describe, expect, it } from "vitest";

import {
  MINUTES_PER_DAY,
  MINUTES_PER_WEEK,
  addDays,
  atMinutes,
  dayBoundaries,
  fromWeekMinute,
  minutesIntoDay,
  snapMinutes,
  startOfWeek,
  toWeekMinute,
} from "./week";

/**
 * A week in July, deliberately: no populated time zone changes its clocks in
 * mid-July, so these cases mean the same thing wherever the tests run. Dates are
 * built with the local constructor throughout, never from a UTC string.
 */
const MONDAY = new Date(2026, 6, 20);

describe("startOfWeek", () => {
  it("finds the Monday of a midweek day", () => {
    expect(startOfWeek(new Date(2026, 6, 23, 14, 30))).toEqual(MONDAY);
  });

  it("leaves a Monday where it is, at midnight", () => {
    expect(startOfWeek(new Date(2026, 6, 20, 9, 0))).toEqual(MONDAY);
  });

  it("counts Sunday as the end of its week, not the start of the next", () => {
    expect(startOfWeek(new Date(2026, 6, 26, 23, 59))).toEqual(MONDAY);
    expect(startOfWeek(new Date(2026, 6, 27))).toEqual(new Date(2026, 6, 27));
  });
});

describe("dayBoundaries", () => {
  it("gives 8 local midnights so every day has an end", () => {
    const days = dayBoundaries(MONDAY);

    expect(days).toHaveLength(8);
    expect(days[0]).toEqual(MONDAY);
    expect(days[7]).toEqual(new Date(2026, 6, 27));
    expect(days.every((day) => minutesIntoDay(day) === 0)).toBe(true);
  });
});

describe("minutesIntoDay", () => {
  it("reads the wall clock rather than elapsed time", () => {
    expect(minutesIntoDay(new Date(2026, 6, 23, 0, 0))).toBe(0);
    expect(minutesIntoDay(new Date(2026, 6, 23, 9, 30))).toBe(570);
    expect(minutesIntoDay(new Date(2026, 6, 23, 23, 59))).toBe(1439);
  });
});

describe("toWeekMinute", () => {
  it("measures from the week's first midnight", () => {
    expect(toWeekMinute(new Date(2026, 6, 20, 0, 0), MONDAY)).toBe(0);
    expect(toWeekMinute(new Date(2026, 6, 20, 9, 30), MONDAY)).toBe(570);
  });

  it("counts whole days into the week", () => {
    // Thursday is day 3.
    expect(toWeekMinute(new Date(2026, 6, 23, 9, 0), MONDAY)).toBe(3 * MINUTES_PER_DAY + 540);
  });

  it("goes negative for an instant before the week", () => {
    // 23:00 the previous Sunday, for an event that runs into Monday.
    expect(toWeekMinute(new Date(2026, 6, 19, 23, 0), MONDAY)).toBe(-60);
  });

  it("runs past the end for an instant after the week", () => {
    expect(toWeekMinute(new Date(2026, 6, 27, 1, 0), MONDAY)).toBe(MINUTES_PER_WEEK + 60);
  });

  it("does not care what time of day the week start carries", () => {
    const sloppy = new Date(2026, 6, 20, 17, 45);
    expect(toWeekMinute(new Date(2026, 6, 23, 9, 0), sloppy)).toBe(3 * MINUTES_PER_DAY + 540);
  });
});

describe("fromWeekMinute", () => {
  it("lands on the right day and time", () => {
    expect(fromWeekMinute(3 * MINUTES_PER_DAY + 540, MONDAY)).toEqual(new Date(2026, 6, 23, 9, 0));
  });

  it("handles the exclusive end of the week", () => {
    expect(fromWeekMinute(MINUTES_PER_WEEK, MONDAY)).toEqual(new Date(2026, 6, 27));
  });

  it("handles a minute before the week", () => {
    expect(fromWeekMinute(-60, MONDAY)).toEqual(new Date(2026, 6, 19, 23, 0));
  });

  it("round-trips every slot of the week", () => {
    for (let minute = 0; minute < MINUTES_PER_WEEK; minute += 15) {
      expect(toWeekMinute(fromWeekMinute(minute, MONDAY), MONDAY)).toBe(minute);
    }
  });
});

describe("snapMinutes", () => {
  it("rounds to the nearest quarter hour", () => {
    expect(snapMinutes(0)).toBe(0);
    expect(snapMinutes(7)).toBe(0);
    expect(snapMinutes(8)).toBe(15);
    expect(snapMinutes(521)).toBe(525);
  });
});

describe("atMinutes and addDays", () => {
  it("normalises minutes past the end of the day", () => {
    expect(atMinutes(MONDAY, MINUTES_PER_DAY)).toEqual(new Date(2026, 6, 21));
  });

  it("crosses a month boundary", () => {
    expect(addDays(new Date(2026, 6, 30), 3)).toEqual(new Date(2026, 7, 2));
  });
});
