import { describe, expect, it } from "vitest";

import {
  DEFAULT_SIDE_WIDTH,
  MIN_CALENDAR_WIDTH,
  MIN_SIDE_WIDTH,
  clampSideWidth,
  maxSideWidth,
  parseStoredWidth,
} from "./split";

/** Wide enough that neither minimum is in play. */
const ROOMY = 1440;

describe("clampSideWidth", () => {
  it("leaves a width that fits alone", () => {
    expect(clampSideWidth(400, ROOMY)).toBe(400);
  });

  it("holds the sidebar at its minimum", () => {
    expect(clampSideWidth(80, ROOMY)).toBe(MIN_SIDE_WIDTH);
  });

  it("stops the sidebar before the calendar loses its minimum", () => {
    expect(clampSideWidth(ROOMY, ROOMY)).toBe(ROOMY - MIN_CALENDAR_WIDTH);
  });

  it("keeps the sidebar usable when the window cannot hold both minimums", () => {
    const cramped = MIN_SIDE_WIDTH + MIN_CALENDAR_WIDTH - 200;

    expect(clampSideWidth(DEFAULT_SIDE_WIDTH, cramped)).toBe(MIN_SIDE_WIDTH);
  });

  it("rounds to whole pixels", () => {
    expect(clampSideWidth(320.4, ROOMY)).toBe(320);
  });

  /** The container ref is null until mount, which reads as a width of 0. */
  it("applies only the floor while the container is unmeasured", () => {
    expect(clampSideWidth(900, 0)).toBe(900);
    expect(clampSideWidth(10, 0)).toBe(MIN_SIDE_WIDTH);
  });

  it("falls back to the default rather than passing NaN on", () => {
    expect(clampSideWidth(Number.NaN, ROOMY)).toBe(DEFAULT_SIDE_WIDTH);
  });
});

describe("maxSideWidth", () => {
  it("gives the calendar its minimum first", () => {
    expect(maxSideWidth(1000)).toBe(1000 - MIN_CALENDAR_WIDTH);
  });

  it("never drops below the sidebar's own minimum", () => {
    expect(maxSideWidth(300)).toBe(MIN_SIDE_WIDTH);
  });
});

describe("parseStoredWidth", () => {
  it("reads back a stored width", () => {
    expect(parseStoredWidth("380")).toBe(380);
  });

  it("has no opinion when nothing was stored", () => {
    expect(parseStoredWidth(null)).toBeNull();
  });

  it("rejects values that are not a usable width", () => {
    expect(parseStoredWidth("")).toBeNull();
    expect(parseStoredWidth("wide")).toBeNull();
    expect(parseStoredWidth("0")).toBeNull();
    expect(parseStoredWidth("-320")).toBeNull();
    expect(parseStoredWidth("Infinity")).toBeNull();
  });
});
