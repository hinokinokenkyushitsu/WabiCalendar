import { onUnmounted, ref, watch, type Ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { DAYS_PER_WEEK } from "../lib/week";
import { parseSession, type Session, type SessionWire } from "../types/session";

/** Mirrors `integrations::SESSION_EVENT`. */
const SESSION_EVENT = "sessions://recorded";

/**
 * The pomodoros that actually ran during the week `days` describes.
 *
 * Read-only by design: `sessions/` is an append-only log of things that already
 * happened, and this half of the week view introduces no way to change it. That
 * is also why there is nothing here matching `useCalendar`'s create/update/
 * remove — the timer writes these, the calendar never does.
 *
 * Must be called during `setup` — it registers an `onUnmounted` hook to drop
 * its event listener.
 */
export function useSessions(days: Ref<readonly Date[]>) {
  const sessions = ref<Session[]>([]);
  const error = ref<string | null>(null);

  async function reload(): Promise<void> {
    const boundaries = days.value;
    const from = boundaries[0];
    const to = boundaries[DAYS_PER_WEEK];
    if (from === undefined || to === undefined) {
      return;
    }

    try {
      const found = await invoke<SessionWire[]>("sessions_range", {
        from: from.toISOString(),
        to: to.toISOString(),
      });
      sessions.value = found.map(parseSession);
      error.value = null;
    } catch (e) {
      error.value = String(e);
    }
  }

  watch(days, () => void reload());

  // The backend emits this only once a record is on disk, so by the time we ask
  // for the range again it is really there. This is the whole of "the user does
  // not have to do anything": finish a pomodoro and it appears.
  const listening = listen(SESSION_EVENT, () => void reload());

  onUnmounted(() => {
    void listening.then((stop) => stop());
  });

  return { sessions, error, reload };
}
