<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, useTemplateRef } from "vue";

import { useCalendar } from "../composables/useCalendar";
import { useSessions } from "../composables/useSessions";
import { useWeekDrag, type DragState, type WeekMinute } from "../composables/useWeekDrag";
import { assignColumns, sliceIntoDays, type DaySegment } from "../lib/layout";
import {
  barWidths,
  formatDuration,
  formatRatio,
  summarise,
} from "../lib/summary";
import {
  DAYS_PER_WEEK,
  MINUTES_PER_DAY,
  formatTime,
  fromWeekMinute,
  isSameDay,
  minutesIntoDay,
  monthLabel,
  toWeekMinute,
  weekdayLabel,
} from "../lib/week";
import { isEditable, type CalEvent } from "../types/calendar";
import type { Session } from "../types/session";

/** Pixels per hour. The grid is a fixed 24 hours tall and scrolls. */
const HOUR_HEIGHT = 48;

/** How close to an edge counts as grabbing it rather than the block itself. */
const EDGE_GRAB = 6;

/** The id the block being dragged out of empty space renders under. */
const DRAFT_ID = "__draft__";

/**
 * How much of a day column each lane gets, as a percentage.
 *
 * The plan sits in the left half, what actually happened in the right, so the
 * two can be read against each other at a glance without either being able to
 * hide the other. Split by arithmetic rather than by nesting the lanes in their
 * own elements: `useWeekDrag` maps a pointer's x across the whole grid to a day
 * index, and `@pointerdown.self` on the day column is what starts a create
 * drag. An extra layer of boxes would break both.
 */
const LANE_WIDTH = 50;

/**
 * The shortest session, in minutes, that gets its start time written on it.
 *
 * Below this the label would be sliced through the middle, which reads as a
 * rendering fault rather than as a short pomodoro. A bare bar says "short" more
 * clearly than half a glyph does, and the hover text still has the detail.
 */
const LABEL_MIN_MINUTES = Math.ceil((16 / HOUR_HEIGHT) * 60);

const calendar = useCalendar();
const { days, events } = calendar;
const { sessions, reload: reloadSessions } = useSessions(days);

const gridRef = useTemplateRef<HTMLElement>("grid");
const scrollerRef = useTemplateRef<HTMLElement>("scroller");

const selectedId = ref<string | null>(null);
const editingId = ref<string | null>(null);
const editingText = ref("");

/** Autofocus the title box the moment it appears. */
const vFocus = {
  mounted: (el: HTMLInputElement) => el.focus(),
};

const { drag, startCreate, startEdit } = useWeekDrag({
  grid: gridRef,
  commit: (state) => void commitDrag(state),
});

/** The grid's coordinate system, anchored on the week being shown. */
function toMinute(at: Date): WeekMinute {
  return toWeekMinute(at, days.value[0]);
}

function toDate(minute: WeekMinute): Date {
  return fromWeekMinute(minute, days.value[0]);
}

/**
 * The events as they should look *right now*, drag included.
 *
 * The dragged block is substituted rather than mutated, so releasing on a failed
 * write leaves nothing to undo — and because the layout below is recomputed from
 * this, blocks re-share their width live as one is dragged over another.
 */
const visible = computed<CalEvent[]>(() => {
  const state = drag.value;
  if (state === null) {
    return events.value;
  }

  if (state.kind === "create") {
    return [
      ...events.value,
      {
        id: DRAFT_ID,
        uid: "",
        summary: "",
        start: toDate(state.start),
        end: toDate(state.end),
        recurring: false,
        allDay: false,
      },
    ];
  }

  return events.value.map((event) =>
    event.id === state.id
      ? { ...event, start: toDate(state.start), end: toDate(state.end) }
      : event,
  );
});

const allDayEvents = computed(() => visible.value.filter((event) => event.allDay));

interface Placed<T> {
  segment: DaySegment<T>;
  column: number;
  columns: number;
}

