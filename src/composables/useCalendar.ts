import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";

import { DAYS_PER_WEEK, addDays, dayBoundaries, startOfWeek } from "../lib/week";
import {
  parseEvent,
  type CalEvent,
  type CalEventWire,
  type EventDraftWire,
} from "../types/calendar";

export interface Draft {
  summary: string;
  start: Date;
  end: Date;
}

function toWire(draft: Draft): EventDraftWire {
  return {
    summary: draft.summary,
    start: draft.start.toISOString(),
    end: draft.end.toISOString(),
  };
}

/**
 * The week's events, and the four operations that change them.
 *
 * The backend owns the files; this only ever holds what the last call returned.
 * A failed write puts the view back in step by reloading rather than leaving the
 * screen showing something the disk does not agree with.
 */
export function useCalendar() {
  const weekStart = ref(startOfWeek(new Date()));
  const events = ref<CalEvent[]>([]);
  const error = ref<string | null>(null);
  const loading = ref(false);

  /** The 8 local midnights bounding the visible week. */
  const days = computed(() => dayBoundaries(weekStart.value));

  type Attempt<T> = { ok: true; value: T } | { ok: false };

  async function guard<T>(op: () => Promise<T>): Promise<Attempt<T>> {
    try {
      const value = await op();
      error.value = null;
      return { ok: true, value };
    } catch (e) {
      error.value = String(e);
      return { ok: false };
    }
  }

  async function reload(): Promise<void> {
    loading.value = true;
    const boundaries = days.value;
    const found = await guard(() =>
      invoke<CalEventWire[]>("calendar_range", {
        from: boundaries[0].toISOString(),
        to: boundaries[DAYS_PER_WEEK].toISOString(),
      }),
    );
    loading.value = false;

    if (found.ok) {
      events.value = found.value.map(parseEvent);
    }
  }

  async function create(draft: Draft): Promise<CalEvent | null> {
    const made = await guard(() =>
      invoke<CalEventWire>("calendar_create", { draft: toWire(draft) }),
    );
    if (!made.ok) {
      await reload();
      return null;
    }

    const event = parseEvent(made.value);
    events.value = [...events.value, event];
    return event;
  }

  async function update(uid: string, draft: Draft): Promise<CalEvent | null> {
    const saved = await guard(() =>
      invoke<CalEventWire>("calendar_update", { uid, draft: toWire(draft) }),
    );
    if (!saved.ok) {
      await reload();
      return null;
    }

    const event = parseEvent(saved.value);
    events.value = events.value.map((existing) => (existing.uid === uid ? event : existing));
    return event;
  }

  async function remove(uid: string): Promise<void> {
    const done = await guard(() => invoke<void>("calendar_delete", { uid }));
    if (!done.ok) {
      await reload();
      return;
    }
    events.value = events.value.filter((event) => event.uid !== uid);
  }

  function goToWeek(offset: number): void {
    weekStart.value = addDays(weekStart.value, offset * DAYS_PER_WEEK);
  }

  function goToToday(): void {
    weekStart.value = startOfWeek(new Date());
  }

  watch(weekStart, () => void reload());

  return {
    weekStart,
    days,
    events,
    error,
    loading,
    reload,
    create,
    update,
    remove,
    goToWeek,
    goToToday,
  };
}
