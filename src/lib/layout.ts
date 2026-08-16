/**
 * Where overlapping blocks go. Pure, and deliberately ignorant of pixels, the
 * DOM and the backend — this is the part of the week view worth testing on its
 * own.
 */

import { MINUTES_PER_DAY, minutesIntoDay } from "./week";

/** Minutes from the top of a day column. */
export interface Span {
  readonly start: number;
  readonly end: number;
}

export interface Placement {
  /** 0-based column within the block's own cluster. */
  readonly column: number;
  /** Columns the cluster needs. A block is `1 / columns` of the day wide. */
  readonly columns: number;
}

/**
 * Split a day's blocks into columns so that no two overlapping ones sit on top
 * of each other, and blocks that share a stretch of the day share its width.
 *
 * This is graph colouring on an interval graph. Two properties make it easy:
 * interval graphs are perfect, so the fewest columns any set of blocks can be
 * drawn in is exactly the largest number of them that are ever live at once;
 * and taking blocks in order of start time and dropping each into the first
 * column that has room finds that number. So the greedy pass below is not an
 * approximation — it is optimal.
 *
 * Width is shared across a whole *cluster* of transitively overlapping blocks,
 * not just the ones a given block touches, so that a block never straddles the
 * column boundary of its neighbour. That is why the chain A–B–C, where A and C
 * do not overlap at all, still yields two columns and two half-width blocks
 * rather than a full-width A beside a half-width B.
 *
 * Returns placements positionally: `result[i]` belongs to `spans[i]`, whatever
 * order the input came in.
 */
export function assignColumns(spans: readonly Span[]): Placement[] {
  const byStart = spans
    .map((_, index) => index)
    // Earliest first; on a tie the longer block leads, which keeps a block that
    // contains others from being pushed off column 0.
    .sort((a, b) => spans[a].start - spans[b].start || spans[b].end - spans[a].end);

  const placements: Placement[] = new Array(spans.length);
  const columnOf: number[] = new Array(spans.length).fill(0);

  /** Indices in the cluster being built. */
  let cluster: number[] = [];
  /** How far each column of that cluster is occupied to. */
  let columnEnds: number[] = [];
  let clusterEnd = -Infinity;

  function closeCluster(): void {
    for (const index of cluster) {
      placements[index] = { column: columnOf[index], columns: columnEnds.length };
    }
    cluster = [];
    columnEnds = [];
    clusterEnd = -Infinity;
  }

  for (const index of byStart) {
    const span = spans[index];

    // Nothing already placed reaches this block, and since the remaining blocks
    // start even later, nothing will reach back across this line either.
    if (span.start >= clusterEnd) {
      closeCluster();
    }

    // Half-open: a block starting exactly when another ends may reuse its column.
    let column = columnEnds.findIndex((end) => end <= span.start);
    if (column === -1) {
      column = columnEnds.length;
      columnEnds.push(-Infinity);
    }

    columnEnds[column] = Math.max(columnEnds[column], span.end);
    columnOf[index] = column;
    cluster.push(index);
    clusterEnd = Math.max(clusterEnd, span.end);
  }
  closeCluster();

  return placements;
}

export interface TimeRange {
  readonly start: Date;
  readonly end: Date;
}

/** One block's presence on one day column. */
export interface DaySegment<T> {
  readonly item: T;
  /** Index into the `dayStarts` that produced it. */
  readonly day: number;
  /** Minutes from local midnight, `0 <= start < end <= 1440`. */
  readonly start: number;
  readonly end: number;
  /** The block began before this day, or runs past it — for squaring off edges. */
  readonly continuesBefore: boolean;
  readonly continuesAfter: boolean;
}

/**
 * Cut each item into one segment per day column it appears in, so that an event
 * running through midnight is drawn in both days rather than off the bottom of
 * one.
 *
 * `dayStarts` is the 8 local midnights bounding 7 days (see `dayBoundaries`).
 * Positions come from the wall clock rather than from elapsed time, which is
 * what keeps blocks under the right hour label on the two days a year that are
 * not 24 hours long.
 */
export function sliceIntoDays<T extends TimeRange>(
  items: readonly T[],
  dayStarts: readonly Date[],
): DaySegment<T>[] {
  const segments: DaySegment<T>[] = [];

  for (const item of items) {
    const from = item.start.getTime();
    const to = item.end.getTime();

    for (let day = 0; day + 1 < dayStarts.length; day += 1) {
      const dayFrom = dayStarts[day].getTime();
      const dayTo = dayStarts[day + 1].getTime();
      if (from >= dayTo || to <= dayFrom) {
        continue;
      }

      // Reaching the boundary and crossing it are different questions. An event
      // ending at exactly midnight fills this day to the bottom (1440, not the
      // 0 its wall clock reads) but does not continue into the next one, which
      // the overlap test above has already excluded.
      const continuesBefore = from < dayFrom;
      const continuesAfter = to > dayTo;
      const start = from <= dayFrom ? 0 : minutesIntoDay(item.start);
      const end = to >= dayTo ? MINUTES_PER_DAY : minutesIntoDay(item.end);

      // Only a zero-length event gets this far, and there is nothing to draw.
      if (end <= start) {
        continue;
      }

      segments.push({ item, day, start, end, continuesBefore, continuesAfter });
    }
  }

  return segments;
}
