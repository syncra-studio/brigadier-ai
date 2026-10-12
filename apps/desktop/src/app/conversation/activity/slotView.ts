import { questionOpen } from "@/app/conversation/cards/questionRound";
import type { CiRun, OrchestratorStep, OrchestratorStepKind, Question, ReviewRun, Task, TaskState } from "@/ipc/generated";

type CiWatchKind = Extract<OrchestratorStepKind, { type: "ciWatch" }>;

/** One push whose CI is being checked, or whose result still shows: its latest row. */
export type CiWatchView = {
  repo: string;
  branch: string;
  sha: string;
  /** No run has started yet. */
  waiting: boolean;
  runs: CiRun[];
  /** When its watch began. */
  sinceMs: number;
  /** How it ended ("passed", "failed", "no runs" or "stopped"), once it has. */
  result: string | null;
  /** When it ended. */
  endedAtMs: number | null;
};

/** How long a watch's result stays on the pill once it ends. */
export const CI_LINGER_MS = 4000;

/**
 * Every CI watch still checking (SESSION-FIXES-PLAN.md Q1), oldest first. A watch files a new
 * row each time its runs change, one watch per repository, branch and sha; its latest row says
 * where it stands. One with a result is over: its result shows until `CI_LINGER_MS` after that
 * row, by `nowMs`, and then it goes; the thread tells the user the rest.
 */
export function ciWatches(steps: readonly OrchestratorStep[], nowMs: number): CiWatchView[] {
  const watches = new Map<string, { latest: CiWatchKind; sinceMs: number; atMs: number }>();
  for (const step of steps) {
    if (step.kind.type !== "ciWatch") continue;
    const key = `${step.kind.repo}\n${step.kind.branch}\n${step.kind.sha}`;
    const known = watches.get(key);
    watches.set(key, { latest: step.kind, sinceMs: Math.min(known?.sinceMs ?? step.atMs, step.atMs), atMs: step.atMs });
  }
  return [...watches.values()]
    .filter(({ latest, atMs }) => latest.result === undefined || nowMs - atMs < CI_LINGER_MS)
    .map(({ latest, sinceMs, atMs }) => ({
      repo: latest.repo,
      branch: latest.branch,
      sha: latest.sha,
      waiting: latest.waiting,
      runs: latest.runs,
      sinceMs,
      result: latest.result ?? null,
      endedAtMs: latest.result === undefined ? null : atMs,
    }))
    .toSorted((a, b) => a.sinceMs - b.sinceMs);
}

/** Run conclusions that count as failed, as the daemon counts them (ci.rs `FAILED_CONCLUSIONS`). */
const FAILED_CONCLUSIONS: readonly string[] = ["failure", "cancelled", "timed_out", "startup_failure", "action_required"];

/** A run is done only once GitHub marks it completed: its jobs can all be done before that. */
function runDone(run: CiRun): boolean {
  return run.status === "completed";
}

function runFailed(run: CiRun): boolean {
  return runDone(run) && FAILED_CONCLUSIONS.includes(run.conclusion);
}

/** A run's words inside the open pill: its jobs while it runs, then how it ended. */
export function ciRunWords(run: CiRun): string {
  if (runDone(run)) return runFailed(run) ? "Failed" : run.conclusion === "skipped" ? "Skipped" : "Passed";
  if (run.jobsTotal === 0) return run.status === "queued" ? "Queued" : "Starting";
  return run.jobsDone < run.jobsTotal ? `${run.jobsDone} of ${run.jobsTotal} jobs done` : "Finishing up";
}

/**
 * Where the pill stands, which picks its icon: still `checking` (a spinner), `passed`, `failed`,
 * or `ended` with nothing to pass or fail (no runs, or the watch stopped).
 */
export type CiState = "checking" | "passed" | "failed" | "ended";

/**
 * The CI pill's state and words, every watch in one. Any failed run fails it at once: "CI failed ·
 * test", naming the workflow (or how many). Otherwise "Waiting for CI to start…" until a run
 * starts, then "Checking CI · 2 of 5 jobs done", naming how many pushes when there are several.
 * Once every run passed, the daemon still waits a while for a late workflow: "CI passed ·
 * checking for more runs…". Then the result alone, while it lingers.
 */
export function ciPill(watches: readonly CiWatchView[]): { state: CiState; words: string } {
  const failed = [...new Set(watches.flatMap((watch) => watch.runs.filter(runFailed).map((run) => run.workflow)))];
  if (failed.length > 0) return { state: "failed", words: `CI failed · ${failed.length === 1 ? failed[0]! : `${failed.length} workflows`}` };
  if (watches.some((watch) => watch.result === "failed")) return { state: "failed", words: "CI failed" };
  const live = watches.filter((watch) => watch.result === null);
  const runs = live.flatMap((watch) => watch.runs);
  if (live.length > 0 && (live.some((watch) => watch.runs.length === 0) || !runs.every(runDone))) {
    const pushes = live.length > 1 ? ` for ${live.length} pushes` : "";
    if (runs.every(runDone)) return { state: "checking", words: `Waiting for CI to start${pushes}…` };
    const done = runs.reduce((sum, run) => sum + (runDone(run) ? run.jobsTotal : run.jobsDone), 0);
    const total = runs.reduce((sum, run) => sum + run.jobsTotal, 0);
    const words = total === 0 ? `Checking CI${pushes}…` : done < total ? `Checking CI${pushes} · ${done} of ${total} jobs done` : `Checking CI${pushes} · finishing up…`;
    return { state: "checking", words };
  }
  if (live.length > 0) return { state: "passed", words: "CI passed · checking for more runs…" };
  if (watches.some((watch) => watch.result === "passed")) return { state: "passed", words: "CI passed" };
  if (watches.some((watch) => watch.result === "stopped")) return { state: "ended", words: "Stopped watching CI" };
  return { state: "ended", words: "No CI runs" };
}

/** The CI pill's words: see `ciPill`. */
export function ciWords(watches: readonly CiWatchView[]): string {
  return ciPill(watches).words;
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
  /** A merge card asked since the user's yes waits for their answer: the merge waits on it. */
  asking: boolean;
  sinceMs: number;
};

/**
 * The held merge (SESSION-FIXES-PLAN.md Q3), while it is held: its latest `mergeHeld` row with
 * no merge, Cancel or stop since. It waits for the write tasks that ran when it was held, until
 * each is final (the daemon's `is_final`), and then for the reviews of their landings. When the
 * thread asks again on a merge card (the user wrote since their yes), it waits for that answer.
 */
export function heldMerge(
  steps: readonly OrchestratorStep[],
  tasks: Readonly<Record<string, Task>>,
  reviews: readonly ReviewRun[],
  questions: readonly Question[] = [],
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
  const consentedAtMs = held.kind.consentedAtMs;
  const asking = questions.some(
    (question) => question.kind.type === "merge" && questionOpen(question) && question.createdAtMs >= consentedAtMs,
  );
  return { branch: held.kind.branch, base: held.kind.base, waiting, reviewing, asking, sinceMs: held.sinceMs };
}

/**
 * "Merging after 2 workers finish", then "Merging after the review", then "Merging…"; while a
 * merge card waits, "Waiting for your yes to merge".
 */
export function mergeWords(merge: Pick<HeldMergeView, "waiting" | "reviewing" | "asking">): string {
  if (merge.asking) return "Waiting for your yes to merge";
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
