import { Check, X } from "@openai/apps-sdk-ui/components/Icon";
import { type FC, type ReactNode, useContext, useMemo } from "react";
import { useShallow } from "zustand/react/shallow";

import { isRunRequest } from "@/app/conversation/blocks";
import { ComposerTargetContext } from "@/app/conversation/composerTarget";
import { useRunDiff } from "@/app/conversation/overnightAdapter";
import { type RunPill, runPill } from "@/app/conversation/phaseView";
import { mono } from "@/components/assistant-ui/elements/surfaces";
import { Spinner } from "@/components/glyphs/spinner";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { DiffStat, FileStat, Plan, Task, TaskState } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { type Board, useBoard } from "@/state/board";

/**
 * What a request changed so far: the stats of its workers' landed commits, merged by file.
 * It keeps counting landings after the orchestrator's reply ended (the turn's diff reads it
 * too), so it is tied to the request, not to the turn.
 */
export function useRequestDiff(requestId: string | null): DiffStat | null {
  const stats = useBoard(
    useShallow((s) =>
      Object.values(s.board?.tasks ?? {})
        .filter((task) => requestId !== null && task.requestId === requestId && task.landed !== null)
        .toSorted((a, b) => a.number - b.number)
        .flatMap((task) => (task.candidate ? [task.candidate.diffStat] : [])),
    ),
  );
  if (stats.length === 0) return null;
  const files = new Map<string, FileStat>();
  for (const file of stats.flatMap((stat) => stat.files)) {
    const seen = files.get(file.path);
    files.set(
      file.path,
      seen
        ? {
            ...seen,
            insertions: seen.insertions + file.insertions,
            deletions: seen.deletions + file.deletions,
            binary: seen.binary || file.binary,
          }
        : file,
    );
  }
  return {
    files: [...files.values()],
    insertions: stats.reduce((sum, stat) => sum + stat.insertions, 0),
    deletions: stats.reduce((sum, stat) => sum + stat.deletions, 0),
  };
}

/** The request still at work (its orchestrator turn or its workers), if any. */
function runningRequest(board: Board | null, conversationId: string): string | null {
  if (!board || board.conversationId !== conversationId) return null;
  return (
    Object.values(board.requests)
      .filter((request) => request.state.type === "working" && request.steeredInto === null)
      .toSorted((a, b) => b.startedAtMs - a.startedAtMs)[0]?.id ?? null
  );
}

/** Something waits for the user's decision: the capsule makes room for its card. */
function anyPending(board: Board | null, conversationId: string): boolean {
  if (!board || board.conversationId !== conversationId) return false;
  return (
    Object.values(board.approvals).some((approval) => approval.state.type === "pending") ||
    Object.values(board.questions).some((question) => question.answeredAtMs === null) ||
    Object.values(board.plans).some((plan) => plan.state.type === "proposed")
  );
}

type StepStatus = "pending" | "active" | "done" | "failed";

function stepStatus(state: TaskState | undefined): StepStatus {
  switch (state) {
    case undefined:
    case "queued":
      return "pending";
    case "landed":
    case "done":
      return "done";
    case "failed":
    case "rejected":
    case "stopped":
      return "failed";
    default:
      return "active";
  }
}

/** Where the session's active run is, if one is at work. */
function useRunPill(conversationId: string): RunPill | null {
  const runs = useBoard((s) => (s.board?.conversationId === conversationId ? s.board.overnight : null));
  const plans = useBoard((s) => s.board?.plans);
  const tasks = useBoard((s) => s.board?.tasks);
  return useMemo(() => (runs && plans && tasks ? runPill(runs, plans, tasks) : null), [runs, plans, tasks]);
}

/** The request's plan being carried out, with each step's status. */
function useRunningPlan(requestId: string | null): { plan: Plan; steps: StepStatus[] } | null {
  const plan = useBoard((s) =>
    Object.values(s.board?.plans ?? {})
      .filter(
        (candidate) =>
          requestId !== null &&
          candidate.requestId === requestId &&
          (candidate.state.type === "approved" || candidate.state.type === "inReview"),
      )
      .toSorted((a, b) => b.createdAtMs - a.createdAtMs)[0],
  );
  const states = useBoard(
    useShallow((s) =>
      (plan?.steps ?? []).map((step) =>
        step.taskId ? (s.board?.tasks[step.taskId] as Task | undefined)?.state : undefined,
      ),
    ),
  );
  if (!plan || plan.steps.length === 0) return null;
  return { plan, steps: states.map(stepStatus) };
}

const Pill: FC<{ tip: ReactNode; children: ReactNode }> = ({ tip, children }) => (
  <Tooltip>
    <TooltipTrigger asChild>
      <span className="flex items-center gap-1.5">
        {children}
      </span>
    </TooltipTrigger>
    <TooltipContent side="top" className="flex-col items-stretch gap-1 py-1.5">
      {tip}
    </TooltipContent>
  </Tooltip>
);

/** A 12 pt progress donut. */
const Donut: FC<{ done: number; total: number }> = ({ done, total }) => (
  <span aria-hidden className="text-link size-icon-xs shrink-0 rounded-full border border-current p-px">
    <span
      className="block size-full rounded-full transition-[background] duration-200"
      style={{ background: `conic-gradient(currentColor ${(done / total) * 100}%, transparent 0)` }}
    />
  </span>
);