/** Cut blocks into day columns and share the width of the ones that overlap. */
function placeByDay<T extends { start: Date; end: Date }>(
  items: readonly T[],
): Placed<T>[][] {
  const segments = sliceIntoDays(items, days.value);

  return Array.from({ length: DAYS_PER_WEEK }, (_, day) => {
    const ofDay = segments.filter((segment) => segment.day === day);
    // The whole point of keeping this pure: it runs on every pointermove.
    const placements = assignColumns(ofDay);
    return ofDay.map((segment, index) => ({
      segment,
      column: placements[index].column,
      columns: placements[index].columns,
    }));
  });
}

/** The left lane: what was planned. */
const byDay = computed(() =>
  placeByDay(visible.value.filter((event) => !event.allDay)),
);

/** The right lane: what actually ran. */
const sessionsByDay = computed(() => placeByDay(sessions.value));

const summary = computed(() =>
  summarise(events.value, sessions.value, days.value[0], days.value[DAYS_PER_WEEK]),
);

const bars = computed(() => barWidths(summary.value));

/** The same placement maths for either lane; `lane` 0 is plans, 1 is sessions. */
function styleOf<T>(placed: Placed<T>, lane: 0 | 1): Record<string, string> {
  const { start, end } = placed.segment;
  const width = LANE_WIDTH / placed.columns;
  return {
    top: `${(start / MINUTES_PER_DAY) * 100}%`,
    height: `${((end - start) / MINUTES_PER_DAY) * 100}%`,
    left: `${lane * LANE_WIDTH + placed.column * width}%`,
    width: `${width}%`,
  };
}

function onDayPointerDown(event: PointerEvent): void {
  selectedId.value = null;
  // Commit rather than discard: the input is about to lose focus anyway, and
  // dropping what the user just typed because they clicked elsewhere would be
  // the rudest possible way to end a rename.
  void commitEditing();
  startCreate(event);
}

function onEventPointerDown(event: PointerEvent, placed: Placed<CalEvent>): void {
  const item = placed.segment.item;
  selectedId.value = item.id;
  if (!isEditable(item) || editingId.value === item.id) {
    return;
  }

  const target = {
    id: item.id,
    uid: item.uid,
    start: toMinute(item.start),
    end: toMinute(item.end),
  };

  const box = (event.currentTarget as HTMLElement).getBoundingClientRect();
  const offset = event.clientY - box.top;
  // On a very short block there is no room for two edges and a middle.
  const edge = Math.min(EDGE_GRAB, box.height / 3);

  // Only an edge the block actually shows can be dragged: a segment running on
  // into the next day has no bottom of its own to take hold of.
  if (offset <= edge && !placed.segment.continuesBefore) {
    startEdit(event, target, "resizeStart");
  } else if (box.height - offset <= edge && !placed.segment.continuesAfter) {
    startEdit(event, target, "resizeEnd");
  } else {
    startEdit(event, target, "move");
  }
}

async function commitDrag(state: DragState): Promise<void> {
  const start = toDate(state.start);
  const end = toDate(state.end);

  if (state.kind === "create") {
    const made = await calendar.create({ summary: "", start, end });
    if (made !== null) {
      selectedId.value = made.id;
      beginEditing(made);
    }
    return;
  }

  const existing = events.value.find((event) => event.id === state.id);
  if (state.uid === null || existing === undefined) {
    return;
  }
  await calendar.update(state.uid, { summary: existing.summary, start, end });
}

function beginEditing(event: CalEvent): void {
  if (!isEditable(event)) {
    return;
  }
  editingId.value = event.id;
  editingText.value = event.summary;
}

function stopEditing(): void {
  editingId.value = null;
  editingText.value = "";
}

async function commitEditing(): Promise<void> {
  const id = editingId.value;
  const text = editingText.value;
  stopEditing();

  const event = events.value.find((candidate) => candidate.id === id);
  if (event === undefined || event.summary === text) {
    return;
  }
  await calendar.update(event.uid, { summary: text, start: event.start, end: event.end });
}

