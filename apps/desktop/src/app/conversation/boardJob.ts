import { useMemo } from "react";

import { isRunRequest } from "@/app/conversation/blocks";
import type { Job, JobStep, JobStepState } from "@/app/conversation/job";
import type { JobStage } from "@/components/conversation/elements/job-progress";
import { contextPlanId, contextPlanIds, latestPlanRequest, planStepStatus, stepStates } from "@/app/conversation/planProgress";
import type { OrchestratorStep, OrchestratorStepKind, Plan, Task, TaskState } from "@/ipc/generated";
import { type Board, useBoard } from "@/state/board";

/**
 * The session's job read off its board (Q88): what its change went through since the last merge
 * (Building, CI, Merging), or the phase at work of its plan. Each step is one the board holds.
 * Every earlier merge closes a job of its own, kept for the job's tab.
 */

export type JobStepId = "build" | "ci" | "merge";

export type BoardJob = Job & {
  atMs: number;
  /** The newest work start, even when several workers share one step's earliest start. */
  latestStartedAtMs: number;
};

/** What a job is read from: the board's parts. */
export type JobInput = Pick<Board, "tasks" | "orchestratorSteps" | "plans" | "requests" | "overnight">;

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

const ORDER: readonly JobStepId[] = ["build", "ci", "merge"];

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
  const steps = input.orchestratorSteps.filter((step) => inWindow(step.atMs) && !isRunRequest(step.requestId));
  const watch = steps.flatMap((step) => (step.kind.type === "ciWatch" ? [{ kind: step.kind, atMs: step.atMs }] : [])).at(-1);
  const build = buildStep(tasks, phase?.state ?? null);
  const ci = ciStep(watch);
  const merge = mergeStep(steps, merged);
  const real: Partial<Record<JobStepId, JobStep>> = Object.fromEntries(
    ([["build", build], ["ci", ci], ["merge", merge]] as const).filter(([, step]) => step !== null),
  );
  // A job is the board's: a worker's build or a merge. A question has none.
  if (!real.build && !real.merge) return null;
  const result = onwards(ORDER.flatMap((id) => (real[id] ? [real[id]] : [])));
  const atMs = Math.min(...result.flatMap((step) => (step.startedAtMs ? [step.startedAtMs] : [])), merged?.atMs ?? Number.POSITIVE_INFINITY);
  return {
    steps: result,
    atMs: Number.isFinite(atMs) ? atMs : since,
    latestStartedAtMs: Math.max(
      Number.isFinite(atMs) ? atMs : since,
      ...tasks.map((task) => task.createdAtMs),
      ...result.flatMap((step) => (step.startedAtMs == null ? [] : [step.startedAtMs])),
    ),
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
function currentPhase(
  input: Pick<JobInput, "tasks" | "plans" | "requests" | "overnight">,
): { plan: Plan; index: number; since: number; until: number; state: JobStepState; tasks: Task[]; statuses: ReturnType<typeof planStepStatus>[] } | null {
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
  // The phase's lead builds until it reports.
  const state: JobStepState = task ? (buildState(task.state) === "stopped" ? "done" : (buildState(task.state) as JobStepState)) : status === "stopped" ? "done" : status;
  // The phase's other builds (its fixes): those since it started that no other phase leads.
  const leads = new Set(plan.steps.flatMap((step) => (step.taskId ? [step.taskId] : [])));
  const others = Object.values(input.tasks).filter((other) => !leads.has(other.id) && builds(other) && other.createdAtMs >= since);
  return { plan, index, since, until: Number.POSITIVE_INFINITY, state, tasks: task ? [task, ...others] : others, statuses };
}

/** A plan the thread wrote, of several steps: the Job card's phase bar (mockup 06). */
export type PlanBar = { stages: JobStage[]; index: number; title: string; words: string };

/**
 * The phase bar of the session's approved plan of two steps or more while steps are left: one
 * segment per step, named as the plan names it. Nothing else gets a bar: the Job card says what
 * is happening in words.
 */
export function planBar(input: Pick<JobInput, "tasks" | "plans" | "requests" | "overnight">): PlanBar | null {
  const phase = currentPhase(input);
  if (!phase) return null;
  const stages = phase.plan.steps.map((step, index) => {
    const status = phase.statuses[index];
    return { name: step.title, weight: 1, state: status === "stopped" ? "done" : (status ?? "pending") } satisfies JobStage;
  });
  return { stages, index: phase.index, title: phase.plan.steps[phase.index]!.title, words: `Step ${phase.index + 1} of ${stages.length}` };
}

/** The board's session's plan bar, live; null while there is none or the board is another conversation's. */
export function usePlanBar(conversationId: string): PlanBar | null {
  const json = useBoard((s) => {
    const board = s.board;
    if (!board || board.conversationId !== conversationId) return null;
    const bar = planBar(board);
    return bar ? JSON.stringify(bar) : null;
  });
  return useMemo(() => (json ? (JSON.parse(json) as PlanBar) : null), [json]);
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
  const json = useBoard((s) => {
    const board = s.board;
    if (!board || board.conversationId !== conversationId) return null;
    return JSON.stringify(sessionJobs(board));
  });
  return useMemo(() => (json ? (JSON.parse(json) as ReturnType<typeof sessionJobs>) : null), [json]);
}
