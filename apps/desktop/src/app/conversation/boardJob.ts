import { useMemo } from "react";

import { isRunRequest } from "@/app/conversation/blocks";
import type { Job, JobStep, JobStepState } from "@/app/conversation/job";
import { contextPlanId, contextPlanIds, latestPlanRequest, planStepStatus, stepStates } from "@/app/conversation/planProgress";
import { batchReviews, type FindingRow, findingRows } from "@/app/conversation/reviewStatus";
import type { IntegratedCheck, MergeRecord, OrchestratorStep, OrchestratorStepKind, Plan, ReviewRun, Task, TaskState } from "@/ipc/generated";
import { formatDuration } from "@/lib/format";
import { type Board, useBoard } from "@/state/board";
import { useApp } from "@/state/store";

/**
 * The session's job read off its board (Q88): what its change went through since the last merge
 * (Building, Reviewing, Fixing, Checks, CI, Merging), or the phase at work of its plan. Each
 * step is one the board holds; while one is at work, the steps the session still goes through
 * follow it as pending (a review when reviews are on, the checks, the merge when it merges on
 * its own). Every earlier merge closes a job of its own, kept for the job's tab.
 */

export type JobStepId = "build" | "review" | "fix" | "checks" | "ci" | "merge";

/** The board's facts about one step, for the job's tab: a review's findings, a merge's record. */
export type JobFacts = {
  findings: FindingRow[];
  /** The merge that closed the job, with what was checked of it. */
  merge: { commit: string | null; base: string; pr: boolean; record: MergeRecord | null } | null;
};

export type BoardJob = Job & { atMs: number; facts: JobFacts };

/** What a job is read from: the board's parts and the two settings that add steps to come. */
export type JobInput = Pick<Board, "tasks" | "reviews" | "integratedChecks" | "orchestratorSteps" | "plans" | "requests" | "overnight"> & {
  reviewWork: boolean;
  autoMerge: boolean;
};

/** The work kinds that build: a change, or an app driven to a state. */
const BUILDS = new Set<Task["kind"]>(["implement", "operate"]);

/** A worker that builds: not one that only looks at the running app and changes nothing (#423). */
const builds = (task: Task) => BUILDS.has(task.kind) && !task.liveLook;

