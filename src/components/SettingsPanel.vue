<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";

import { toAccelerator, useIntegrations } from "../composables/useIntegrations";
import type { FeatureStatus } from "../types/integrations";

// Durations live next to the timer in the sidebar, not here.
const { report, error, busy, refresh, setAutostart, setShortcut, testNotification } =
  useIntegrations();

onMounted(refresh);

/** True while we are swallowing keystrokes to capture a new shortcut. */
const capturing = ref(false);

function describe(status: FeatureStatus): string {
  switch (status.state) {
    case "ready":
      return "Available";
    case "off":
      return "Off";
    case "unsupported":
      return `Unsupported on this platform: ${status.reason}`;
    case "denied":
      return `Unavailable: ${status.reason}`;
  }
}

function statusClass(status: FeatureStatus): string {
  return status.state === "ready" ? "ok" : status.state === "off" ? "muted" : "warn";
}

function beginCapture() {
  capturing.value = true;
  window.addEventListener("keydown", onCaptureKey, { capture: true });
}

function endCapture() {
  capturing.value = false;
  window.removeEventListener("keydown", onCaptureKey, { capture: true });
}

function onCaptureKey(event: KeyboardEvent) {
  event.preventDefault();
  event.stopPropagation();

  if (event.code === "Escape") {
    endCapture();
    return;
  }

  const accelerator = toAccelerator(event);
  // A lone modifier, or a key with none — keep listening rather than binding
  // something that would misbehave system-wide.
  if (accelerator === null) {
    return;
  }

  endCapture();
  void setShortcut(accelerator);
}

// The listener is on `window`, so it has to go even if the panel disappears
// mid-capture.
onUnmounted(endCapture);

/** What this OS can actually draw next to the tray icon. */
const countdownNote = computed(() => {
  const c = report.value?.countdown;
  if (c === undefined) {
    return "";
  }
  const surfaces = [
    c.title ? "text beside the icon" : null,
    c.tooltip ? "hover tooltip" : null,
    c.menuItem ? "first menu item" : null,
  ].filter((s): s is string => s !== null);
  return `This platform can show the countdown in: ${surfaces.join(", ")}.`;
});
</script>

<template>
  <section class="settings">
    <h2>System integration</h2>

    <p v-if="report === null" class="muted">…</p>

    <template v-else>
      <div class="row">
        <div>
          <strong>Tray</strong>
          <p class="note" :class="statusClass(report.status.tray)">
            {{ describe(report.status.tray) }}
          </p>
          <p class="note muted">{{ countdownNote }}</p>
          <p v-if="report.status.tray.state !== 'ready'" class="note warn">
            No tray, so closing the window quits the app outright.
          </p>
        </div>
      </div>

      <div class="row">
        <div>
          <strong>Global shortcut</strong>
          <p class="note" :class="statusClass(report.status.shortcut)">
            {{ describe(report.status.shortcut) }}
          </p>
          <p class="note muted">
            Current: <code>{{ report.shortcutToggle ?? "not set" }}</code>
          </p>
        </div>
        <div class="controls">
          <button :disabled="busy" @click="capturing ? endCapture() : beginCapture()">
            {{ capturing ? "Press a combination… (Esc to cancel)" : "Rebind" }}
          </button>
          <button :disabled="busy || report.shortcutToggle === null" @click="setShortcut(null)">
            Clear
          </button>
        </div>
      </div>

      <div class="row">
        <div>
          <strong>Autostart</strong>
          <p class="note" :class="statusClass(report.status.autostart)">
            {{ describe(report.status.autostart) }}
          </p>
        </div>
        <div class="controls">
          <label>
            <input
              type="checkbox"
              :checked="report.autostart"
              :disabled="busy"
              @change="setAutostart(($event.target as HTMLInputElement).checked)"
            />
            Launch at login
          </label>
        </div>
      </div>

      <div class="row">
        <div>
          <strong>Notifications</strong>
          <p class="note" :class="statusClass(report.status.notification)">
            {{ describe(report.status.notification) }}
          </p>
          <p class="note muted">
            In dev mode macOS drops notifications silently; only a packaged build really
            shows them.
          </p>
        </div>
        <div class="controls">
          <button :disabled="busy" @click="testNotification">Send a test notification</button>
        </div>
      </div>
    </template>

    <p v-if="error" role="alert" class="error">{{ error }}</p>
  </section>
</template>

<style scoped>
h2 {
  font-size: 1rem;
  font-weight: 600;
  margin-bottom: 1rem;
}

.row {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
  gap: 1rem;
  padding: 0.75rem 0;
  border-top: 1px solid var(--border);
}

.controls {
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: 0.4rem;
  flex-shrink: 0;
}

.note {
  font-size: 0.85rem;
  margin-top: 0.25rem;
}

.muted {
  opacity: 0.65;
}

.ok {
  color: var(--ok);
}

.warn {
  color: var(--warn);
}

.error {
  margin-top: 1rem;
  color: var(--danger);
  font-size: 0.85rem;
}

code {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 0.8rem;
}

label {
  font-size: 0.85rem;
  display: flex;
  align-items: center;
  gap: 0.35rem;
}

button {
  padding: 0.35rem 0.7rem;
  font: inherit;
  font-size: 0.85rem;
  border: 1px solid var(--border-strong);
  border-radius: 6px;
  background: var(--surface);
  color: inherit;
  cursor: pointer;
  white-space: nowrap;
}

button:hover:not(:disabled) {
  background: color-mix(in srgb, var(--accent) 10%, var(--surface));
  border-color: color-mix(in srgb, var(--accent) 45%, var(--border-strong));
}

button:disabled {
  opacity: 0.5;
  cursor: default;
}
</style>
