/**
 * Where the sidebar ends and the calendar begins. Pure, and deliberately
 * ignorant of the DOM: everything here is arithmetic on pixel widths.
 *
 * A pixel width for the sidebar rather than a share of the window, because
 * nothing in the sidebar wants to grow. The timer and the duration fields are
 * fixed-size controls; the grid is the only thing that gets better with more
 * room, so every pixel the window gains belongs to it.
 */

import { clamp } from "./week";

/** What the sidebar was before it could be dragged: the old `flex: 0 0 20rem`. */
export const DEFAULT_SIDE_WIDTH = 320;

/** Under this the duration fields no longer sit beside their labels. */
export const MIN_SIDE_WIDTH = 240;

/**
 * Seven day columns plus the hour gutter. Narrower than this and a block is
 * too thin to aim at, which costs more than a cramped sidebar does.
 */
export const MIN_CALENDAR_WIDTH = 420;

/**
 * The widest the sidebar may be inside a container of `containerWidth`.
 *
 * When the window is too narrow to satisfy both minimums the sidebar wins, on
 * the grounds that a squeezed calendar still scrolls and still shows the week,
 * whereas a sidebar below its minimum has controls that overlap.
 */
export function maxSideWidth(containerWidth: number): number {
  return Math.max(MIN_SIDE_WIDTH, containerWidth - MIN_CALENDAR_WIDTH);
}

/**
 * The width to actually render, given what the user asked for and how much
 * room there is.
 *
 * `containerWidth` of 0 means "not measured yet" — the ref is null until the
 * layout is mounted. Only the floor is knowable then, and it matters that the
 * ceiling is *not* applied: pinning the sidebar to its minimum for the first
 * frame would be visible, and if that value were then stored it would be
 * remembered as a choice the user never made.
 */
export function clampSideWidth(desired: number, containerWidth: number): number {
  const wanted = Number.isFinite(desired) ? Math.round(desired) : DEFAULT_SIDE_WIDTH;

  if (!Number.isFinite(containerWidth) || containerWidth <= 0) {
    return Math.max(wanted, MIN_SIDE_WIDTH);
  }

  return clamp(wanted, MIN_SIDE_WIDTH, maxSideWidth(containerWidth));
}

/**
 * Read back what was stored, or `null` to mean "no usable preference".
 *
 * Anything can be in `localStorage` — a half-written value, something from a
 * future build, something a user typed into the devtools console. `null` sends
 * the caller to the default rather than letting `NaN` reach a style binding.
 */
export function parseStoredWidth(raw: string | null): number | null {
  if (raw === null) {
    return null;
  }

  const width = Number.parseFloat(raw);
  return Number.isFinite(width) && width > 0 ? Math.round(width) : null;
}
