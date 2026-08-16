import { computed, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import type { TimerState, Transition } from "../types/timer";

/** How often we ask the backend where the countdown has got to. */
const POLL_MS = 1000;

/** Mirrors `integrations::TRANSITION_EVENT`. */
const TRANSITION_EVENT = "timer://transition";

export function formatClock(totalSec: number): string {
  const safe = Math.max(0, Math.floor(totalSec));
  const minutes = Math.floor(safe / 60);
  const seconds = safe % 60;
  return `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
}

/**
 * Must be called during `setup` — it registers an `onUnmounted` hook to stop
 * polling and drop its event listener.
 */
export function useTimer() {
  const state = ref<TimerState | null>(null);
  const error = ref<string | null>(null);
  /** The most recent thing that happened, for the UI to announce. */
  const lastTransition = ref<Transition | null>(null);

  async function attempt(op: () => Promise<TimerState>): Promise<void> {
    try {
      state.value = await op();
      error.value = null;
    } catch (e) {
      error.value = String(e);
    }
  }

  /** Guards against a slow reply letting polls stack up on top of each other. */
  let inFlight = false;

  /**
   * Polling, never accumulating. The backend owns the count (invariant #2), so a
   * dropped tick, a throttled background window or a suspended machine all
   * self-correct on the next reply instead of drifting away from the truth.
   */
  async function poll(): Promise<void> {
    if (inFlight) {
      return;
    }
    inFlight = true;
    try {
      await attempt(() => invoke<TimerState>("timer_state"));
    } finally {
      inFlight = false;
    }
  }

  const ticker = setInterval(poll, POLL_MS);

  // A hidden window gets its timers throttled hard, so the first thing the user
  // sees on coming back would otherwise be a stale clock.
  function onVisible() {
    if (document.visibilityState === "visible") {
      void poll();
    }
  }
  document.addEventListener("visibilitychange", onVisible);

  // The backend also pushes, so the end of a segment lands immediately rather
  // than up to a second late.
  const listening = listen<Transition[]>(TRANSITION_EVENT, (event) => {
    const latest = event.payload[event.payload.length - 1];
    if (latest !== undefined) {
      lastTransition.value = latest;
    }
    void poll();
  });

  onUnmounted(() => {
    clearInterval(ticker);
    document.removeEventListener("visibilitychange", onVisible);
    void listening.then((stop) => stop());
  });

  function start(): Promise<void> {
    lastTransition.value = null;
    return attempt(() => invoke<TimerState>("timer_start"));
  }

  function toggle(): Promise<void> {
    lastTransition.value = null;
    return attempt(() => invoke<TimerState>("timer_toggle"));
  }

  function reset(): Promise<void> {
    lastTransition.value = null;
    return attempt(() => invoke<TimerState>("timer_reset"));
  }

  function setDurations(workSec: number, breakSec: number): Promise<void> {
    return attempt(() =>
      invoke<TimerState>("timer_set_durations", { workSecs: workSec, breakSecs: breakSec }),
    );
  }

  const display = computed(() => formatClock(state.value?.remainingSec ?? 0));

  return {
    state,
    error,
    lastTransition,
    display,
    poll,
    start,
    toggle,
    reset,
    setDurations,
  };
}
