import { workerDone, workerStoppable, workerWorking } from "@/app/conversation/workerPresentation";
import type { Message, Task, UserRequest, WorkerStep } from "@/ipc/generated";

/** What the Workers strip on the composer shows (THREAD-UX-PLAN.md §3.5). */
export type StripView = {
  /** Its rows, by task id: the workers at it oldest first, then the finished ones. */
  rows: string[];
  working: number;
  waiting: number;
  done: number;
  /** Of the finished, those stopped before they finished: the user's Stop all, or the lead. */
  stopped: number;
  /** Whether Stop all has anything to stop. */
  stoppable: boolean;
};

const ENDINGS = new Set<WorkerStep["kind"]>(["finished", "landed", "rejected", "stopped", "failed"]);

/** When a worker last finished: its last ending step, not its last update (a restore updates it). */
function finishedAtMs(task: Task, steps: readonly WorkerStep[]): number {
  const ends = steps.filter((step) => step.taskId === task.id && ENDINGS.has(step.kind)).map((step) => step.atMs);
  return ends.length > 0 ? Math.max(...ends) : task.updatedAtMs;
}

/**
 * When the thread last replied for each request, by request id: what hides its finished workers.
 * A message steered into a running turn takes over that turn's later replies, which carry its
 * request's id while the turn's workers keep theirs; so a reply counts for every request its own
 * was steered into as well (`requests`: the board's).
 */
export function lastReplies(
  messages: readonly Message[],
  requests: Readonly<Record<string, Pick<UserRequest, "steeredInto">>> = {},
): Record<string, number> {
  const replies: Record<string, number> = {};
  const hopLimit = Object.keys(requests).length;
  for (const message of messages) {
    if (message.role !== "assistant" || message.requestId === null) continue;
    // A steer chain is short; the bound only guards against a cycle in stored data.
    let requestId: string | null = message.requestId;
    for (let hops = 0; requestId !== null && hops <= hopLimit; hops += 1) {
      replies[requestId] = Math.max(replies[requestId] ?? 0, message.createdAtMs);
      requestId = requests[requestId]?.steeredInto ?? null;
    }
  }
  return replies;
}

/**
 * The session's workers the strip is about (SESSION-FIXES-PLAN.md Q8): every worker still
 * running or waiting, and those finished whose request's answer isn't posted yet. The answer is
 * the thread's reply for their request after the last of its workers finished, so the thread's
 * answer hides them, not a timer; finished workers stay in the side panel. A worker with no
 * request has no answer to wait for. `loadedFromMs`, when older messages aren't loaded: the
 * oldest loaded one's time; workers that finished before it have their answer out of view, so
 * they count as answered. `null` when there are none, so it hides.
 */
export function workersStrip(
  tasks: readonly Task[],
  replies: Readonly<Record<string, number>>,
  steps: readonly WorkerStep[],
  loadedFromMs: number | null = null,
): StripView | null {
  const byAge = tasks.toSorted((a, b) => a.createdAtMs - b.createdAtMs || a.number - b.number);
  const live = byAge.filter((task) => !workerDone(task));
  const busy = new Set(live.map((task) => task.requestId));
  const lastFinish = new Map<string, number>();
  for (const task of byAge) {
    if (task.requestId === null || !workerDone(task)) continue;
    lastFinish.set(task.requestId, Math.max(lastFinish.get(task.requestId) ?? 0, finishedAtMs(task, steps)));
  }
  const unanswered = (requestId: string | null) => {
    if (requestId === null) return false;
    if (busy.has(requestId)) return true;
    const finishedAt = lastFinish.get(requestId) ?? 0;
    if (loadedFromMs !== null && finishedAt < loadedFromMs) return false;
    return (replies[requestId] ?? -1) <= finishedAt;
  };
  const finished = byAge.filter((task) => workerDone(task) && unanswered(task.requestId));
  if (live.length + finished.length === 0) return null;
  const working = live.filter(workerWorking).length;
  const stopped = finished.filter((task) => task.state === "stopped").length;
  return {
    rows: [...live, ...finished].map((task) => task.id),
    working,
    waiting: live.length - working,
    done: finished.length - stopped,
    stopped,
    stoppable: live.some(workerStoppable),
  };
}

/** "2 workers working · 1 waiting · 1 done": the first count names them, zeros are left out. */
export function stripWords({ working, waiting, done, stopped }: Pick<StripView, "working" | "waiting" | "done" | "stopped">): string {
  const counts = [
    [working, "working"],
    [waiting, "waiting"],
    [done, "done"],
    [stopped, "stopped"],
  ] as const;
  return counts
    .filter(([count]) => count > 0)
    .map(([count, word], index) => index === 0 ? `${count} ${count === 1 ? "worker" : "workers"} ${word}` : `${count} ${word}`)
    .join(" · ");
}