async function removeSelected(): Promise<void> {
  const event = events.value.find((candidate) => candidate.id === selectedId.value);
  if (event === undefined || !isEditable(event)) {
    return;
  }
  selectedId.value = null;
  await calendar.remove(event.uid);
}

/** Is the user typing into something? Then Backspace means backspace. */
function isTyping(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLElement &&
    (target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName))
  );
}

// Window-wide, so the shortcut works without the grid holding focus. That also
// means it fires while the timer's settings are being edited, hence the guard:
// the listener has no idea what else is on screen.
function onKeyDown(event: KeyboardEvent): void {
  if (editingId.value !== null || selectedId.value === null || isTyping(event.target)) {
    return;
  }
  if (event.key === "Backspace" || event.key === "Delete") {
    event.preventDefault();
    void removeSelected();
  }
}

/**
 * The "now" line. It re-reads the clock rather than counting, so it is a display
 * of the wall clock and not a timer — invariant #2 is about the pomodoro count,
 * which still lives in Rust.
 */
const now = ref(new Date());
const clock = setInterval(() => {
  now.value = new Date();
}, 30_000);

const nowLine = computed(() => {
  const index = days.value.findIndex((day) => isSameDay(day, now.value));
  if (index < 0 || index >= DAYS_PER_WEEK) {
    return null;
  }
  return { day: index, top: (minutesIntoDay(now.value) / MINUTES_PER_DAY) * 100 };
});

const hours = Array.from({ length: 24 }, (_, hour) => hour);

const title = computed(() => monthLabel(days.value));

function dayClasses(day: Date): Record<string, boolean> {
  return { "is-today": isSameDay(day, now.value) };
}

function eventClasses(placed: Placed<CalEvent>): Record<string, boolean> {
  const item = placed.segment.item;
  return {
    "is-selected": selectedId.value === item.id,
    "is-readonly": !isEditable(item),
    "is-dragging": drag.value?.id === item.id || item.id === DRAFT_ID,
    "runs-in": placed.segment.continuesBefore,
    "runs-out": placed.segment.continuesAfter,
  };
}

function sessionClasses(placed: Placed<Session>): Record<string, boolean> {
  const item = placed.segment.item;
  return {
    "is-break": item.kind === "break",
    "is-aborted": item.outcome === "aborted",
    "is-void": item.outcome === "invalidated",
    "runs-in": placed.segment.continuesBefore,
    "runs-out": placed.segment.continuesAfter,
  };
}

/** What a session was, spelled out for the hover text and for screen readers. */
function sessionLabel(session: Session): string {
  const kind = session.kind === "work" ? "Work" : "Break";
  const span = `${formatTime(session.start)}–${formatTime(session.end)}`;
  const counted = formatDuration(session.actualSec);

  switch (session.outcome) {
    case "completed":
      return `${kind} ${span}, counted ${counted}`;
    case "aborted":
      return `${kind} ${span}, counted ${counted}, ended early (planned ${formatDuration(
        session.plannedSec,
      )})`;
    case "invalidated":
      return `${kind} ${span}, machine slept — this one was voided`;
  }
}

/** Why a block will not budge, so the refusal is not a mystery. */
function readonlyReason(event: CalEvent): string {
  if (event.recurring) {
    return "Recurring event — edit the RRULE in the .ics directly";
  }
  if (event.allDay) {
    return "All-day event";
  }
  return "This VEVENT has no UID, so there is nothing to address";
}

onMounted(() => {
  window.addEventListener("keydown", onKeyDown);
  void calendar.reload();
  void reloadSessions();
  // Open on the working day rather than on midnight.
  if (scrollerRef.value !== null) {
    scrollerRef.value.scrollTop = 8 * HOUR_HEIGHT;
  }
});

onUnmounted(() => {
  window.removeEventListener("keydown", onKeyDown);
  clearInterval(clock);
});
</script>

