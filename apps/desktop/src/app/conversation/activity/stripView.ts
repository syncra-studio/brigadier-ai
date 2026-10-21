import { workerDone, workerStoppable, workerWorking } from "@/app/conversation/workerPresentation";
import type { Task } from "@/ipc/generated";

/** What the Workers strip on the composer shows (THREAD-UX-PLAN.md §3.5): the workers at it. */
export type StripView = {
  /** Its rows, by task id: the workers running or waiting, oldest first. */
  rows: string[];
  working: number;
  waiting: number;
  /** Whether Stop all has anything to stop. */
  stoppable: boolean;
};

/**
 * The session's workers the strip is about: only those still running or waiting to run. Finished
 * ones (done, stopped, failed) leave it at once and stay in the context card. `null` when none
 * are at work, so it hides.
 */
export function workersStrip(tasks: readonly Task[]): StripView | null {
  const live = tasks.filter((task) => !workerDone(task)).toSorted((a, b) => a.createdAtMs - b.createdAtMs || a.number - b.number);
  if (live.length === 0) return null;
  const working = live.filter(workerWorking).length;
  return {
    rows: live.map((task) => task.id),
    working,
    waiting: live.length - working,
    stoppable: live.some(workerStoppable),
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
