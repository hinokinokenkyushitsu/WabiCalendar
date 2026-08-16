/**
 * Mirrors `TimerState`, `Phase`, `RunState` and `Transition` in
 * `src-tauri/src/timer.rs`.
 * Maintained by hand — change both sides together.
 */

export type Phase = "work" | "break";

export type RunState =
  | "idle"
  /** Counting down. */
  | "running"
  | "paused"
  /** The previous segment completed; `phase` is what is queued next. */
  | "finished"
  /** The machine slept through part of the segment, so it does not count. */
  | "invalidated";

export interface TimerState {
  phase: Phase;
  run: RunState;
  plannedSec: number;
  elapsedSec: number;
  remainingSec: number;
}

export type Transition =
  | { kind: "finished"; phase: Phase; next: Phase; plannedSec: number }
  | { kind: "invalidated"; phase: Phase; elapsedSec: number; sleptSec: number };
