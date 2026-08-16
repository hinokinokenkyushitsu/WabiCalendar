<script setup lang="ts">
import { computed, onMounted } from "vue";

import { useIntegrations } from "../composables/useIntegrations";

/**
 * Its own `useIntegrations` instance, and deliberately so: the composable makes
 * fresh refs per call, and the panel in the settings dialog only exists while
 * that dialog is open, so there is no longer-lived instance to share with.
 * Durations are the only field read here.
 */
const { report, error, busy, refresh, setDurations } = useIntegrations();

onMounted(refresh);

const workMin = computed(() => Math.round((report.value?.workSecs ?? 1500) / 60));
const breakMin = computed(() => Math.round((report.value?.breakSecs ?? 300) / 60));

function apply(work: number, brk: number) {
  void setDurations(Math.max(1, work) * 60, Math.max(1, brk) * 60);
}
</script>

<template>
  <section class="durations">
    <label>
      <span>Work</span>
      <input
        type="number"
        min="1"
        :value="workMin"
        :disabled="busy"
        @change="apply(Number(($event.target as HTMLInputElement).value), breakMin)"
      />
      <span class="unit">min</span>
    </label>
    <label>
      <span>Break</span>
      <input
        type="number"
        min="1"
        :value="breakMin"
        :disabled="busy"
        @change="apply(workMin, Number(($event.target as HTMLInputElement).value))"
      />
      <span class="unit">min</span>
    </label>

    <p v-if="error" role="alert" class="error">{{ error }}</p>
  </section>
</template>

<style scoped>
.durations {
  display: flex;
  gap: 1rem;
  flex-wrap: wrap;
}

label {
  display: flex;
  align-items: center;
  gap: 0.35rem;
  font-size: 0.85rem;
  opacity: 0.8;
}

.unit {
  opacity: 0.7;
}

input[type="number"] {
  width: 3.5rem;
  font: inherit;
  font-size: 0.85rem;
  padding: 0.2rem 0.35rem;
  border: 1px solid var(--border-strong);
  border-radius: 4px;
  background: var(--surface);
  color: inherit;
}

input[type="number"]:focus-visible {
  outline: 2px solid color-mix(in srgb, var(--accent) 55%, transparent);
  outline-offset: 1px;
}

.error {
  flex-basis: 100%;
  margin: 0;
  color: var(--danger);
  font-size: 0.85rem;
}
</style>
