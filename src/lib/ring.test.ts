import { describe, expect, it } from "vitest";

import { RING_CIRCUMFERENCE, RING_RADIUS, ringAnimates, ringProgress } from "./ring";
import type { TimerState } from "../types/timer";

const WORK = 1500;

function state(over: Partial<TimerState> = {}): TimerState {
  return {
    phase: "work",
    run: "running",
    plannedSec: WORK,
    elapsedSec: 0,
    remainingSec: WORK,
    ...over,
  };
}

describe("ringProgress", () => {
  it("is empty before anything has been started", () => {
    expect(ringProgress(state({ run: "idle" }))).toBe(0);
  });

  it("is empty while there is no state at all", () => {
    expect(ringProgress(null)).toBe(0);
  });

  it("tracks the segment", () => {
    expect(ringProgress(state({ remainingSec: 1125 }))).toBeCloseTo(0.25);
    expect(ringProgress(state({ remainingSec: 750 }))).toBeCloseTo(0.5);
    expect(ringProgress(state({ remainingSec: 1 }))).toBeCloseTo(1 - 1 / WORK);
  });

  /**
   * The backend rounds remaining up and truncates elapsed, so the two fields
   * disagree for most of every second. The digits read remaining, and the ring
   * has to agree with the digits or the two halves of the same clock contradict
   * each other.
   */
  it("agrees with the digits rather than with elapsed", () => {
    const firstSecond = state({ remainingSec: WORK, elapsedSec: 0 });
    expect(ringProgress(firstSecond)).toBe(0);

    const halfWay = state({ remainingSec: 750, elapsedSec: 749 });
    expect(ringProgress(halfWay)).toBeCloseTo(0.5);
  });

  /**
   * The segment ran its length, but the backend has already handed over: phase
   * is the break, the count is back at zero and the digits show 05:00. The
   * closed circle is the only thing left saying the pomodoro finished.
   */
  it("holds a closed circle after a segment finishes", () => {
    const handedOver = state({
      run: "finished",
      phase: "break",
      plannedSec: 300,
      elapsedSec: 0,
      remainingSec: 300,
    });

    expect(ringProgress(handedOver)).toBe(1);
  });

  /** Nothing was banked, so there is nothing to draw. */
  it("empties when a segment is voided by a sleep", () => {
    expect(ringProgress(state({ run: "invalidated", remainingSec: WORK }))).toBe(0);
  });

  it("stays paused where it stood", () => {
    expect(ringProgress(state({ run: "paused", remainingSec: 600 }))).toBeCloseTo(0.6);
  });

  /** A zero-length segment would otherwise put NaN into `stroke-dashoffset`. */
  it("does not divide by a zero-length segment", () => {
    expect(ringProgress(state({ plannedSec: 0, remainingSec: 0 }))).toBe(0);
  });

  it("clamps rather than trusting the two fields to be consistent", () => {
    expect(ringProgress(state({ remainingSec: WORK + 60 }))).toBe(0);
    expect(ringProgress(state({ remainingSec: -60 }))).toBe(1);
  });
});

describe("ringAnimates", () => {
  it("interpolates across an ordinary tick", () => {
    expect(ringAnimates(state({ remainingSec: 900 }), state({ remainingSec: 899 }))).toBe(true);
  });

  it("cuts on the first frame, with nothing to come from", () => {
    expect(ringAnimates(null, state({ remainingSec: 900 }))).toBe(false);
  });

  it("cuts when the timer stops rather than easing to a halt", () => {
    const running = state({ remainingSec: 900 });

    expect(ringAnimates(running, state({ run: "paused", remainingSec: 900 }))).toBe(false);
    expect(ringAnimates(running, state({ run: "idle", remainingSec: WORK }))).toBe(false);
    expect(ringAnimates(running, state({ run: "invalidated", remainingSec: WORK }))).toBe(false);
  });

  /** Resuming lands on the banked value; there is no gap to sweep across. */
  it("cuts on the way back out of a pause", () => {
    const paused = state({ run: "paused", remainingSec: 900 });

    expect(ringAnimates(paused, state({ remainingSec: 900 }))).toBe(false);
  });

  it("cuts at the hand-over between segments", () => {
    const nearlyDone = state({ remainingSec: 1 });
    const finished = state({ run: "finished", phase: "break", plannedSec: 300, remainingSec: 300 });

    expect(ringAnimates(nearlyDone, finished)).toBe(false);
  });

  /** The two readings are shares of different circles, so the arc has to jump. */
  it("cuts when the duration is edited mid-segment", () => {
    const before = state({ remainingSec: 900 });
    const after = state({ plannedSec: 3000, remainingSec: 2400 });

    expect(ringAnimates(before, after)).toBe(false);
  });

  it("cuts when the phase changes underneath the same run state", () => {
    const work = state({ remainingSec: 900 });
    const brk = state({ phase: "break", remainingSec: 900 });

    expect(ringAnimates(work, brk)).toBe(false);
  });

  /** Starting over mid-segment: the arc belongs back at zero immediately. */
  it("cuts when progress goes backwards", () => {
    const partway = state({ remainingSec: 900 });
    const restarted = state({ remainingSec: WORK });

    expect(ringAnimates(partway, restarted)).toBe(false);
  });
});

describe("ring geometry", () => {
  /** The template's `r` and this dash length have to describe the same circle. */
  it("derives the dash length from the radius", () => {
    expect(RING_CIRCUMFERENCE).toBeCloseTo(2 * Math.PI * RING_RADIUS);
    expect(RING_RADIUS).toBe(45);
  });
});