/** A task's part in Building: queued, at work, or past it (it reported, landed or ended). */
function buildState(state: TaskState): JobStepState | "stopped" {
  switch (state) {
    case "queued":
      return "pending";
    case "starting":
    case "running":
    case "blocked":
    case "paused":
      return "active";
    case "failed":
      return "failed";
    case "rejected":
    case "stopped":
      return "stopped";
    default:
      return "done";
  }
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

function buildStep(tasks: readonly Task[], phase: JobStepState | null): JobStep | null {
  if (tasks.length === 0 && phase === null) return null;
  const states = tasks.map((task) => buildState(task.state));
  const done = states.filter((state) => state === "done").length;
  const active = states.includes("active") || (states.includes("pending") && done > 0);
  const state: JobStepState =
    phase === "active" || active
      ? "active"
      : states.includes("pending") || phase === "pending"
        ? "pending"
        : phase === "failed" || (states.includes("failed") && done === 0)
          ? "failed"
          : "done";
  const failed = tasks.find((task) => task.state === "failed");
  return {
    id: "build",
    name: "Building",
    state,
    count: state === "active" && tasks.length > 1 ? `${done} of ${tasks.length}` : undefined,
    failedLabel: "Building failed",
    startedAtMs: tasks.length > 0 ? Math.min(...tasks.map((task) => task.createdAtMs)) : null,
    endedAtMs: state === "done" && tasks.length > 0 ? Math.max(...tasks.map((task) => task.updatedAtMs)) : null,
    summary:
      state === "failed" && failed
        ? failed.title
        : state === "done" && done > 0
          ? `${done === 1 ? "A worker" : `${done} workers`} built it`
          : undefined,
  };
}

/** How many findings the reviews hold (one stored before findings were items counts its own). */
function findingCount(reviews: readonly ReviewRun[]): number {
  return reviews.reduce(
    (sum, review) => sum + (review.items.length > 0 ? review.items.length : review.state.type === "findings" ? review.state.count : 0),
    0,
  );
}

function reviewStep(reviews: readonly ReviewRun[]): JobStep | null {
  if (reviews.length === 0) return null;
  const running = reviews.filter((review) => review.state.type === "running").length;
  const found = findingCount(reviews);
  const ended = reviews.flatMap((review) => (review.endedAtMs === null ? [] : [review.endedAtMs]));
  return {
    id: "review",
    name: "Reviewing",
    state: running > 0 ? "active" : "done",
    count: running > 0 && reviews.length > 1 ? `${reviews.length - running} of ${reviews.length}` : undefined,
    startedAtMs: Math.min(...reviews.map((review) => review.startedAtMs)),
    endedAtMs: running === 0 && ended.length > 0 ? Math.max(...ended) : null,
    summary:
      running > 0
        ? undefined
        : reviews.every((review) => review.state.type === "failed")
          ? "Review couldn’t run"
          : found === 0
            ? "Review found nothing to fix"
            : `Review found ${plural(found, "issue")}`,
  };
}

/** What the thread did about the findings: "2 fixed, 1 dismissed", "1 left open". */
function settledWords(rows: readonly FindingRow[]): string | undefined {
  const fixed = rows.filter((row) => row.status === "Fixed" || row.status === "Unverified").length;
  const dismissed = rows.filter((row) => row.status === "Dismissed").length;
  const open = rows.filter((row) => row.status === "Open").length;
  const words = [fixed > 0 && `${fixed} fixed`, dismissed > 0 && `${dismissed} dismissed`, open > 0 && `${open} left open`];
  return words.filter(Boolean).join(", ") || undefined;
}

const checkNumber = (check: IntegratedCheck) => Number(check.id.replace(/^check-/, "")) || 0;

function checksStep(checks: readonly IntegratedCheck[]): JobStep | null {
  const latest = checks.toSorted((a, b) => checkNumber(b) - checkNumber(a))[0];
  if (!latest) return null;
  const startedAtMs = latest.startedAtMs ?? latest.dueAtMs;
  const took = latest.endedAtMs !== null && latest.startedAtMs !== null ? ` in ${formatDuration(latest.endedAtMs - latest.startedAtMs)}` : "";
  const step = { id: "checks", name: "Checks", startedAtMs, endedAtMs: latest.endedAtMs };
  switch (latest.state.type) {
    case "due":
      return { ...step, state: "active" };
    case "running":
      return { ...step, state: "active", summary: latest.state.command ?? undefined };
    case "passed":
      return { ...step, state: "done", summary: latest.state.commands.length > 0 ? `Checks passed${took}` : "Nothing to check" };
    case "failed":
      return { ...step, state: "failed", failedLabel: "Checks failed", summary: `${latest.state.command}: ${latest.state.status}` };
    case "interrupted":
      return { ...step, state: "failed", failedLabel: "Checks couldn’t run", summary: latest.state.reason };
  }
}

type CiWatch = Extract<OrchestratorStepKind, { type: "ciWatch" }>;

function ciStep(watch: { kind: CiWatch; atMs: number } | undefined): JobStep | null {
  if (!watch) return null;
  const { kind } = watch;
  const step = { id: "ci", name: "CI", startedAtMs: watch.atMs };
  if (kind.result === undefined) {
    const done = kind.runs.reduce((sum, run) => sum + run.jobsDone, 0);
    const total = kind.runs.reduce((sum, run) => sum + run.jobsTotal, 0);
    return { ...step, state: "active", count: total > 0 ? `${done} of ${total}` : undefined, summary: kind.waiting ? "Waiting for CI to start" : undefined };
  }
  if (kind.result === "failed") return { ...step, state: "failed", failedLabel: "CI failed", summary: kind.runs.filter((run) => run.conclusion === "failure").map((run) => run.workflow).join(", ") || undefined };
  return { ...step, state: "done", summary: kind.result === "passed" ? "CI passed" : kind.result === "no runs" ? "No CI runs" : "CI watch stopped" };
}

type Merged = Extract<OrchestratorStepKind, { type: "merged" }>;

/** The merge step: the one that closed the job, else one asked for and not yet over. */
function mergeStep(steps: readonly OrchestratorStep[], merged: { kind: Merged; atMs: number } | null): JobStep | null {
  if (merged) {
    const commit = merged.kind.mergeCommit?.slice(0, 7);
    return {
      id: "merge",
      name: "Merging",
      state: "done",
      startedAtMs: merged.atMs,
      summary: `Merged ${commit ? `${commit} ` : ""}into ${merged.kind.base}${merged.kind.pr === undefined ? "" : ` (#${merged.kind.pr})`}`,
    };
  }
  const last = steps.findLast((step) => ["mergeRequested", "mergeCancelled", "mergeStopped"].includes(step.kind.type));
  if (last?.kind.type === "mergeRequested") return { id: "merge", name: "Merging", state: "active", startedAtMs: last.atMs, summary: `Into ${last.kind.base}` };
  if (last?.kind.type === "mergeStopped") return { id: "merge", name: "Merging", state: "failed", failedLabel: "Merge stopped", startedAtMs: last.atMs, summary: last.kind.reason };
  return null;
}

const ORDER: readonly JobStepId[] = ["build", "review", "fix", "checks", "ci", "merge"];
const NAMES: Record<JobStepId, string> = { build: "Building", review: "Reviewing", fix: "Fixing", checks: "Checks", ci: "CI", merge: "Merging" };

/** One job: the board's steps between `since` and `until`, closed by `merged` when one did. */
function windowJob(
  input: JobInput,
  since: number,
  until: number,
  merged: { kind: Merged; atMs: number } | null,
  phase: { tasks: readonly Task[]; state: JobStepState } | null,
): BoardJob | null {
  const inWindow = (atMs: number) => atMs >= since && atMs < until;
  const tasks = phase?.tasks ?? Object.values(input.tasks).filter((task) => builds(task) && !isRunRequest(task.requestId) && inWindow(task.createdAtMs));
  const reviews = batchReviews(Object.values(input.reviews), since, until).filter((review) => !isRunRequest(review.requestId));
  const checks = Object.values(input.integratedChecks).filter((check) => inWindow(check.dueAtMs) && !check.requests.some(isRunRequest));
  const steps = input.orchestratorSteps.filter((step) => inWindow(step.atMs) && !isRunRequest(step.requestId));
  const watch = steps.flatMap((step) => (step.kind.type === "ciWatch" ? [{ kind: step.kind, atMs: step.atMs }] : [])).at(-1);
  const findings = findingRows(reviews);

  // A build that starts once the reviews or checks have begun is a fix: the job never goes back
  // to Building (#423).
  const trouble = Math.min(
    ...reviews.map((review) => review.startedAtMs),
    ...checks.flatMap((check) => (check.startedAtMs !== null ? [check.startedAtMs] : [])),
  );
  const lead = phase?.tasks[0];
  const fixes = tasks.filter((task) => task !== lead && task.createdAtMs > trouble);
  const build = buildStep(tasks.filter((task) => !fixes.includes(task)), phase?.state ?? null);
  const fixing = fixes.some((task) => buildState(task.state) === "active" || buildState(task.state) === "pending");
  const review = reviewStep(reviews);
  const check = checksStep(checks);
  const ci = ciStep(watch);
  const merge = mergeStep(steps, merged);
  const unrecorded = findings.filter((row) => row.status === "New").length;
  const checkFailed = check?.state === "failed";
  const live = fixing || [build, review, check, ci, merge].some((step) => step?.state === "active");
  const fix: JobStep | null =
    findingCount(reviews) > 0 || checkFailed || fixes.length > 0
      ? {
          id: "fix",
          name: "Fixing",
          // While a review still runs, its findings wait for it.
          state: fixing || ((unrecorded > 0 || checkFailed) && live && review?.state !== "active") ? "active" : unrecorded > 0 || checkFailed ? "pending" : "done",
          count: unrecorded > 0 ? `${plural(unrecorded, "finding")} to go` : undefined,
          summary: unrecorded > 0 ? undefined : settledWords(findings),
        }
      : null;
  const real: Partial<Record<JobStepId, JobStep>> = Object.fromEntries(
    ([["build", build], ["review", review], ["fix", fix], ["checks", check], ["ci", ci], ["merge", merge]] as const).filter(([, step]) => step !== null),
  );
  // A job is the board's: a review, the checks or a worker's build. A question has none.
  if (!real.build && !real.review && !real.checks && !real.merge) return null;
  // While a step is at work, what the session still goes through comes after it.
  const furthest = Math.max(...ORDER.map((id, index) => (real[id] ? index : -1)));
  const expected = new Set<JobStepId>([...(input.reviewWork ? (["review"] as const) : []), "checks", ...(input.autoMerge ? (["merge"] as const) : [])]);
  const listed: JobStep[] = ORDER.flatMap((id, index) => {
    const step = real[id];
    if (step) return [step];
    return live && !merged && index > furthest && expected.has(id) ? [{ id, name: NAMES[id], state: "pending" as const }] : [];
  });
  const result = onwards(listed);
  const atMs = Math.min(...result.flatMap((step) => (step.startedAtMs ? [step.startedAtMs] : [])), merged?.atMs ?? Number.POSITIVE_INFINITY);
  return {
    steps: result,
    atMs: Number.isFinite(atMs) ? atMs : since,
    facts: {
      findings,
      merge: merged ? { commit: merged.kind.mergeCommit ?? null, base: merged.kind.base, pr: merged.kind.pr !== undefined, record: merged.kind.record ?? null } : null,
    },
  };
}

/**
 * One step at work at a time, and never back to an earlier one (#414): the furthest step at work
 * is the job's, and every step before it that was still at work or to come counts as passed. A
 * failed step stays failed.
 */
export function onwards(steps: readonly JobStep[]): JobStep[] {
  const at = steps.findLastIndex((step) => step.state === "active");
  return steps.map((step, index) => (index < at && (step.state === "active" || step.state === "pending") ? { ...step, state: "done", count: undefined } : step));
}

/** The phase at work of the session's current plan of several phases, with its window. */
function currentPhase(input: JobInput): { plan: Plan; index: number; since: number; until: number; state: JobStepState; tasks: Task[] } | null {
  const runPlans = new Set(Object.values(input.overnight).flatMap((run) => (run.planId ? [run.planId] : [])));
  const ids = contextPlanIds(input.plans).filter((id) => !runPlans.has(id));
  const id = contextPlanId(input.plans, ids, latestPlanRequest(input.requests));
  const plan = id ? input.plans[id] : undefined;
  if (!plan || plan.state.type !== "approved" || plan.steps.length < 2) return null;
  const request = plan.requestId ? input.requests[plan.requestId]?.state.type : undefined;
  const states = stepStates(plan, plan.steps.map((step) => (step.taskId ? input.tasks[step.taskId]?.state : undefined)), request);
  const statuses = states.map(planStepStatus);
  let index = statuses.findIndex((status) => status === "active" || status === "failed");
  if (index === -1) index = statuses.findIndex((status) => status === "pending");
  if (index === -1) return null;
  const startOf = (i: number) => {
    const step = plan.steps[i];
    const task = step?.taskId ? input.tasks[step.taskId] : undefined;
    return step?.startedAtMs ?? task?.createdAtMs ?? null;
  };
  const since = startOf(index) ?? (index > 0 ? (startOf(index - 1) ?? plan.decidedAtMs ?? plan.createdAtMs) : (plan.decidedAtMs ?? plan.createdAtMs));
  const task = plan.steps[index]?.taskId ? input.tasks[plan.steps[index]!.taskId!] : undefined;
  const status = statuses[index]!;
  // The phase's lead builds until it reports; past that, its reviews and checks are at work.
  const state: JobStepState = task ? (buildState(task.state) === "stopped" ? "done" : (buildState(task.state) as JobStepState)) : status === "stopped" ? "done" : status;
  // The phase's other builds (its fixes): those since it started that no other phase leads.
  const leads = new Set(plan.steps.flatMap((step) => (step.taskId ? [step.taskId] : [])));
  const others = Object.values(input.tasks).filter((other) => !leads.has(other.id) && builds(other) && other.createdAtMs >= since);
  return { plan, index, since, until: Number.POSITIVE_INFINITY, state, tasks: task ? [task, ...others] : others };
}

/**
 * The session's job at work (`current`, null when there is none or it is all done) and the jobs
 * each earlier merge closed, newest first.
 */
export function sessionJobs(input: JobInput): { current: BoardJob | null; past: BoardJob[] } {
  const merges = input.orchestratorSteps
    .flatMap((step) => (step.kind.type === "merged" && !isRunRequest(step.requestId) ? [{ kind: step.kind, atMs: step.atMs }] : []))
    .toSorted((a, b) => a.atMs - b.atMs);
  const past = merges
    .map((merge, index) => windowJob(input, (merges[index - 1]?.atMs ?? -1) + 1, merge.atMs + 1, merge, null))
    .filter((job): job is BoardJob => job !== null)
    .toReversed();
  const lastMerge = (merges.at(-1)?.atMs ?? -1) + 1;
  const phase = currentPhase(input);
  const current = phase
    ? windowJob(input, Math.max(phase.since, lastMerge), phase.until, null, { tasks: phase.tasks, state: phase.state })
    : windowJob(input, lastMerge, Number.POSITIVE_INFINITY, null, null);
  const job = current && phase ? { ...current, phase: { number: phase.index + 1, of: phase.plan.steps.length, title: phase.plan.steps[phase.index]!.title } } : current;
  // A plan's job stays while phases are left; a change's leaves once all of it is done.
  const shown = job && (job.steps.some((step) => step.state !== "done") || (phase && phase.index < phase.plan.steps.length));
  return { current: shown ? job : null, past };
}

/** The board's session's jobs, live; null while the board is another conversation's. */
export function useSessionJobs(conversationId: string): { current: BoardJob | null; past: BoardJob[] } | null {
  const reviewWork = useApp((s) => s.settings.reviewWork);
  const autoMerge = useApp((s) => s.settings.autoMerge);
  const json = useBoard((s) => {
    const board = s.board;
    if (!board || board.conversationId !== conversationId) return null;
    return JSON.stringify(sessionJobs({ ...board, reviewWork, autoMerge }));
  });
  return useMemo(() => (json ? (JSON.parse(json) as ReturnType<typeof sessionJobs>) : null), [json]);
}
