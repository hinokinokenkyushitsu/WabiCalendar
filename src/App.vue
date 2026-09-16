<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, useTemplateRef } from "vue";

import DurationsPanel from "./components/DurationsPanel.vue";
import SettingsDialog from "./components/SettingsDialog.vue";
import TimerPanel from "./components/TimerPanel.vue";
import WeekCalendar from "./components/WeekCalendar.vue";
import { useSplitPane } from "./composables/useSplitPane";
import { useVault } from "./composables/useVault";

// Kept here, not in the dialog: the calendar is gated on this status, and
// `useVault` builds fresh refs per call, so there can only be one instance.
const vault = useVault();
const { status } = vault;

const settingsOpen = ref(false);

const rowRef = useTemplateRef<HTMLElement>("row");
const {
  width: sideWidth,
  min: minSideWidth,
  max: maxSideWidth,
  dragging: resizing,
  start: startResize,
  nudge: nudgeResize,
  reset: resetSideWidth,
} = useSplitPane({ container: rowRef });

const settingsHint = navigator.userAgent.includes("Mac") ? "⌘," : "Ctrl+,";

/**
 * The platform-standard way in, and nothing else claims it — Tauri's default
 * menu has no Preferences item, so the keystroke reaches the webview. Bubble
 * phase on purpose: SettingsPanel listens in the capture phase while recording
 * a global shortcut, so ⌘, still binds as a shortcut when that is what the user
 * is pressing it for.
 */
function onKeyDown(event: KeyboardEvent): void {
  if ((event.metaKey || event.ctrlKey) && event.key === "," && !event.repeat) {
    event.preventDefault();
    settingsOpen.value = !settingsOpen.value;
  }
}

onMounted(() => {
  window.addEventListener("keydown", onKeyDown);
  void vault.refresh();
});

onUnmounted(() => {
  window.removeEventListener("keydown", onKeyDown);
});

/** Why the calendar isn't here. The controls for fixing it live in settings. */
const placeholder = computed(() => {
  switch (status.value?.state) {
    case "missing":
      return `Can't find the vault you used last: ${status.value.path}`;
    case "unconfigured":
      return "No vault chosen yet. Everything is stored as plain text in a directory you pick.";
    default:
      return null;
  }
});
</script>

<template>
  <main ref="row">
    <!-- The timer stands on its own: it neither needs a vault nor waits for
         one, which is why it is outside everything the vault gates. -->
    <aside class="side" :style="{ width: `${sideWidth}px` }">
      <header class="side__bar">
        <h1>WabiCalendar</h1>
        <button
          class="gear"
          :title="`Settings (${settingsHint})`"
          aria-label="Settings"
          @click="settingsOpen = true"
        >
          <svg
            viewBox="0 0 24 24"
            width="18"
            height="18"
            fill="none"
            stroke="currentColor"
            stroke-width="1.7"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
          >
            <circle cx="12" cy="12" r="3.25" />
            <path
              d="M19.1 14.4a1.5 1.5 0 0 0 .3 1.65l.05.06a1.9 1.9 0 1 1-2.69 2.69l-.06-.06a1.5 1.5 0 0 0-1.65-.3 1.5 1.5 0 0 0-.91 1.37v.16a1.9 1.9 0 0 1-3.8 0v-.09a1.5 1.5 0 0 0-.98-1.37 1.5 1.5 0 0 0-1.65.3l-.06.06a1.9 1.9 0 1 1-2.69-2.69l.06-.06a1.5 1.5 0 0 0 .3-1.65 1.5 1.5 0 0 0-1.37-.91h-.16a1.9 1.9 0 0 1 0-3.8h.09a1.5 1.5 0 0 0 1.37-.98 1.5 1.5 0 0 0-.3-1.65l-.06-.06A1.9 1.9 0 1 1 7.62 4.4l.06.06a1.5 1.5 0 0 0 1.65.3h.07a1.5 1.5 0 0 0 .91-1.37v-.16a1.9 1.9 0 0 1 3.8 0v.09a1.5 1.5 0 0 0 .91 1.37 1.5 1.5 0 0 0 1.65-.3l.06-.06a1.9 1.9 0 1 1 2.69 2.69l-.06.06a1.5 1.5 0 0 0-.3 1.65v.07a1.5 1.5 0 0 0 1.37.91h.16a1.9 1.9 0 0 1 0 3.8h-.09a1.5 1.5 0 0 0-1.37.91z"
            />
          </svg>
        </button>
      </header>

      <TimerPanel />
      <DurationsPanel />
    </aside>

    <!-- Straddles the sidebar's border rather than replacing it: the line you
         see is still that border, this is only the part you can grab. -->
    <div
      class="split"
      :class="{ 'split--dragging': resizing }"
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize sidebar"
      :aria-valuenow="sideWidth"
      :aria-valuemin="minSideWidth"
      :aria-valuemax="maxSideWidth"
      tabindex="0"
      title="Drag to resize, double-click to reset"
      @pointerdown="startResize"
      @keydown="nudgeResize"
      @dblclick="resetSideWidth"
    />

    <section class="calendar">
      <!-- Keyed on the path so that picking a different vault starts the week
           view over rather than leaving the old vault's events on screen. -->
      <WeekCalendar v-if="status?.state === 'ready'" :key="status.path" />
      <div v-else-if="placeholder !== null" class="placeholder">
        <p class="muted">{{ placeholder }}</p>
        <button @click="settingsOpen = true">Open settings ({{ settingsHint }})</button>
      </div>
    </section>

    <SettingsDialog :open="settingsOpen" :vault="vault" @close="settingsOpen = false" />
  </main>
