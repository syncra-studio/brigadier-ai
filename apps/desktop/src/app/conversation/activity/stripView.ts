import { workerDone, workerWorking } from "@/app/conversation/workerPresentation";
import type { Task, UserRequest } from "@/ipc/generated";

/** What the Workers strip on the composer shows (THREAD-UX-PLAN.md §3.5). */
export type StripView = {
  /** Its rows, by task id: the workers at it oldest first, then the finished ones. */
  rows: string[];
  working: number;
  waiting: number;
  done: number;
};

/**
 * The session's workers the strip is about: every worker still running or waiting, and those
 * that finished since the user's last message. `null` when there are none, so it hides.
 */
export function workersStrip(tasks: readonly Task[], requests: readonly UserRequest[]): StripView | null {
  const since = requests.reduce((latest, request) => Math.max(latest, request.startedAtMs), 0);
  const byAge = tasks.toSorted((a, b) => a.createdAtMs - b.createdAtMs || a.number - b.number);
  const live = byAge.filter((task) => !workerDone(task));
  const finished = byAge.filter((task) => workerDone(task) && task.updatedAtMs >= since);
  if (live.length + finished.length === 0) return null;
  const working = live.filter(workerWorking).length;
  return {
    rows: [...live, ...finished].map((task) => task.id),
    working,
    waiting: live.length - working,
    done: finished.length,
  };
}

/** "2 workers working · 1 waiting · 1 done": the first count names them, zeros are left out. */
export function stripWords({ working, waiting, done }: Pick<StripView, "working" | "waiting" | "done">): string {
  const counts = [
    [working, "working"],
    [waiting, "waiting"],
    [done, "done"],
  ] as const;
  return counts
    .filter(([count]) => count > 0)
    .map(([count, word], index) => index === 0 ? `${count} ${count === 1 ? "worker" : "workers"} ${word}` : `${count} ${word}`)
    .join(" · ");
}
