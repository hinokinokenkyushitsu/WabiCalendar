import { onUnmounted, ref, type Ref } from "vue";

import {
  DAYS_PER_WEEK,
  MINUTES_PER_DAY,
  MINUTES_PER_WEEK,
  SLOT_MINUTES,
  clamp,
  snapMinutes,
} from "../lib/week";

/**
 * Positions are held as minutes since the week's first local midnight, which is
 * the grid's own coordinate system: `day * 1440 + minutes into that day`. It is
 * a wall-clock measure, not elapsed time, so it stays aligned with the hour
 * labels through a daylight-saving change. Converting back to a `Date` goes
 * through `atMinutes`, which resolves it against the real calendar.
 */
export type WeekMinute = number;

export type DragKind = "create" | "move" | "resizeStart" | "resizeEnd";

/**
 * How far the pointer must travel down or up the grid before a press on empty
 * space starts drawing a block, in pixels.
 *
 * Below this the press is a click, and a click creates nothing: the grid is a
 * very large target, so a press that leaves a block behind every time it lands
 * somewhere by accident is a block to be deleted every time it lands somewhere
 * by accident.
 *
 * Pixels rather than "has the pointer reached a different 15-minute slot",
 * which sounds like the more natural rule for a grid and is not: a slot is only
 * 12px tall at the default zoom, so pressing near a slot boundary would satisfy
 * that rule at zero movement, and whether a click created anything would depend
 * on where in the slot it landed.
 */
const CREATE_THRESHOLD_PX = 4;

export interface DragTarget {
  /** The block being dragged; `null` while dragging out a brand new one. */
  id: string | null;
  uid: string | null;
  start: WeekMinute;
  end: WeekMinute;
}

export interface DragState extends DragTarget {
  kind: DragKind;
}

interface Options {
  /** The element the 7 day columns are laid out in. */
  grid: Ref<HTMLElement | null>;
  /** Called on release, only when the block actually moved. */
  commit: (state: DragState) => void;
}

/**
 * Drag to create, move and resize blocks on the week grid.
 *
 * Must be called during `setup` — it registers an `onUnmounted` hook to drop its
 * window listeners.
 */
