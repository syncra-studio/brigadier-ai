import { questionOpen } from "@/app/conversation/cards/questionRound";
import type { CiRun, OrchestratorStep, OrchestratorStepKind, Question, Task, TaskState } from "@/ipc/generated";

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

/**
 * Every CI watch (SESSION-FIXES-PLAN.md Q1), oldest first. A watch files a new row each time its
 * runs change, one watch per repository, branch and sha; its latest row says where it stands.
 * One with a result is over and stays, so the Job card keeps its outcome until the user's next
 * message (J4).
 */
export function ciWatches(steps: readonly OrchestratorStep[]): CiWatchView[] {
  const watches = new Map<string, { latest: CiWatchKind; sinceMs: number; atMs: number }>();
  for (const step of steps) {
    if (step.kind.type !== "ciWatch") continue;
    const key = `${step.kind.repo}\n${step.kind.branch}\n${step.kind.sha}`;
    const known = watches.get(key);
    watches.set(key, { latest: step.kind, sinceMs: Math.min(known?.sinceMs ?? step.atMs, step.atMs), atMs: step.atMs });
  }
  return [...watches.values()]
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

export function runFailed(run: CiRun): boolean {
  return runDone(run) && FAILED_CONCLUSIONS.includes(run.conclusion);
}

/** How many of the runs' jobs are done, of all of them: a completed run counts every job done. */
export function ciJobs(runs: readonly CiRun[]): { done: number; total: number } {
  return {
    done: runs.reduce((sum, run) => sum + (runDone(run) ? run.jobsTotal : run.jobsDone), 0),
    total: runs.reduce((sum, run) => sum + run.jobsTotal, 0),
  };
}

/**
 * Where the pill stands, which picks its icon: still `checking` (a spinner), `passed`, `failed`,
 * `cancelled` (a run cancelled, none failed: GitHub's word, never a silent failure), or `ended`
 * with nothing to pass or fail (no runs, or the watch stopped).
 */
export type CiState = "checking" | "passed" | "failed" | "cancelled" | "ended";

/** The workflows with a run that `ended` so, each once: "test", or "2 workflows"; null for none. */
function named(watches: readonly CiWatchView[], ended: (run: CiRun) => boolean): string | null {
  const workflows = [...new Set(watches.flatMap((watch) => watch.runs.filter(ended).map((run) => run.workflow)))];
  return workflows.length === 0 ? null : workflows.length === 1 ? workflows[0]! : `${workflows.length} workflows`;
}

/**
 * The CI pill's state and words, every watch in one. Any failed run fails it at once: "CI failed ·
 * test", naming the workflow (or how many). A cancelled run, with none failed, says so: "CI
 * cancelled · test" (the daemon counts it failed; its mark and words are the cancel's). Otherwise "Waiting for CI to start…" until a run
 * starts, then "Checking CI · 2 of 5 jobs done", naming how many pushes when there are several.
 * Once every run passed, the daemon still waits a while for a late workflow: "CI passed ·
 * checking for more runs…". Then the result alone, while it lingers.
 */
export function ciPill(watches: readonly CiWatchView[]): { state: CiState; words: string } {
  const failed = named(watches, (run) => runFailed(run) && run.conclusion !== "cancelled");
  if (failed) return { state: "failed", words: `CI failed · ${failed}` };
  const cancelled = named(watches, (run) => runFailed(run) && run.conclusion === "cancelled");
  if (cancelled) return { state: "cancelled", words: `CI cancelled · ${cancelled}` };
  if (watches.some((watch) => watch.result === "failed")) return { state: "failed", words: "CI failed" };
  const live = watches.filter((watch) => watch.result === null);
  const runs = live.flatMap((watch) => watch.runs);
  if (live.length > 0 && (live.some((watch) => watch.runs.length === 0) || !runs.every(runDone))) {
    const pushes = live.length > 1 ? ` for ${live.length} pushes` : "";
    if (runs.every(runDone)) return { state: "checking", words: `Waiting for CI to start${pushes}…` };
    const { done, total } = ciJobs(runs);
    const words = total === 0 ? `Checking CI${pushes}…` : done < total ? `Checking CI${pushes} · ${done} of ${total} jobs done` : `Checking CI${pushes} · finishing up…`;
    return { state: "checking", words };
  }
  if (live.length > 0) return { state: "passed", words: "CI passed · checking for more runs…" };
  if (watches.some((watch) => watch.result === "passed")) return { state: "passed", words: "CI passed" };
  if (watches.some((watch) => watch.result === "stopped")) return { state: "ended", words: "Stopped watching CI" };
  return { state: "ended", words: "No CI runs" };
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
  /** One of them is a merge task bringing the base in (the merge met conflicts). */
  mergingIn: boolean;
  /** A merge card asked since the user's yes waits for their answer: the merge waits on it. */
  asking: boolean;
  sinceMs: number;
};

const WRITES: readonly Task["kind"][] = ["implement", "merge"];

/**
 * The held merge (SESSION-FIXES-PLAN.md Q3), while it is held: its latest `mergeHeld` row with
 * no merge, Cancel or stop since. It waits for the write tasks that ran when it was held, or
 * started since (a merge task after conflicts, #223), until each is final (the daemon's
 * `is_final`); reviews and the full checks don't hold it (#241, #242). When the thread asks
 * again on a merge card (the user wrote since their yes), it waits for that answer.
 */
export function heldMerge(
  steps: readonly OrchestratorStep[],
  tasks: Readonly<Record<string, Task>>,
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
  const sinceMs = held.sinceMs;
  const writers = Object.values(tasks).filter(
    (task) =>
      !FINAL.includes(task.state) &&
      (held.kind.waiting.includes(task.id) || (WRITES.includes(task.kind) && task.createdAtMs >= sinceMs)),
  );
  const waiting = writers.length;
  // A merge task with no subject brings the base in; one with a subject merges into a worker's.
  const mergingIn = writers.some((task) => task.kind === "merge" && task.subject === null);
  const consentedAtMs = held.kind.consentedAtMs;
  const asking = questions.some(
    (question) => question.kind.type === "merge" && questionOpen(question) && question.createdAtMs >= consentedAtMs,
  );
  return { branch: held.kind.branch, base: held.kind.base, waiting, mergingIn, asking, sinceMs };
}

/**
 * "Merging after main is merged in" while a merge task resolves conflicts, "Merging after 2
 * workers finish", then "Merging…"; while a merge card waits, "Waiting for your yes to merge".
 */
export function mergeWords(merge: Pick<HeldMergeView, "base" | "waiting" | "mergingIn" | "asking">): string {
  if (merge.asking) return "Waiting for your yes to merge";
  if (merge.mergingIn) return `Merging after ${merge.base} is merged in`;
  if (merge.waiting > 0) return `Merging after ${merge.waiting} ${merge.waiting === 1 ? "worker finishes" : "workers finish"}`;
  return "Merging…";
}
