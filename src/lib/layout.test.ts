import { describe, expect, it } from "vitest";

import { assignColumns, sliceIntoDays, type Span } from "./layout";

/** `9.5` -> 570 minutes, so the cases below read like a timetable. */
function at(hour: number): number {
  return Math.round(hour * 60);
}

function span(startHour: number, endHour: number): Span {
  return { start: at(startHour), end: at(endHour) };
}

describe("assignColumns", () => {
  it("gives a lone block the whole width", () => {
    expect(assignColumns([span(9, 10)])).toEqual([{ column: 0, columns: 1 }]);
  });

  it("returns nothing for nothing", () => {
    expect(assignColumns([])).toEqual([]);
  });

  it("puts blocks that do not overlap all in the first column", () => {
    const placements = assignColumns([span(9, 10), span(11, 12), span(14, 15)]);

    expect(placements).toEqual([
      { column: 0, columns: 1 },
      { column: 0, columns: 1 },
      { column: 0, columns: 1 },
    ]);
  });

  /** Half-open: 10:00–11:00 does not overlap 09:00–10:00. */
  it("treats a block starting exactly when another ends as not overlapping", () => {
    const placements = assignColumns([span(9, 10), span(10, 11)]);

    expect(placements).toEqual([
      { column: 0, columns: 1 },
      { column: 0, columns: 1 },
    ]);
  });

  it("splits the width evenly between blocks that all overlap each other", () => {
    // 09:00–10:00, 09:30–10:30, 09:45–10:15 — every pair shares some time.
    const placements = assignColumns([span(9, 10), span(9.5, 10.5), span(9.75, 10.25)]);

    expect(placements).toEqual([
      { column: 0, columns: 3 },
      { column: 1, columns: 3 },
      { column: 2, columns: 3 },
    ]);
  });

  /**
   * A overlaps B and B overlaps C, but A and C are disjoint. Only two blocks
   * are ever live at once, so two columns are enough — and C is free to reuse
   * the column A has finished with.
   */
  it("needs only two columns for a chain of overlaps", () => {
    const a = span(9, 10.5);
    const b = span(10, 11.5);
    const c = span(11, 12);

    const placements = assignColumns([a, b, c]);

    expect(placements).toEqual([
      { column: 0, columns: 2 },
      { column: 1, columns: 2 },
      { column: 0, columns: 2 },
    ]);
  });

  it("keeps the whole chain to one shared width even when it is long", () => {
    // Five blocks, each overlapping only its neighbours: still two columns.
    const chain = [span(9, 10), span(9.5, 10.5), span(10, 11), span(10.5, 11.5), span(11, 12)];

    const placements = assignColumns(chain);

    expect(placements.map((p) => p.columns)).toEqual([2, 2, 2, 2, 2]);
    expect(placements.map((p) => p.column)).toEqual([0, 1, 0, 1, 0]);
  });

  it("puts a contained block beside the one containing it", () => {
    const placements = assignColumns([span(9, 17), span(10, 11)]);

    expect(placements).toEqual([
      { column: 0, columns: 2 },
      { column: 1, columns: 2 },
    ]);
  });

  it("keeps the containing block on the first column whichever order it arrives in", () => {
    const placements = assignColumns([span(10, 11), span(9, 17)]);

    // The long one still leads, so the all-day-ish block does not get shunted
    // into the second column by a short block that happens to be listed first.
    expect(placements).toEqual([
      { column: 1, columns: 2 },
      { column: 0, columns: 2 },
    ]);
  });

  it("widens only the cluster that needs it", () => {
    // A three-way pile-up in the morning, one quiet block in the afternoon.
    const placements = assignColumns([
      span(9, 11),
      span(9.5, 11.5),
      span(10, 12),
      span(15, 16),
    ]);

    expect(placements.map((p) => p.columns)).toEqual([3, 3, 3, 1]);
  });

  it("answers positionally, whatever order the input came in", () => {
    const shuffled = [span(11, 12), span(9, 10.5), span(10, 11.5)];

    const placements = assignColumns(shuffled);

    // Same three blocks as the chain case, listed back to front.
    expect(placements).toEqual([
      { column: 0, columns: 2 },
      { column: 0, columns: 2 },
      { column: 1, columns: 2 },
    ]);
  });

  it("survives a zero-length block without looping or crashing", () => {
    const placements = assignColumns([span(9, 9), span(9, 10)]);

    expect(placements).toHaveLength(2);
    expect(placements.every((p) => p.column < p.columns)).toBe(true);
  });
});

describe("sliceIntoDays", () => {
  /** The 8 local midnights bounding the week of Monday 20 July 2026. */
  const week = Array.from({ length: 8 }, (_, i) => new Date(2026, 6, 20 + i));

  it("leaves a block inside one day alone", () => {
    const event = { start: new Date(2026, 6, 22, 14, 0), end: new Date(2026, 6, 22, 15, 30) };

    expect(sliceIntoDays([event], week)).toEqual([
      {
        item: event,
        day: 2,
        start: 14 * 60,
        end: 15 * 60 + 30,
        continuesBefore: false,
        continuesAfter: false,
      },
    ]);
  });

  it("cuts a block running through midnight into one segment per day", () => {
    const event = { start: new Date(2026, 6, 22, 23, 0), end: new Date(2026, 6, 23, 1, 0) };

    const segments = sliceIntoDays([event], week);

    expect(segments).toEqual([
      {
        item: event,
        day: 2,
        start: 23 * 60,
        end: 24 * 60,
        continuesBefore: false,
        continuesAfter: true,
      },
      {
        item: event,
        day: 3,
        start: 0,
        end: 60,
        continuesBefore: true,
        continuesAfter: false,
      },
    ]);
  });

  it("does not give the next day an empty sliver", () => {
    const event = { start: new Date(2026, 6, 22, 23, 0), end: new Date(2026, 6, 23) };

    const segments = sliceIntoDays([event], week);

    expect(segments).toHaveLength(1);
    expect(segments[0].day).toBe(2);
    expect(segments[0].end).toBe(24 * 60);
  });

  it("clips a block that starts before the week and ends after it", () => {
    const event = { start: new Date(2026, 6, 18), end: new Date(2026, 6, 30) };

    const segments = sliceIntoDays([event], week);

    expect(segments).toHaveLength(7);
    expect(segments.every((s) => s.start === 0 && s.end === 24 * 60)).toBe(true);
    expect(segments[0].continuesBefore).toBe(true);
    expect(segments[6].continuesAfter).toBe(true);
  });

  it("ignores a block that falls outside the week entirely", () => {
    const event = { start: new Date(2026, 7, 3, 9, 0), end: new Date(2026, 7, 3, 10, 0) };

    expect(sliceIntoDays([event], week)).toEqual([]);
  });
});
