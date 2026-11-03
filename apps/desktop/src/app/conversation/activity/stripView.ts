import { rowOnHand, rowWaiting, workerStateOf } from "@/app/conversation/activity/workerState";
import type { WorkerRow } from "@/ipc/generated";
import { isNativeWorker } from "@/state/nativeWorkers";

/** What the Workers strip on the composer shows (THREAD-UX-PLAN.md §3.5): the workers at it. */
export type StripView = {
  /** Its rows, by task id: the workers working, waiting or paused, oldest first. */
  rows: string[];
  working: number;
  waiting: number;
  paused: number;
  /** The ones Pause all pauses: at work or waiting, their CLI still there (D199). */
  pausable: string[];
};

/**
 * The session's workers the strip is about: only those still at work, waiting (on the user, on
 * a limit) or paused, by their rows (`workerStateOf`). Finished ones (done, merged, ended,
 * failed) leave it at once and stay in the context card. `null` when none are, so it hides.
 */
export function workersStrip(rows: readonly WorkerRow[], now: number): StripView | null {
  const live = rows.filter((row) => rowOnHand(row, now)).toSorted((a, b) => a.startedAtMs - b.startedAtMs || a.number - b.number);
  if (live.length === 0) return null;
  const waiting = live.filter((row) => rowWaiting(row, now)).length;
  const paused = live.filter((row) => workerStateOf(row, now).kind === "paused");
  return {
    rows: live.map((row) => row.id),
    working: live.length - waiting - paused.length,
    waiting,
    paused: paused.length,
    pausable: live.filter((row) => !row.gone && !paused.includes(row) && !isNativeWorker(row)).map((row) => row.id),
  };
}

/** "2 workers working · 1 waiting · 1 paused": the first count names them, zeros are left out. */
export function stripWords({ working, waiting, paused }: Pick<StripView, "working" | "waiting" | "paused">): string {
  const counts = [
    [working, "working"],
    [waiting, "waiting"],
    [paused, "paused"],
  ] as const;
  return counts
    .filter(([count]) => count > 0)
    .map(([count, word], index) => (index === 0 ? `${count} ${count === 1 ? "worker" : "workers"} ${word}` : `${count} ${word}`))
    .join(" · ");
}