<template>
  <section class="week">
    <header class="week__bar">
      <h2>{{ title }}</h2>
      <div class="week__nav">
        <button title="Previous week" @click="calendar.goToWeek(-1)">‹</button>
        <button @click="calendar.goToToday()">Today</button>
        <button title="Next week" @click="calendar.goToWeek(1)">›</button>
      </div>
    </header>

    <p v-if="calendar.error.value" role="alert" class="week__error">
      {{ calendar.error.value }}
    </p>

    <!-- The claim the whole app makes, in three numbers: what the week was for,
         what it actually held, and how much of the first the second covered. -->
    <div class="week__summary">
      <dl class="week__stats">
        <div class="week__stat">
          <dt><i class="week__swatch week__swatch--plan" />Planned</dt>
          <dd>{{ formatDuration(summary.plannedSec) }}</dd>
        </div>
        <div class="week__stat">
          <dt><i class="week__swatch week__swatch--actual" />Focused</dt>
          <dd>{{ formatDuration(summary.focusedSec) }}</dd>
        </div>
        <div class="week__stat">
          <dt>Coverage</dt>
          <dd class="week__stat--lead">{{ formatRatio(summary.ratio) }}</dd>
        </div>
      </dl>

      <div class="week__gauge" aria-hidden="true">
        <div class="week__gauge-track">
          <div class="week__gauge-fill week__gauge-fill--plan" :style="{ width: `${bars.planned}%` }" />
        </div>
        <div class="week__gauge-track">
          <div
            class="week__gauge-fill week__gauge-fill--actual"
            :style="{ width: `${bars.focused}%` }"
          />
        </div>
      </div>
    </div>

    <div class="week__head">
      <div class="week__gutter" />
      <div
        v-for="(day, index) in days.slice(0, DAYS_PER_WEEK)"
        :key="index"
        class="week__day-head"
        :class="dayClasses(day)"
      >
        <span class="week__weekday">{{ weekdayLabel(index) }}</span>
        <span class="week__date">{{ day.getDate() }}</span>
      </div>
    </div>

    <!-- All-day events have no place on a 24-hour grid, but leaving them
         invisible would be worse than a strip that cannot be dragged. -->
    <div v-if="allDayEvents.length" class="week__allday">
      <div class="week__gutter week__gutter--label">All-day</div>
      <div
        v-for="(day, index) in days.slice(0, DAYS_PER_WEEK)"
        :key="index"
        class="week__allday-cell"
      >
        <span
          v-for="event in allDayEvents.filter(
            (candidate) => candidate.start < days[index + 1] && candidate.end > day,
          )"
          :key="event.id"
          class="week__allday-chip"
          :title="readonlyReason(event)"
        >
          {{ event.summary || "(untitled)" }}
        </span>
      </div>
    </div>

    <div ref="scroller" class="week__scroller">
      <div class="week__body" :style="{ height: `${24 * HOUR_HEIGHT}px` }">
        <div class="week__gutter week__hours">
          <div v-for="hour in hours" :key="hour" class="week__hour-label">
            <span v-if="hour > 0">{{ String(hour).padStart(2, "0") }}:00</span>
          </div>
        </div>

        <div ref="grid" class="week__grid">
          <div
            v-for="(day, index) in days.slice(0, DAYS_PER_WEEK)"
            :key="index"
            class="week__day"
            :class="dayClasses(day)"
            @pointerdown.self="onDayPointerDown"
          >
            <div
              v-if="nowLine?.day === index"
              class="week__now"
              :style="{ top: `${nowLine.top}%` }"
            />

            <!-- Read-only by construction: a session is something that already
                 happened, and there is no gesture here that could change it.
                 With no handlers a press on one does nothing at all — the day's
                 own `pointerdown.self` will not fire for a press that landed on
                 a child — so the only cost is that a create drag cannot begin
                 on top of a session. Plans are drawn in the left lane anyway,
                 and that half is untouched. Worth it for the hover text, which
                 is where a pomodoro says how it ended. -->
            <div
              v-for="placed in sessionsByDay[index]"
              :key="`${placed.segment.item.id}-${placed.segment.day}`"
              class="week__session"
              :class="sessionClasses(placed)"
              :style="styleOf(placed, 1)"
              :title="sessionLabel(placed.segment.item)"
            >
              <span
                v-if="placed.segment.end - placed.segment.start >= LABEL_MIN_MINUTES"
                class="week__session-time"
              >
                {{ formatTime(placed.segment.item.start) }}
              </span>
            </div>

            <div
              v-for="placed in byDay[index]"
              :key="`${placed.segment.item.id}-${placed.segment.day}`"
              class="week__event"
              :class="eventClasses(placed)"
              :style="styleOf(placed, 0)"
              :title="
                isEditable(placed.segment.item) ? undefined : readonlyReason(placed.segment.item)
              "
              @pointerdown.stop="onEventPointerDown($event, placed)"
              @dblclick.stop="beginEditing(placed.segment.item)"
            >
              <span class="week__event-time">
                {{ formatTime(placed.segment.item.start) }}
              </span>

              <input
                v-if="editingId === placed.segment.item.id"
                v-model="editingText"
                v-focus
                class="week__event-input"
                placeholder="New event"
                @pointerdown.stop
                @keydown.enter.prevent="commitEditing"
                @keydown.esc.prevent="stopEditing"
                @blur="commitEditing"
              />
              <span v-else class="week__event-title">
                {{ placed.segment.item.summary || "(untitled)" }}
                <span v-if="placed.segment.item.recurring" title="Recurring event">↻</span>
              </span>

              <button
                v-if="selectedId === placed.segment.item.id && isEditable(placed.segment.item)"
                class="week__event-delete"
                title="Delete"
                @pointerdown.stop
                @click.stop="removeSelected"
              >
                ×
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  </section>
</template>

