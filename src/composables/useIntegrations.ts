import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

import type { IntegrationReport } from "../types/integrations";

export function useIntegrations() {
  const report = ref<IntegrationReport | null>(null);
  const error = ref<string | null>(null);
  const busy = ref(false);

  async function attempt(op: () => Promise<IntegrationReport>): Promise<void> {
    busy.value = true;
    error.value = null;
    try {
      report.value = await op();
    } catch (e) {
      error.value = String(e);
    } finally {
      busy.value = false;
    }
  }

  function refresh(): Promise<void> {
    return attempt(() => invoke<IntegrationReport>("integration_status"));
  }

  function setAutostart(enabled: boolean): Promise<void> {
    return attempt(() => invoke<IntegrationReport>("set_autostart", { enabled }));
  }

  /** `null` turns the global shortcut off. */
  function setShortcut(accelerator: string | null): Promise<void> {
    return attempt(() => invoke<IntegrationReport>("set_shortcut", { accelerator }));
  }

  function testNotification(): Promise<void> {
    return attempt(() => invoke<IntegrationReport>("test_notification"));
  }

  /**
   * Durations live on the timer command, so set them there and re-read the
   * report rather than duplicating the state here.
   */
  function setDurations(workSec: number, breakSec: number): Promise<void> {
    return attempt(async () => {
      await invoke("timer_set_durations", { workSecs: workSec, breakSecs: breakSec });
      return invoke<IntegrationReport>("integration_status");
    });
  }

  return { report, error, busy, refresh, setAutostart, setShortcut, testNotification, setDurations };
}

/**
 * Turn a keypress into an accelerator the Rust side can parse.
 *
 * Built from `event.code` (physical position) rather than `event.key`, because
 * the underlying key table is position-based — `.key` would bind the wrong
 * physical key on a non-QWERTY layout.
 *
 * Returns `null` for anything unusable: a lone modifier, or a bare key with no
 * modifier at all. Registering a bare key globally would swallow it in every
 * other application on the machine.
 */
export function toAccelerator(event: KeyboardEvent): string | null {
  if (/^(Control|Shift|Alt|Meta)(Left|Right)$/.test(event.code)) {
    return null;
  }

  const modifiers: string[] = [];
  if (event.metaKey || event.ctrlKey) {
    modifiers.push("CommandOrControl");
  }
  if (event.altKey) {
    modifiers.push("Alt");
  }
  if (event.shiftKey) {
    modifiers.push("Shift");
  }

  if (modifiers.length === 0) {
    return null;
  }

  return [...modifiers, event.code].join("+");
}