</template>

<style scoped>
main {
  display: flex;
  align-items: stretch;
  height: 100vh;
  font-family: system-ui, -apple-system, sans-serif;
  line-height: 1.6;
}

/* Width comes from `useSplitPane`; `flex` only has to stop it growing. */
.side {
  flex: 0 0 auto;
  box-sizing: border-box;
  padding: 2rem 1.5rem;
  overflow-y: auto;
  /* Sits a shade above the page so the sidebar reads as its own surface
     without needing a heavier rule to say so. */
  background: color-mix(in srgb, var(--surface) 55%, var(--bg));
  border-right: 1px solid var(--border);
}

.calendar {
  flex: 1 1 auto;
  min-width: 0;
  box-sizing: border-box;
  padding: 1.25rem 1.5rem;
  display: flex;
  flex-direction: column;
}

/*
 * A target you can hit sitting on a line you can barely see. The negative
 * margins give back exactly the width it occupies, so the two panes still meet
 * on the sidebar's own 1px border and nothing shifts by adding this.
 *
 * Not `--accent`: that colour means "now" and nothing else.
 */
.split {
  position: relative;
  z-index: 1;
  flex: 0 0 auto;
  width: 9px;
  margin: 0 -4.5px;
  cursor: col-resize;
  /* Without this the webview takes the drag as a pan and never delivers it. */
  touch-action: none;
}

.split::after {
  content: "";
  position: absolute;
  inset: 0 3px;
  background: transparent;
  transition: background 120ms ease;
}

.split:hover::after,
.split--dragging::after {
  background: var(--border-strong);
}

.split:focus-visible {
  outline: 2px solid var(--border-strong);
  outline-offset: -2px;
}

.placeholder {
  margin: auto;
  max-width: 24rem;
  text-align: center;
}

.side__bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 1rem;
  margin-bottom: 2rem;
}

h1 {
  font-size: 1.25rem;
  font-weight: 600;
  margin: 0;
}

.muted {
  opacity: 0.7;
  font-size: 0.9rem;
}

button {
  margin-top: 1.25rem;
  padding: 0.45rem 0.9rem;
  font: inherit;
  font-size: 0.9rem;
  border: 1px solid var(--border-strong);
  border-radius: 6px;
  background: var(--surface);
  color: inherit;
  cursor: pointer;
}

button:hover {
  background: color-mix(in srgb, var(--accent) 10%, var(--surface));
  border-color: color-mix(in srgb, var(--accent) 45%, var(--border-strong));
}

/* Icon-only, so it carries no border until you reach for it. */
.gear {
  display: flex;
  margin-top: 0;
  padding: 0.35rem;
  border-color: transparent;
  background: transparent;
  opacity: 0.65;
}

.gear:hover {
  opacity: 1;
}
</style>