<style scoped>
.week {
  /* `--actual` and `--accent` both come from `src/style.css`. They are kept a
     full hue apart on purpose: `--accent` already means "now" (today's column,
     the current-time line), so what actually happened needs its own colour or
     the two readings would blur into each other. */
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  font-size: 0.85rem;
}

.week__summary {
  display: flex;
  align-items: center;
  gap: 1.25rem;
  padding: 0 0 0.75rem;
}

.week__stats {
  display: flex;
  gap: 1.25rem;
  margin: 0;
}

.week__stat dt {
  display: flex;
  align-items: center;
  gap: 0.3rem;
  font-size: 0.7rem;
  opacity: 0.6;
}

.week__stat dd {
  margin: 0;
  font-size: 0.95rem;
  font-variant-numeric: tabular-nums;
}

.week__stat--lead {
  font-weight: 600;
}

.week__swatch {
  width: 0.6rem;
  height: 0.6rem;
  border-radius: 2px;
}

.week__swatch--plan {
  border: 1px solid var(--border-strong);
  background: var(--surface);
}

.week__swatch--actual {
  background: var(--actual);
}

/* Plan over actual, same scale, so the shortfall is the gap between two bars
   rather than a number to be worked out. */
.week__gauge {
  flex: 1 1 auto;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.week__gauge-track {
  height: 5px;
  border-radius: 3px;
  background: color-mix(in srgb, var(--text) 7%, transparent);
}

.week__gauge-fill {
  height: 100%;
  border-radius: 3px;
  transition: width 0.2s ease-out;
}

.week__gauge-fill--plan {
  background: color-mix(in srgb, var(--text) 30%, transparent);
}

.week__gauge-fill--actual {
  background: var(--actual);
}

.week__bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 0 0.75rem;
}

.week__bar h2 {
  font-size: 1rem;
  font-weight: 600;
  margin: 0;
}

.week__nav {
  display: flex;
  gap: 0.35rem;
}

.week__nav button {
  padding: 0.25rem 0.6rem;
  font: inherit;
  border: 1px solid var(--border-strong);
  border-radius: 6px;
  background: var(--surface);
  color: inherit;
  cursor: pointer;
}

.week__nav button:hover {
  background: color-mix(in srgb, var(--accent) 10%, var(--surface));
  border-color: color-mix(in srgb, var(--accent) 45%, var(--border-strong));
}

