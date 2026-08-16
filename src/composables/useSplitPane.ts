import { computed, onMounted, onUnmounted, ref, type Ref } from "vue";

import {
  DEFAULT_SIDE_WIDTH,
  MIN_SIDE_WIDTH,
  clampSideWidth,
  maxSideWidth,
  parseStoredWidth,
} from "../lib/split";

/**
 * `localStorage`, not `settings.toml`: this is a property of the window in
 * front of you, not of the vault or of the app's behaviour, and it changes on
 * every frame of a drag. Round-tripping that through a Tauri command would buy
 * nothing the webview's own store does not already give.
 */
const STORAGE_KEY = "calenpomo.sideWidth";

/** How far one arrow-key press moves the divider. */
const KEY_STEP_PX = 16;

interface Options {
  /** The flex row the two panes live in; the divider's travel is measured in it. */
  container: Ref<HTMLElement | null>;
}

function load(): number {
  try {
    return parseStoredWidth(window.localStorage.getItem(STORAGE_KEY)) ?? DEFAULT_SIDE_WIDTH;
  } catch {
    // Storage can be unavailable or throw outright. A divider that cannot
    // remember where it was is still a working divider.
    return DEFAULT_SIDE_WIDTH;
  }
}

function store(width: number): void {
  try {
    window.localStorage.setItem(STORAGE_KEY, String(width));
  } catch {
    /* See `load`. */
  }
}

/**
 * A draggable divider between the sidebar and the calendar.
 *
 * Must be called during `setup` — it registers lifecycle hooks for its observer
 * and its window listeners.
 */
export function useSplitPane({ container }: Options) {
  /**
   * What the user asked for, which is not always what gets rendered: a window
   * too narrow to honour it squeezes the sidebar, and widening the window again
   * must give back exactly what was squeezed. So the request is what persists,
   * and `width` is that request resolved against the room available.
   */
  const desired = ref(load());
  const containerWidth = ref(0);
  const dragging = ref(false);

  const width = computed(() => clampSideWidth(desired.value, containerWidth.value));
  const max = computed(() => maxSideWidth(containerWidth.value));

  /** Where the divider sat when the drag began, for `Escape`. */
  let origin: { x: number; width: number } | null = null;
  /** Held for the duration of the drag so the capture can be released. */
  let captured: { handle: HTMLElement; pointerId: number } | null = null;

  function set(next: number): void {
    desired.value = clampSideWidth(next, containerWidth.value);
  }

  function commit(): void {
    store(width.value);
  }

  function onPointerMove(event: PointerEvent): void {
    if (origin === null) {
      return;
    }

    // No button is down, so the release happened somewhere this never heard
    // about. Same reasoning as `useWeekDrag`: without it the divider would go
    // on following the pointer with nothing held down.
    if (event.buttons === 0) {
      onPointerUp();
      return;
    }

    // Follow the delta rather than the pointer's absolute position, so the
    // divider keeps whatever part of the handle was grabbed instead of
    // snapping its centre under the cursor.
    set(origin.width + (event.clientX - origin.x));
  }

  function onPointerUp(): void {
    if (origin === null) {
      return;
    }

    finish();
    commit();
  }

  function onKeyDown(event: KeyboardEvent): void {
    if (event.key === "Escape" && origin !== null) {
      const { width: before } = origin;
      finish();
      set(before);
    }
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
   * Take the drag over from the rest of the page.
   *
   * Both of these are for the whole window rather than the handle: the pointer
   * spends the drag over the calendar, where the text would otherwise select
   * under it and the cursor would flicker back to an arrow at every boundary.
   */
  function grabWindow(): void {
    document.body.style.userSelect = "none";
    document.body.style.cursor = "col-resize";
  }

  function releaseWindow(): void {
    document.body.style.userSelect = "";
    document.body.style.cursor = "";
  }

  function start(event: PointerEvent): void {
    // Left button only; a right-click on the handle is not a drag.
    if (event.button !== 0) {
      return;
    }

    event.preventDefault();
    const handle = event.currentTarget as HTMLElement;
    handle.setPointerCapture(event.pointerId);
    captured = { handle, pointerId: event.pointerId };
    origin = { x: event.clientX, width: width.value };
    dragging.value = true;
    grabWindow();
    listen();
  }

  function finish(): void {
    origin = null;
    dragging.value = false;
    // Capture is dropped implicitly on pointerup, but not when Escape ends the
    // drag with the button still down. Asking first because releasing a pointer
    // that is no longer captured throws.
    if (captured !== null && captured.handle.hasPointerCapture(captured.pointerId)) {
      captured.handle.releasePointerCapture(captured.pointerId);
    }
    captured = null;
    releaseWindow();
    unlisten();
  }

  /** Keyboard equivalent of the drag, for a focused handle. */
  function nudge(event: KeyboardEvent): void {
    const step =
      event.key === "ArrowLeft" ? -KEY_STEP_PX : event.key === "ArrowRight" ? KEY_STEP_PX : 0;
    if (step === 0) {
      return;
    }

    event.preventDefault();
    set(width.value + step);
    commit();
  }

  function reset(): void {
    set(DEFAULT_SIDE_WIDTH);
    commit();
  }

  const observer = new ResizeObserver((entries) => {
    const entry = entries[0];
    if (entry !== undefined) {
      containerWidth.value = entry.contentRect.width;
    }
  });

  onMounted(() => {
    if (container.value !== null) {
      // Fires once on observe, which is also how the first measurement arrives.
      observer.observe(container.value);
    }
  });

  onUnmounted(() => {
    observer.disconnect();
    // A drag in flight when the view goes away would otherwise leave the body
    // stuck with a resize cursor and no listeners to clear it.
    finish();
  });

  return { width, min: MIN_SIDE_WIDTH, max, dragging, start, nudge, reset };
}