const StepGlyph: FC<{ status: StepStatus }> = ({ status }) => {
  switch (status) {
    case "done":
      return <Check className="text-muted-foreground size-icon-xs" />;
    case "active":
      return <Spinner className="size-icon-xs animate-spin motion-reduce:animate-none" />;
    case "failed":
      return <X className="text-destructive size-icon-xs" />;
    case "pending":
      return <span className="border-foreground/40 size-icon-xs rounded-full border" />;
  }
};

/**
 * The capsule above the composer while a request works: "Step n / m" of its plan and "N files
 * changed +a −d" of what its workers landed, updated on every landing. During an overnight run
 * it shows the run instead ("Phase 2 of 3 · Fix · 2 of 4 steps") and the run branch's diff. It
 * makes room for a pending decision and goes when the request stops working or the run ends.
 */
export const ComposerCapsule: FC = () => {
  const conversationId = useContext(ComposerTargetContext)?.conversation?.id ?? "";
  const pending = useBoard((s) => anyPending(s.board, conversationId));
  const run = useRunPill(conversationId);
  if (pending) return null;
  return run ? <RunCapsule conversationId={conversationId} run={run} /> : <RequestCapsule conversationId={conversationId} />;
};

/** The capsule of an overnight run: its phase, the phase's steps and the run branch's diff. */
const RunCapsule: FC<{ conversationId: string; run: RunPill }> = ({ conversationId, run }) => {
  const diff = useRunDiff(conversationId, run.runId);
  return (
    <Capsule>
      <Pill
        tip={
          run.steps.length > 0 ? (
            <ol className="flex flex-col gap-1">
              {run.steps.map((step, index) => (
                <li key={index} className="flex items-center gap-1.5">
                  <StepGlyph status={step.done ? "done" : step.active ? "active" : "pending"} />
                  <span className={cn(!step.active && "text-muted-foreground")}>{step.title}</span>
                </li>
              ))}
            </ol>
          ) : (
            run.label
          )
        }
      >
        {run.total > 0 && <Donut done={run.done} total={run.total} />}
        <span className="text-foreground tabular-nums">{run.label}</span>
      </Pill>
      {diff && <span aria-hidden>·</span>}
      {diff && <DiffPill diff={diff} />}
    </Capsule>
  );
};

const Capsule: FC<{ children: ReactNode }> = ({ children }) => (
  <div className="pointer-events-none absolute inset-x-0 bottom-full mb-1.5 flex justify-center">
    <div
      data-slot="composer-capsule"
      className="border-border/80 bg-background/70 text-muted-foreground rounded-capsule animate-in fade-in slide-in-from-bottom-1 pointer-events-auto flex items-center gap-2 border px-3 py-1.5 text-xs backdrop-blur-sm duration-150 motion-reduce:animate-none"
    >
      {children}
    </div>
  </div>
);

/** A request's capsule outside a run: its plan's step and what its workers landed. */
const RequestCapsule: FC<{ conversationId: string }> = ({ conversationId }) => {
  const requestId = useBoard((s) => runningRequest(s.board, conversationId));
  const diff = useRequestDiff(requestId);
  const running = useRunningPlan(requestId);
  // A run's phase speaks through the run's capsule; once the run is over, nothing of it shows.
  if (!requestId || isRunRequest(requestId) || (!diff && !running)) return null;

  const done = running?.steps.filter((status) => status === "done").length ?? 0;
  const total = running?.plan.steps.length ?? 0;
  return (
    <Capsule>
      {running && (
        <Pill
          tip={
            <ol className="flex flex-col gap-1">
              {running.plan.steps.map((step, index) => (
                <li key={step.title} className="flex items-center gap-1.5">
                  <StepGlyph status={running.steps[index] ?? "pending"} />
                  <span className={cn(running.steps[index] !== "active" && "text-muted-foreground")}>
                    {step.title}
                  </span>
                </li>
              ))}
            </ol>
          }
        >
          <Donut done={done} total={total} />
          <span className="text-foreground tabular-nums">
            Step {Math.min(done + 1, total)} / {total}
          </span>
        </Pill>
      )}
      {running && diff && <span aria-hidden>·</span>}
      {diff && <DiffPill diff={diff} />}
    </Capsule>
  );
};

/** "N files changed +a −d", with each file on hover. */
const DiffPill: FC<{ diff: DiffStat }> = ({ diff }) => {
  const files = diff.files.length;
  return (
    <Pill
      tip={
        <ul className={cn(mono, "flex flex-col gap-0.5")}>
          {diff.files.map((file) => (
            <li key={file.path} className="flex gap-2">
              <span className="min-w-0 flex-1 truncate">{file.path}</span>
              {file.binary ? (
                <span className="text-muted-foreground">binary</span>
              ) : (
                <span className="tabular-nums">
                  <span className="text-success">+{file.insertions}</span>{" "}
                  <span className="text-destructive">−{file.deletions}</span>
                </span>
              )}
            </li>
          ))}
        </ul>
      }
    >
      <span>
        {files} {files === 1 ? "file" : "files"} changed
      </span>
      <span className="font-mono tabular-nums">
        <span className="text-success">+{diff.insertions}</span>{" "}
        <span className="text-destructive">−{diff.deletions}</span>
      </span>
    </Pill>
  );
};
