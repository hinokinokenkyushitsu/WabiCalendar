/**
 * Mirrors `SessionView` and `Outcome` in `src-tauri/src/sessions.rs` and
 * `src-tauri/src/timer.rs`.
 * Maintained by hand — change both sides together.
 */

import type { Phase } from "./timer";

export type Outcome =
  /** Ran to its planned length. */
  | "completed"
  /** The user cut it short. */
  | "aborted"
  /** The machine slept through part of it, so the time does not count. */
  | "invalidated";

/** As it comes over the wire: timestamps are RFC 3339 strings with an offset. */
export interface SessionWire {
  id: string;
  kind: Phase;
  plannedSec: number;
  actualSec: number;
  startedAt: string;
  endedAt: string;
  outcome: Outcome;
  label: string | null;
}

/**
 * The same session with its timestamps parsed.
 *
 * They come back named `start` and `end` so that a session is a `TimeRange` and
 * can go through the same `sliceIntoDays` / `assignColumns` the planned blocks
 * do — the two lanes are laid out by one piece of code.
 */
export interface Session {
  id: string;
  kind: Phase;
  plannedSec: number;
  /**
   * What the timer actually counted. Not `end - start`: a segment that spent
   * ten minutes paused spans more wall time than it counted, and the block is
   * drawn across the span while the totals are added up from this.
   */
  actualSec: number;
  start: Date;
  end: Date;
  outcome: Outcome;
  label: string | null;
}

/**
 * Does this session count towards time actually spent focusing?
 *
 * Work only — a break is not focus — and never an invalidated one: the app
 * itself decided it could not vouch for that stretch, so counting it would be
 * the app contradicting its own warning. It is still drawn, struck through.
 */
export function isFocus(session: Session): boolean {
  return session.kind === "work" && session.outcome !== "invalidated";
}

export function parseSession(wire: SessionWire): Session {
  return {
    id: wire.id,
    kind: wire.kind,
    plannedSec: wire.plannedSec,
    actualSec: wire.actualSec,
    start: new Date(wire.startedAt),
    end: new Date(wire.endedAt),
    outcome: wire.outcome,
    label: wire.label,
  };
}