.week__error {
  margin: 0 0 0.5rem;
  color: var(--danger);
  font-size: 0.8rem;
}

.week__head,
.week__allday {
  display: flex;
  border-bottom: 1px solid var(--border);
}

.week__gutter {
  flex: 0 0 3.5rem;
  box-sizing: border-box;
}

.week__gutter--label {
  padding: 0.35rem 0.5rem;
  opacity: 0.55;
  font-size: 0.7rem;
  text-align: right;
}

.week__day-head {
  flex: 1 1 0;
  min-width: 0;
  display: flex;
  align-items: baseline;
  justify-content: center;
  gap: 0.35rem;
  padding: 0.4rem 0;
  border-left: 1px solid var(--border);
}

.week__weekday {
  opacity: 0.6;
  font-size: 0.75rem;
}

.week__date {
  font-variant-numeric: tabular-nums;
}

.week__day-head.is-today .week__date {
  color: var(--accent-ink);
  font-weight: 600;
}

.week__allday-cell {
  flex: 1 1 0;
  min-width: 0;
  display: flex;
  flex-wrap: wrap;
  gap: 0.2rem;
  padding: 0.25rem;
  border-left: 1px solid var(--border);
}

.week__allday-chip {
  max-width: 100%;
  padding: 0.05rem 0.3rem;
  border-radius: 4px;
  background: color-mix(in srgb, var(--text) 10%, transparent);
  font-size: 0.7rem;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.week__scroller {
  flex: 1 1 auto;
  min-height: 0;
  overflow-y: auto;
}

.week__body {
  display: flex;
  position: relative;
}

.week__hours {
  position: relative;
}

.week__hour-label {
  height: 48px;
  box-sizing: border-box;
  padding-right: 0.5rem;
  text-align: right;
  font-size: 0.7rem;
  font-variant-numeric: tabular-nums;
  opacity: 0.55;
  /* Sitting on the line rather than under it. */
  transform: translateY(-0.55em);
}

.week__grid {
  flex: 1 1 auto;
  display: flex;
  min-width: 0;
}

.week__day {
  position: relative;
  flex: 1 1 0;
  min-width: 0;
  /* Heavier than the lane split below, so a day boundary always reads as the
     stronger of the two lines. */
  border-left: 1px solid var(--border-strong);
  /* One line per hour, one fainter per half hour, and a wash over the right
     half of the column. The wash rather than a divider line: a line of its own
     is indistinguishable from the day borders either side, and the grid ends up
     reading as fourteen columns. Tinted in the "actual" colour and drawn even
     where nothing sits in either lane, so the split is a property of the column
     rather than something to infer from where the blocks happened to land. */
  background-image: linear-gradient(
      to right,
      transparent 0 50%,
      color-mix(in srgb, var(--actual) 9%, transparent) 50% 100%
    ),
    repeating-linear-gradient(
      to bottom,
      color-mix(in srgb, var(--text) 12%, transparent) 0 1px,
      transparent 1px 48px
    ),
    repeating-linear-gradient(
      to bottom,
      transparent 0 24px,
      color-mix(in srgb, var(--text) 6%, transparent) 24px 25px,
      transparent 25px 48px
    );
  touch-action: none;
}

.week__day.is-today {
  background-color: color-mix(in srgb, var(--accent) 6%, transparent);
}

.week__now {
  position: absolute;
  left: 0;
  right: 0;
  height: 1px;
  background: var(--accent);
  z-index: 3;
  pointer-events: none;
}

.week__now::before {
  content: "";
  position: absolute;
  left: -3px;
  top: -2.5px;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--accent);
}

.week__event {
  position: absolute;
  box-sizing: border-box;
  padding: 1px 4px;
  border: 1px solid var(--border-strong);
  border-radius: 4px;
  /* Lighter than the grid it sits on, so a plan reads as paper laid on the
     desk. The grid's own tint shows through nowhere. */
  background: var(--surface);
  overflow: hidden;
  cursor: grab;
  touch-action: none;
  z-index: 1;
  /* Room to see the block underneath at the shared edge. */
  outline: 1px solid transparent;
}

