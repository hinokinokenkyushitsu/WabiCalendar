<script setup lang="ts">
import { ref, watch } from "vue";

import SettingsPanel from "./SettingsPanel.vue";
import type { VaultController } from "../composables/useVault";

/**
 * The vault controller is handed down rather than built here: `useVault` makes
 * fresh refs per call, so a second instance would drift from the one the
 * calendar is gated on.
 */
const props = defineProps<{ open: boolean; vault: VaultController }>();
const emit = defineEmits<{ close: [] }>();

// The controller object itself never changes identity, so destructuring it once
// is safe — and it lets the template read the refs without `.value`, which
// props do not unwrap for nested objects.
const {
  status: vaultStatus,
  error: vaultError,
  busy: vaultBusy,
  choose: chooseVault,
} = props.vault;

const dialogRef = ref<HTMLDialogElement | null>(null);

/**
 * `showModal` rather than an overlay of our own: it brings the focus trap,
 * Esc-to-dismiss and the inert backdrop with it.
 */
watch(
  () => props.open,
  (open) => {
    const el = dialogRef.value;
    if (el === null) {
      return;
    }
    if (open && !el.open) {
      el.showModal();
    } else if (!open && el.open) {
      el.close();
    }
  },
);

/** A click that lands on the dialog element itself landed on the backdrop. */
function onClick(event: MouseEvent) {
  if (event.target === dialogRef.value) {
    emit("close");
  }
}
</script>

<template>
  <!-- The body is behind `v-if` so that closing really unmounts SettingsPanel:
       it registers a window-level keydown listener while capturing a shortcut,
       and its unmount hook is what guarantees that listener goes away. -->
  <dialog ref="dialogRef" class="sheet" @close="emit('close')" @click="onClick">
    <div v-if="open" class="sheet__body">
      <header class="sheet__bar">
        <h2>Settings</h2>
        <button class="sheet__close" aria-label="Close" @click="emit('close')">✕</button>
      </header>

      <section class="vault">
        <h3>Vault</h3>

        <p v-if="vaultBusy" class="muted">…</p>

        <template v-else-if="vaultStatus?.state === 'ready'">
          <code>{{ vaultStatus.path }}</code>
          <p v-if="vaultStatus.rebuilt.length" class="note">
            Rebuilt what was missing: {{ vaultStatus.rebuilt.join(", ") }}
          </p>
          <button @click="chooseVault">Change directory</button>
        </template>

        <template v-else-if="vaultStatus?.state === 'missing'">
          <p class="muted">Can't find the vault you used last:</p>
          <code>{{ vaultStatus.path }}</code>
          <p class="note">
            The directory may have been moved or renamed, or its disk isn't mounted.
          </p>
          <button @click="chooseVault">Choose again</button>
        </template>

        <template v-else-if="vaultStatus?.state === 'unconfigured'">
          <p class="muted">
            No vault chosen yet. Everything is stored as plain text in the directory you
            pick.
          </p>
          <button @click="chooseVault">Choose vault directory</button>
        </template>

        <p v-if="vaultError" role="alert" class="error">{{ vaultError }}</p>
      </section>

      <SettingsPanel />
    </div>
  </dialog>
</template>

<style scoped>
.sheet {
  width: min(34rem, calc(100vw - 3rem));
  max-height: min(40rem, calc(100vh - 4rem));
  padding: 0;
  border: 1px solid var(--border-strong);
  border-radius: 10px;
  background: var(--bg);
  color: var(--text);
  font-family: system-ui, -apple-system, sans-serif;
  line-height: 1.6;
  box-shadow: var(--shadow-drag);
}

.sheet::backdrop {
  background: rgb(0 0 0 / 0.35);
}

.sheet__body {
  padding: 1.25rem 1.5rem 1.5rem;
}

.sheet__bar {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 1rem;
  margin-bottom: 1.5rem;
}

h2 {
  font-size: 1.1rem;
  font-weight: 600;
  margin: 0;
}

h3 {
  font-size: 1rem;
  font-weight: 600;
  margin: 0 0 1rem;
}

.vault {
  margin-bottom: 2rem;
}

code {
  display: block;
  padding: 0.6rem 0.75rem;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--surface);
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 0.85rem;
  overflow-wrap: anywhere;
}

.muted {
  opacity: 0.7;
  font-size: 0.9rem;
}

.note {
  font-size: 0.85rem;
  opacity: 0.7;
  margin-top: 0.75rem;
}

.error {
  margin-top: 1rem;
  color: var(--danger);
  font-size: 0.85rem;
}

button {
  margin-top: 1rem;
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

.sheet__close {
  margin-top: 0;
  padding: 0.15rem 0.45rem;
  border-color: transparent;
  background: transparent;
  font-size: 0.9rem;
  opacity: 0.6;
}

.sheet__close:hover {
  opacity: 1;
}
</style>