export function useWeekDrag({ grid, commit }: Options) {
  const drag = ref<DragState | null>(null);

  /** Where the block sat when the drag began, to tell a click from a move. */
  let initial: { start: WeekMinute; end: WeekMinute } | null = null;
  /** For `move`: how far into the block the pointer grabbed it. */
  let grabOffset = 0;
  /** For `create`: the edge that stays put. */
  let anchor = 0;
  /**
   * A press on empty space that has not yet travelled far enough to become a
   * drag. Nothing is drawn and nothing will be written while this is what the
   * gesture is; releasing here is a click.
   */
  let pending: { anchor: WeekMinute; y: number } | null = null;

  /** Pointer position as a week minute, unsnapped. */
  function pointerAt(event: PointerEvent): { day: number; minute: number } | null {
    const element = grid.value;
    if (element === null) {
      return null;
    }

    // Viewport-relative, so this already accounts for however far the grid has
    // been scrolled.
    const rect = element.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) {
      return null;
    }

    const day = clamp(
      Math.floor(((event.clientX - rect.left) / rect.width) * DAYS_PER_WEEK),
      0,
      DAYS_PER_WEEK - 1,
    );
    const minute = clamp(
      ((event.clientY - rect.top) / rect.height) * MINUTES_PER_DAY,
      0,
      MINUTES_PER_DAY,
    );

    return { day, minute };
  }

  function weekMinute(event: PointerEvent): WeekMinute | null {
    const at = pointerAt(event);
    return at === null ? null : at.day * MINUTES_PER_DAY + at.minute;
  }

  function listen(): void {
    window.addEventListener("pointermove", onPointerMove);
    window.addEventListener("pointerup", onPointerUp);
    window.addEventListener("pointercancel", finish);
    window.addEventListener("keydown", onKeyDown);
  }

  function unlisten(): void {
    window.removeEventListener("pointermove", onPointerMove);
    window.removeEventListener("pointerup", onPointerUp);
    window.removeEventListener("pointercancel", finish);
    window.removeEventListener("keydown", onKeyDown);
  }

  /**
   * Arm a create drag from empty space.
   *
   * Nothing appears yet: only dragging creates a block, so the press is held as
   * `pending` until it has moved far enough to mean it.
   */
  function startCreate(event: PointerEvent): void {
    const at = pointerAt(event);
    if (at === null) {
      return;
    }

    // Floor rather than round, so pressing at 09:07 starts the block at 09:00
    // instead of jumping backwards to 09:15's neighbour.
    const start =
      at.day * MINUTES_PER_DAY + Math.floor(at.minute / SLOT_MINUTES) * SLOT_MINUTES;
    initial = null;
    drag.value = null;
    pending = { anchor: start, y: event.clientY };
    listen();
  }

  /** Move or resize a block that is already there. */
  function startEdit(event: PointerEvent, target: DragTarget, kind: DragKind): void {
    const at = weekMinute(event);
    if (at === null) {
      return;
    }

    grabOffset = at - target.start;
    initial = { start: target.start, end: target.end };
    drag.value = { ...target, kind };
    listen();
  }

  function onPointerMove(event: PointerEvent): void {
    // No button is down any more, so the release happened somewhere we never
    // heard about -- outside the window is the usual way. Without this the block
    // would go on following the pointer with nothing held down, and the next
    // click anywhere would drop it there.
    if (event.buttons === 0) {
      onPointerUp();
      return;
    }

    if (pending !== null) {
      // Vertical only: a create drag stays in the day it began in, so sweeping
      // sideways is not movement this gesture can express, and it should not be
      // what conjures a block out of a stray click either.
      if (Math.abs(event.clientY - pending.y) < CREATE_THRESHOLD_PX) {
        return;
      }

      anchor = pending.anchor;
      pending = null;
      drag.value = {
        kind: "create",
        id: null,
        uid: null,
        start: anchor,
        end: anchor + SLOT_MINUTES,
      };
    }

    const state = drag.value;
    if (state === null) {
      return;
    }

    const at = weekMinute(event);
    if (at === null) {
      return;
    }
    const snapped = snapMinutes(at);

    switch (state.kind) {
      case "create": {
        // Creation stays in the day it began in: dragging sideways to sweep a
        // block across days is not a gesture this grid offers.
        const day = Math.floor(anchor / MINUTES_PER_DAY);
        const low = day * MINUTES_PER_DAY;
        const edge = clamp(snapped, low, low + MINUTES_PER_DAY);
        const start = Math.min(anchor, edge);
        const end = Math.max(anchor, edge);
        drag.value = {
          ...state,
          start,
          // Dragging back onto the anchor would leave nothing to see.
          end: end - start < SLOT_MINUTES ? start + SLOT_MINUTES : end,
        };
        break;
      }
      case "move": {
        const length = state.end - state.start;
        const start = clamp(snapMinutes(at - grabOffset), 0, MINUTES_PER_WEEK - length);
        drag.value = { ...state, start, end: start + length };
        break;
      }
      case "resizeStart": {
        const start = clamp(snapped, 0, state.end - SLOT_MINUTES);
        drag.value = { ...state, start };
        break;
      }
      case "resizeEnd": {
        const end = clamp(snapped, state.start + SLOT_MINUTES, MINUTES_PER_WEEK);
        drag.value = { ...state, end };
        break;
      }
    }
  }

  function onPointerUp(): void {
    const state = drag.value;
    finish();
    // Either a press on empty space that never became a drag, or nothing at all.
    // Either way there is no block to write.
    if (state === null) {
      return;
    }

    // A press that never moved is a click, and a click is not an edit.
    const moved =
      initial === null || state.start !== initial.start || state.end !== initial.end;
    if (moved) {
      commit(state);
    }
  }

  function onKeyDown(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      finish();
    }
  }

  function finish(): void {
    drag.value = null;
    initial = null;
    pending = null;
    unlisten();
  }

  onUnmounted(unlisten);

  return { drag, startCreate, startEdit, cancel: finish };
}