.week__event.runs-in {
  border-top-left-radius: 0;
  border-top-right-radius: 0;
  border-top-style: dashed;
}

.week__event.runs-out {
  border-bottom-left-radius: 0;
  border-bottom-right-radius: 0;
  border-bottom-style: dashed;
}

.week__event.is-selected {
  border-color: var(--accent);
  z-index: 2;
}

.week__event.is-dragging {
  z-index: 4;
  cursor: grabbing;
  box-shadow: var(--shadow-drag);
}

.week__event.is-readonly {
  cursor: default;
  background-image: repeating-linear-gradient(
    45deg,
    transparent 0 4px,
    color-mix(in srgb, var(--text) 7%, transparent) 4px 8px
  );
}

/* The right lane. Shares the geometry of a planned block and none of its
   affordances -- no cursor, no drag, nothing to select. */
.week__session {
  position: absolute;
  box-sizing: border-box;
  padding: 0 3px;
  border: 1px solid var(--actual);
  border-radius: 4px;
  background: color-mix(in srgb, var(--actual) 65%, var(--bg));
  overflow: hidden;
  cursor: default;
  z-index: 1;
}

.week__session.runs-in {
  border-top-left-radius: 0;
  border-top-right-radius: 0;
  border-top-style: dashed;
}

.week__session.runs-out {
  border-bottom-left-radius: 0;
  border-bottom-right-radius: 0;
  border-bottom-style: dashed;
}

/* A break is time that passed, not time that was spent, so it recedes. */
.week__session.is-break {
  background: color-mix(in srgb, var(--actual) 14%, var(--bg));
  border-color: color-mix(in srgb, var(--actual) 40%, transparent);
}

/* Stopped early: the block already shows how far it got, the open bottom edge
   says that is not where it was meant to end. */
.week__session.is-aborted {
  background: color-mix(in srgb, var(--actual) 32%, var(--bg));
  border-bottom-style: dashed;
}

/* Voided by a suspend. Still drawn -- the time really did pass -- but struck
   through, and excluded from the focus total above. */
.week__session.is-void {
  background: repeating-linear-gradient(
    45deg,
    transparent 0 3px,
    color-mix(in srgb, var(--text) 12%, transparent) 3px 6px
  );
  border-color: color-mix(in srgb, var(--text) 30%, transparent);
  opacity: 0.75;
}

.week__session.is-void::after {
  content: "";
  position: absolute;
  left: 2px;
  right: 2px;
  top: 50%;
  border-top: 1px solid color-mix(in srgb, var(--text) 55%, transparent);
}

.week__session-time {
  display: block;
  font-size: 0.65rem;
  font-variant-numeric: tabular-nums;
  line-height: 1.3;
  white-space: nowrap;
}

.week__session.is-break .week__session-time,
.week__session.is-void .week__session-time {
  opacity: 0.6;
}

.week__event-time {
  display: block;
  font-size: 0.65rem;
  font-variant-numeric: tabular-nums;
  opacity: 0.6;
  line-height: 1.2;
}

.week__event-title {
  display: block;
  font-size: 0.75rem;
  line-height: 1.25;
  overflow-wrap: anywhere;
}

.week__event-input {
  width: 100%;
  padding: 0;
  border: 0;
  background: transparent;
  color: inherit;
  font: inherit;
  font-size: 0.75rem;
  outline: none;
}

.week__event-delete {
  position: absolute;
  top: 0;
  right: 0;
  width: 1.1rem;
  height: 1.1rem;
  padding: 0;
  border: 0;
  border-radius: 4px;
  background: transparent;
  color: inherit;
  font-size: 0.85rem;
  line-height: 1;
  cursor: pointer;
  opacity: 0.55;
}

.week__event-delete:hover {
  opacity: 1;
  color: var(--danger);
  background: color-mix(in srgb, var(--danger) 15%, transparent);
}
</style>
