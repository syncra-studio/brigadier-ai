import { rowFinished, rowWaiting, workerStateOf } from "@/app/conversation/activity/workerState";
import { isNativeWorker } from "@/state/nativeWorkers";
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
  // The session's own subagents have a strip of their own (`subagentsStrip`).
  const live = rows.filter((row) => !isNativeWorker(row) && !rowFinished(row, now)).toSorted((a, b) => a.startedAtMs - b.startedAtMs || a.number - b.number);
  if (live.length === 0) return null;
  const waiting = live.filter((row) => rowWaiting(row, now)).length;
  return {
    rows: live.map((row) => row.id),
    working: live.length - waiting,
    waiting,
    // The daemon's `stop_workers` stops every worker whose CLI still runs.
    stoppable: live.some((row) => !row.gone && (!isNativeWorker(row) || row.native.backgroundId !== null)),
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

/** What the Subagents strip on the composer shows: the session CLI's own agents, as worker rows. */
export type SubagentsView = {
  stopStarts?: Record<string, number>;
  /** Its rows, by row id, oldest first: those at work, and those that ended a moment ago. */
  rows: string[];
  working: number;
  waiting: number;
  done: number;
  failed: number;
  stopped: number;
  /** The background ids its Stop all stops: the running agents the CLI can stop. */
  stops: string[];
};

/**
 * The session's own subagents (`workersOf`'s native rows): while one works or waits, those, and
 * those that ended in the last half minute, as the CLI's own task list keeps a finished agent.
 * Once none works, it stays a moment (`STRIP_TAIL_MS`) so the last end can be read, then hides.
 */
export function subagentsStrip(rows: readonly WorkerRow[], now: number): SubagentsView | null {
  const native = rows.filter(isNativeWorker);
  const shown = native.filter((row) => !rowFinished(row, now))
    .toSorted((a, b) => a.startedAtMs - b.startedAtMs || a.number - b.number);
  if (shown.length === 0) return null;
  const count = (kinds: readonly string[]) => shown.filter((row) => kinds.includes(workerStateOf(row, now).kind)).length;
  const waiting = shown.filter((row) => rowWaiting(row, now)).length;
  return {
    rows: shown.map((row) => row.id),
    working: shown.filter((row) => !rowFinished(row, now)).length - waiting,
    waiting,
    done: count(["done", "doneNotMerged", "merged"]),
    failed: count(["failed"]),
    stopped: count(["stopped"]),
    stopStarts: Object.fromEntries(shown.filter(isNativeWorker).map((row) => [row.native.backgroundId ?? "", row.startedAtMs])),
    stops: shown.filter((row) => !row.gone && !rowFinished(row, now) && row.native.backgroundId !== null).map((row) => row.native.backgroundId!),
  };
}

/** "2 subagents working · 1 done": the first count names them, zeros are left out. */
export function subagentWords(view: Pick<SubagentsView, "working" | "waiting" | "done" | "failed" | "stopped">): string {
  const counts = [
    [view.working, "working"],
    [view.waiting, "waiting"],
    [view.done, "done"],
    [view.failed, "failed"],
    [view.stopped, "stopped"],
  ] as const;
  return counts
    .filter(([count]) => count > 0)
    .map(([count, word], index) => (index === 0 ? `${count} ${count === 1 ? "subagent" : "subagents"} ${word}` : `${count} ${word}`))
    .join(" · ");
}
