/**
 * The countdown as a circle. Pure, and deliberately ignorant of SVG: everything
 * here is arithmetic on a `TimerState`.
 *
 * The ring never counts anything itself (invariant #2) — it is a projection of
 * whatever the backend last said. Between polls the *rendering* interpolates,
 * but every value it interpolates towards came from `timer_state`, so a dropped
 * poll parks the arc on the last true reading instead of drifting past it.
 */

import { clamp } from "./week";
import type { TimerState } from "../types/timer";

/** The `r` of the arc, in the viewBox units `TimerPanel` draws in. */
export const RING_RADIUS = 45;

/**
 * Dash length for a full circle.
 *
 * Derived rather than written out, because a hand-copied 282.74 and an `r="45"`
 * in the template are two numbers that can drift apart, and the failure is a
 * ring that never quite closes.
 */
export const RING_CIRCUMFERENCE = 2 * Math.PI * RING_RADIUS;

/**
 * How much of the circle is filled, from 0 to 1.
 *
 * Taken from `remainingSec`, not `elapsedSec`. The backend rounds remaining
 * *up* and truncates elapsed (`timer.rs`, `Timer::state`), so the two disagree
 * by up to a second — and remaining is the one the digits are showing. Reading
 * the same field the clock does is what keeps "25:00" paired with an empty ring
 * and "00:00" with a closed one.
 */
export function ringProgress(state: TimerState | null): number {
  if (state === null) {
    return 0;
  }

  // `Finished` is the one place the ring cannot follow the backend. The segment
  // ran its full length, but the hand-over has already happened: `phase` is the
  // next one and the count is back at zero, so the arithmetic below would say
  // "empty" at the exact moment the circle ought to read as closed. Nothing
  // else can show that a pomodoro completed once the digits have moved on, so
  // the ring holds it until the user starts the next segment.
  if (state.run === "finished") {
    return 1;
  }

  if (state.plannedSec <= 0) {
    return 0;
  }

  return clamp((state.plannedSec - state.remainingSec) / state.plannedSec, 0, 1);
}

/**
 * Whether the step from `prev` to `next` is the countdown advancing — worth
 * animating across — or a jump, which has to be cut.
 *
 * Only a segment ticking on under its own steam earns the interpolation.
 * Everything else (start, reset, the hand-over at the end, a sleep voiding the
 * count, editing the duration mid-run) moves the arc somewhere it did not
 * travel to, and sweeping a circle backwards over a second draws the timer
 * un-winding, which is not a thing that happened.
 */
export function ringAnimates(prev: TimerState | null, next: TimerState | null): boolean {
  if (prev === null || next === null) {
    return false;
  }

  if (prev.run !== "running" || next.run !== "running") {
    return false;
  }

  // A different segment, or the same one re-planned underneath us: in both
  // cases the two progress values are not measured against the same circle.
  if (prev.phase !== next.phase || prev.plannedSec !== next.plannedSec) {
    return false;
  }

  return ringProgress(next) >= ringProgress(prev);
}
