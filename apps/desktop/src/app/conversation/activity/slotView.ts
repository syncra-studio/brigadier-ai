import type { CiRun, OrchestratorStep, OrchestratorStepKind, ReviewRun, Task, TaskState } from "@/ipc/generated";

type CiWatchKind = Extract<OrchestratorStepKind, { type: "ciWatch" }>;

/** One push whose CI is still being checked: its latest row. */
export type CiWatchView = {
  repo: string;
  branch: string;
  sha: string;
  /** No run has started yet. */
  waiting: boolean;
  runs: CiRun[];
  /** When its watch began. */
  sinceMs: number;
};

/**
 * Every CI watch still checking (SESSION-FIXES-PLAN.md Q1), oldest first. A watch files a new
 * row each time its runs change, one watch per repository, branch and sha; its latest row says
 * where it stands, and one with a result is over: the thread tells the user that result.
 */
export function ciWatches(steps: readonly OrchestratorStep[]): CiWatchView[] {
  const watches = new Map<string, { latest: CiWatchKind; sinceMs: number }>();
  for (const step of steps) {
    if (step.kind.type !== "ciWatch") continue;
    const key = `${step.kind.repo}\n${step.kind.branch}\n${step.kind.sha}`;
    const known = watches.get(key);
    watches.set(key, { latest: step.kind, sinceMs: Math.min(known?.sinceMs ?? step.atMs, step.atMs) });
  }
  return [...watches.values()]
    .filter(({ latest }) => latest.result === undefined)
    .map(({ latest, sinceMs }) => ({
      repo: latest.repo,
      branch: latest.branch,
      sha: latest.sha,
      waiting: latest.waiting,
      runs: latest.runs,
      sinceMs,
    }))
    .toSorted((a, b) => a.sinceMs - b.sinceMs);
}

/**
 * The CI pill's words, every watch in one: "Waiting for CI to start…" until a run starts, then
 * "Checking CI · 2 of 5 jobs done", naming how many pushes when there are several.
 */
export function ciWords(watches: readonly CiWatchView[]): string {
  const runs = watches.flatMap((watch) => watch.runs);
  const pushes = watches.length > 1 ? ` for ${watches.length} pushes` : "";
  if (runs.length === 0) return `Waiting for CI to start${pushes}…`;
  const done = runs.reduce((sum, run) => sum + run.jobsDone, 0);
  const total = runs.reduce((sum, run) => sum + run.jobsTotal, 0);
  return total > 0 ? `Checking CI${pushes} · ${done} of ${total} jobs done` : `Checking CI${pushes}…`;
}

/** The short sha a pill names a push by. */
export function shortSha(sha: string): string {
  return sha.slice(0, 8);
}

const FINAL: readonly TaskState[] = ["landed", "done", "rejected", "stopped", "failed"];

/** The merge the user said yes to while workers wrote, held until they finish. */
export type HeldMergeView = {
  branch: string;
  base: string;
  /** The write tasks it still waits for. */
  waiting: number;
  /** A review of the session's work still runs: the merge waits for it too. */
  reviewing: boolean;
  sinceMs: number;
};

/**
 * The held merge (SESSION-FIXES-PLAN.md Q3), while it is held: its latest `mergeHeld` row with
 * no merge, Cancel or stop since. It waits for the write tasks that ran when it was held, until
 * each is final (the daemon's `is_final`), and then for the reviews of their landings.
 */
export function heldMerge(
  steps: readonly OrchestratorStep[],
  tasks: Readonly<Record<string, Task>>,
  reviews: readonly ReviewRun[],
): HeldMergeView | null {
  type Held = { kind: Extract<OrchestratorStepKind, { type: "mergeHeld" }>; sinceMs: number };
  let held: Held | null = null as Held | null;
  for (const step of steps) {
    const kind = step.kind;
    const askedIn: string | undefined = held?.kind.askedIn;
    if (kind.type === "mergeHeld") {
      held = { kind, sinceMs: held && askedIn === kind.askedIn ? held.sinceMs : step.atMs };
    } else if (kind.type === "merged" || ((kind.type === "mergeCancelled" || kind.type === "mergeStopped") && kind.askedIn === askedIn)) {
      held = null;
    }
  }
  if (!held) return null;
  const waiting = held.kind.waiting.filter((id) => {
    const task = tasks[id];
    return task !== undefined && !FINAL.includes(task.state);
  }).length;
  const reviewing = reviews.some(
    (review) => review.kind === "code" && review.notify.type === "orchestrator" && review.state.type === "running",
  );
  return { branch: held.kind.branch, base: held.kind.base, waiting, reviewing, sinceMs: held.sinceMs };
}

/** "Merging after 2 workers finish", then "Merging after the review", then "Merging…". */
export function mergeWords(merge: Pick<HeldMergeView, "waiting" | "reviewing">): string {
  if (merge.waiting > 0) return `Merging after ${merge.waiting} ${merge.waiting === 1 ? "worker finishes" : "workers finish"}`;
  return merge.reviewing ? "Merging after the review" : "Merging…";
}

export type SlotRow = "workers" | "ci" | "merge";

/** The slot's rows, newest last (Q8.4): each by when it appeared. At most three, one of each. */
export function slotRows(rows: Partial<Record<SlotRow, number>>): SlotRow[] {
  return (Object.entries(rows) as [SlotRow, number][])
    .toSorted(([, a], [, b]) => a - b)
    .map(([row]) => row);
}
