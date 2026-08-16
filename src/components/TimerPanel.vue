<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";

import { useTimer } from "../composables/useTimer";
import { formatClock } from "../composables/useTimer";
import { RING_CIRCUMFERENCE, ringAnimates, ringProgress } from "../lib/ring";

const { state, error, lastTransition, display, poll, toggle, reset } = useTimer();

onMounted(poll);

const phaseLabel = computed(() => (state.value?.phase === "break" ? "Break" : "Work"));

/** The segment the machine slept through. Struck out rather than hidden. */
const stale = computed(() => state.value?.run === "invalidated");

const toggleLabel = computed(() => {
  switch (state.value?.run) {
    case "running":
      return "Pause";
    case "paused":
      return "Resume";
    default:
      return "Start";
  }
});

/**
 * Where the arc stops. Offset counts backwards from a full circle, so a
 * progress of 0 leaves the whole circumference dashed away.
 */
const dashOffset = computed(() => RING_CIRCUMFERENCE * (1 - ringProgress(state.value)));

/**
 * Whether this step is worth interpolating across.
 *
 * The backend is polled once a second (invariant #2), which would leave the
 * arc stepping rather than travelling. Rather than count in the frontend, each
 * reply sets a new target and the CSS transition covers the second in between,
 * so every point the arc passes through lies between two readings the backend
 * gave us. Jumps — reset, the hand-over at the end, a voided segment — turn it
 * off, since easing into those draws the ring un-winding.
 */
const animating = ref(false);
watch(state, (next, prev) => {
  animating.value = ringAnimates(prev, next);
});

/** What just happened, in one line. Cleared by the next action. */
const announcement = computed(() => {
  const t = lastTransition.value;
  if (t === null) {
    return null;
  }
  if (t.kind === "finished") {
    // Mirrors `Phase::auto_starts` in `timer.rs`: the break is already counting
    // by the time this line appears, whereas work waits to be started.
    return t.next === "break"
      ? "Work session finished — the break has already started."
      : "Break finished — start the next pomodoro when you are ready.";
  }
  return `This ${t.phase === "work" ? "work session" : "break"} was voided: the machine slept for ${formatClock(
    t.sleptSec,
  )}, so the count was broken.`;
});
</script>

<template>
  <section class="timer">
    <p class="muted">{{ phaseLabel }}</p>

    <!-- The digits sit in the ring's own grid cell rather than under it, so the
         two are one dial and the clock keeps its own markup. -->
    <div class="ring">
      <svg
        class="ring__dial"
        :class="{ 'ring__dial--stale': stale }"
        viewBox="0 0 100 100"
        aria-hidden="true"
      >
        <circle class="ring__track" cx="50" cy="50" r="45" />
        <!-- Rotated by attribute, not by CSS: `transform-box` defaults differ
             between engines, and this needs no origin to be agreed on. -->
        <circle
          class="ring__arc"
          :class="{ 'ring__arc--cut': !animating }"
          cx="50"
          cy="50"
          r="45"
          transform="rotate(-90 50 50)"
          :stroke-dasharray="RING_CIRCUMFERENCE"
          :stroke-dashoffset="dashOffset"
        />
      </svg>

      <p class="clock" :class="{ stale }">{{ display }}</p>
    </div>

    <p v-if="announcement" class="note" role="status">{{ announcement }}</p>

    <div class="actions">
      <button @click="toggle">{{ toggleLabel }}</button>
      <button @click="reset">Reset</button>
    </div>

    <p v-if="error" role="alert" class="error">{{ error }}</p>
  </section>
</template>

<style scoped>
.timer {
  margin-bottom: 2.5rem;
}

/* One cell, two children stacked in it. Sized off the sidebar so the dial
   shrinks with the pane instead of overflowing it. */
.ring {
  display: grid;
  place-items: center;
  width: min(100%, 200px);
  margin: 0.25rem 0;
}

.ring > * {
  grid-area: 1 / 1;
}

.ring__dial {
  display: block;
  width: 100%;
  height: auto;
}

.ring__dial--stale {
  opacity: 0.45;
}

.ring__track,
.ring__arc {
  fill: none;
  stroke-width: 5;
}

/* Same wash as the week view's summary gauge, for the same reason: a track is
   the absence of progress, not a second colour competing with it. */
.ring__track {
  stroke: color-mix(in srgb, var(--text) 7%, transparent);
}

/* `--actual` and not `--accent`: this is focus that has actually been put in,
   the same thing the session blocks in the week view are drawn with. */
.ring__arc {
  stroke: var(--actual);
  transition: stroke-dashoffset 1s linear;
}

.ring__arc--cut {
  transition: none;
}

@media (prefers-reduced-motion: reduce) {
  .ring__arc {
    transition: none;
  }
}

.clock {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 3rem;
  font-variant-numeric: tabular-nums;
  line-height: 1.1;
  margin: 0;
}

.clock.stale {
  opacity: 0.45;
  text-decoration: line-through;
}

.actions {
  display: flex;
  gap: 0.5rem;
}

.muted {
  opacity: 0.7;
  font-size: 0.9rem;
}

.note {
  font-size: 0.85rem;
  opacity: 0.75;
  margin-top: 0.5rem;
}

.error {
  margin-top: 1rem;
  color: var(--danger);
  font-size: 0.85rem;
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
</style>
