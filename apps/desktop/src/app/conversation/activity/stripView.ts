import { rowFinished, rowWaiting } from "@/app/conversation/activity/workerState";
import type { WorkerRow } from "@/ipc/generated";

/** What the Workers strip on the composer shows (THREAD-UX-PLAN.md §3.5): the workers at it. */
export type StripView = {
  /** Its rows, by task id: the workers working or waiting, oldest first. */
  rows: string[];
  working: number;
  waiting: number;
  /** Whether Stop all has anything to stop. */
  stoppable: boolean;
};

/**
 * The session's workers the strip is about: only those still at work or waiting (on the user, on
 * a limit), by their rows (`workerStateOf`). Finished ones (done, merged, stopped, failed) leave
 * it at once and stay in the context card. `null` when none are at work, so it hides.
 */
export function workersStrip(rows: readonly WorkerRow[], now: number): StripView | null {
  const live = rows.filter((row) => !rowFinished(row, now)).toSorted((a, b) => a.startedAtMs - b.startedAtMs || a.number - b.number);
  if (live.length === 0) return null;
  const waiting = live.filter((row) => rowWaiting(row, now)).length;
  return {
    rows: live.map((row) => row.id),
    working: live.length - waiting,
    waiting,
    // The daemon's `stop_workers` stops every worker whose CLI still runs.
    stoppable: live.some((row) => !row.gone),
  };
}

/** "2 workers working · 1 waiting": the first count names them, zeros are left out. */
export function stripWords({ working, waiting }: Pick<StripView, "working" | "waiting">): string {
  const counts = [
    [working, "working"],
    [waiting, "waiting"],
  ] as const;
  return counts
    .filter(([count]) => count > 0)
    .map(([count, word], index) => (index === 0 ? `${count} ${count === 1 ? "worker" : "workers"} ${word}` : `${count} ${word}`))
    .join(" · ");
}
