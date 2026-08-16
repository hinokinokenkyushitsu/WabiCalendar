/**
 * Mirrors `CalEvent` and `EventDraft` in `src-tauri/src/calendar/ics.rs`.
 * Maintained by hand — change both sides together.
 */

/** As it comes over the wire: timestamps are RFC 3339 strings. */
export interface CalEventWire {
  id: string;
  uid: string;
  summary: string;
  start: string;
  end: string;
  recurring: boolean;
  allDay: boolean;
}

export interface EventDraftWire {
  summary: string;
  start: string;
  end: string;
}

/** The same event with its timestamps parsed, which is what the view works in. */
export interface CalEvent {
  id: string;
  uid: string;
  summary: string;
  start: Date;
  end: Date;
  recurring: boolean;
  allDay: boolean;
}

/**
 * Can a drag change this block?
 *
 * Three ways to be read-only: it is an occurrence of a repeating event (v1 will
 * not rewrite an RRULE), it is an all-day event (there is no time to drag), or
 * the VEVENT carries no UID and so cannot be found again to edit.
 */
export function isEditable(event: CalEvent): boolean {
  return !event.recurring && !event.allDay && event.uid !== "";
}

export function parseEvent(wire: CalEventWire): CalEvent {
  return {
    ...wire,
    start: new Date(wire.start),
    end: new Date(wire.end),
  };
}
