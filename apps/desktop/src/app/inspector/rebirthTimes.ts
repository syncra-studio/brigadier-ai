import type { RebirthRecord } from "@/ipc/generated";
import { formatDuration } from "@/lib/format";

/** A wait shorter than this is not worth a mention. */
export const WAIT_SHOWN_MS = 1000;

/**
 * Where a rebirth's time went: writing the handoff note, then the swap (the old CLI retired,
 * the briefing built for the new one), and between them the wait for the orchestrator's next
 * turn, which is idle time, not work. A record made before the swap's start was kept has only
 * the whole span.
 */
type RebirthTimes =
  | { kind: "split"; note: number | null; swap: number; waited: number }
  | { kind: "span"; span: number };

export function rebirthTimes(record: RebirthRecord): RebirthTimes {
  const swapStarted = record.swapStartedAtMs;
  if (swapStarted === null) {
    return { kind: "span", span: record.swappedAtMs - record.prepareStartedAtMs };
  }
  const ready = record.handoffReadyAtMs;
  if (ready === null) {
    return { kind: "split", note: null, swap: record.swappedAtMs - swapStarted, waited: 0 };
  }
  // A swap that could not wait any longer began before the note was ready and waited for it.
  return {
    kind: "split",
    note: ready - record.prepareStartedAtMs,
    swap: record.swappedAtMs - Math.max(swapStarted, ready),
    waited: Math.max(0, swapStarted - ready),
  };
}

/** Rebirth steps take seconds; tenths are enough. */
export function seconds(ms: number): string {
  return `${(ms / 1000).toFixed(1)} s`;
}

export function tookText(times: RebirthTimes): string {
  if (times.kind === "span") return `prepared to swapped in ${seconds(times.span)}`;
  const work = (times.note ?? 0) + times.swap;
  const waited =
    times.waited >= WAIT_SHOWN_MS ? ` · waited ${formatDuration(times.waited)} for the next turn` : "";
  return `took ${seconds(work)}${waited}`;
}
