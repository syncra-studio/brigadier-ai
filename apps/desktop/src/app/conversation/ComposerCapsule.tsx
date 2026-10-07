import { Check, Clock, X } from "@openai/apps-sdk-ui/components/Icon";
import { type FC, type ReactNode, useContext, useMemo } from "react";
import { useShallow } from "zustand/react/shallow";

import { ComposerTargetContext } from "@/app/conversation/composerTarget";
import { useRunDiff } from "@/app/conversation/overnightAdapter";
import { type RunPill, runPill } from "@/app/conversation/phaseView";
import {
  activePlanRequest,
  capsuleMode,
  currentRequestPlan,
  planProgress,
  planStepStatus,
} from "@/app/conversation/planProgress";
import { mono } from "@/components/assistant-ui/elements/surfaces";
import { Spinner } from "@/components/glyphs/spinner";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { DiffStat, FileStat } from "@/ipc/generated";
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

/** Something waits for the user's decision: the capsule makes room for its card. */
function anyPending(board: Board | null, conversationId: string): boolean {
  if (!board || board.conversationId !== conversationId) return false;
  return (
    Object.values(board.approvals).some((approval) => approval.state.type === "pending") ||
    Object.values(board.questions).some((question) => question.answeredAtMs === null) ||
    Object.values(board.plans).some((plan) => plan.state.type === "proposed")
  );
}

/** Where the session's active run is, if one is at work. */
function useRunPill(conversationId: string): RunPill | null {
  const runs = useBoard((s) => (s.board?.conversationId === conversationId ? s.board.overnight : null));
  const plans = useBoard((s) => s.board?.plans);
  return useMemo(() => (runs && plans ? runPill(runs, plans) : null), [runs, plans]);
}

/** The newest revision belongs to this request, regardless of its lifecycle. */
function useRunningPlan(requestId: string | null) {
  const plan = useBoard((s) => currentRequestPlan(s.board?.plans ?? {}, requestId));
  const states = useBoard(
    useShallow((s) =>
      (plan?.steps ?? []).map((step) => step.taskId ? s.board?.tasks[step.taskId]?.state : undefined),
    ),
  );
  return plan ? { plan, states } : null;
}

const Pill: FC<{ tip: ReactNode; label?: string; children: ReactNode }> = ({ tip, label, children }) => (
  <Tooltip delayDuration={0}>
    <TooltipTrigger asChild>
      {label ? (
        // A plan's checklist: reachable from the keyboard too.
        <button
          type="button"
          aria-label={label}
          className="hover:text-foreground rounded-control flex min-w-0 items-center gap-1.5 text-start transition-colors"
        >
          {children}
        </button>
      ) : (
        <span className="flex items-center gap-1.5">{children}</span>
      )}
    </TooltipTrigger>
    <TooltipContent side="top" className="flex-col items-stretch gap-1 py-1.5">
      {tip}
    </TooltipContent>
  </Tooltip>
);

/** A plan's whole checklist, as its pill shows it on hover: each step's mark and words. */
const StepList: FC<{ steps: readonly { title: string; status: ReturnType<typeof planStepStatus> }[] }> = ({ steps }) => (
  <ol className="flex max-h-(--radix-tooltip-content-available-height) max-w-80 flex-col gap-2 overflow-y-auto px-0.5 py-1 text-start">
    {steps.map((step, index) => (
      <li key={index} className="flex min-w-0 items-start gap-2">
        <span className="flex size-4 shrink-0 items-center justify-center">
          <StepGlyph status={step.status} />
        </span>
        <span
          className={cn(
            "min-w-0 max-w-72 leading-4 wrap-break-word",
            step.status === "done" ? "text-muted-foreground/70" : step.status === "active" ? "text-foreground" : "text-muted-foreground",
          )}
        >
          {step.title}
        </span>
        <span className="sr-only">
          {step.status === "pending" ? "Not started" : step.status === "active" ? "Running" : step.status === "done" ? "Done" : "Failed"}
        </span>
      </li>
    ))}
  </ol>
);

/** "Phase N / M": the phase at work, else the first still to do, else the last. */
function phaseCount(statuses: readonly ReturnType<typeof planStepStatus>[]): string {
  const at = statuses.indexOf("active");
  const next = statuses.findIndex((status) => status !== "done");
  const phase = at >= 0 ? at : next >= 0 ? next : statuses.length - 1;
  return `Phase ${phase + 1} / ${statuses.length}`;
}

/** A 12 pt progress ring, filling clockwise from the top as phases finish. */
const Donut: FC<{ done: number; total: number }> = ({ done, total }) => (
  <svg aria-hidden viewBox="0 0 16 16" className="text-link size-icon-xs shrink-0 -rotate-90">
    <circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" className="stroke-2 opacity-25" />
    <circle
      cx="8"
      cy="8"
      r="6"
      fill="none"
      stroke="currentColor"
      pathLength={100}
      strokeDasharray={`${total > 0 ? (done / total) * 100 : 0} 100`}
      className="stroke-2 transition-[stroke-dasharray] duration-200 motion-reduce:transition-none"
    />
  </svg>
);

const StepGlyph: FC<{ status: ReturnType<typeof planStepStatus> }> = ({ status }) => {
  switch (status) {
    case "done":
      return <Check className="text-muted-foreground size-icon-xs" />;
    case "active":
      return <Spinner className="size-icon-xs animate-spin motion-reduce:animate-none" />;
    case "failed":
      return <X className="text-destructive size-icon-xs" />;
    case "pending":
      return <span className="border-foreground/35 size-icon-xs rounded-full border" />;
  }
};

/**
 * The active request's plan progress and landed diff. Overnight runs keep their phase capsule.
 */
export const ComposerCapsule: FC = () => {
  const conversationId = useContext(ComposerTargetContext)?.conversation?.id ?? "";
  const pending = useBoard((s) => anyPending(s.board, conversationId));
  const run = useRunPill(conversationId);
  const request = useBoard((s) =>
    s.board?.conversationId === conversationId ? activePlanRequest(s.board.requests) : null,
  );
  const plan = useBoard((s) => currentRequestPlan(s.board?.plans ?? {}, request?.id ?? null));
  const mode = capsuleMode(run !== null, pending, request, plan);
  if (mode === null) return null;
  return mode === "run" && run ? <RunCapsule conversationId={conversationId} run={run} /> : <RequestCapsule conversationId={conversationId} />;
};

/** The capsule of an overnight run: its phase, the phase's steps and the run branch's diff. */
const RunCapsule: FC<{ conversationId: string; run: RunPill }> = ({ conversationId, run }) => {
  const runDiff = useRunDiff(conversationId, run.runId);
  // Nothing landed yet: no "0 files changed".
  const diff = runDiff && runDiff.files.length > 0 ? runDiff : null;
  return (
    <Capsule>
      <Pill
        tip={
          run.steps.length > 0 ? (
            <StepList
              steps={run.steps.map((step) => ({
                title: step.title,
                status: step.done ? "done" : step.active ? "active" : "pending",
              }))}
            />
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
      className="max-w-full border-border/80 bg-background/70 text-muted-foreground rounded-capsule animate-in fade-in slide-in-from-bottom-1 pointer-events-auto flex items-center gap-2 border px-3 py-1.5 text-xs backdrop-blur-sm duration-150 motion-reduce:animate-none"
    >
      {children}
    </div>
  </div>
);

/** A request's capsule outside a run: its plan's step and what its workers landed. */
const RequestCapsule: FC<{ conversationId: string }> = ({ conversationId }) => {
  const requestId = useBoard((s) =>
    s.board?.conversationId === conversationId ? activePlanRequest(s.board.requests)?.id ?? null : null,
  );
  const diff = useRequestDiff(requestId);
  const running = useRunningPlan(requestId);
  if (!requestId || (!diff && !running)) return null;
  const progress = running ? planProgress(running.plan, running.states) : null;
  const statuses = running ? running.plan.steps.map((_, index) => planStepStatus(running.states[index])) : [];
  return (
    <Capsule>
      {running && progress && (
        <Pill
          label={`Plan steps: ${running.plan.title}`}
          tip={
            <StepList
              steps={running.plan.steps.map((step, index) => ({
                title: step.title,
                status: planStepStatus(running.states[index]),
              }))}
            />
          }
        >
          {progress.status === "review" ? (
            <>
              <Clock aria-hidden className="size-icon-xs shrink-0" />
              <span className="text-foreground min-w-0 truncate">{progress.label}</span>
            </>
          ) : progress.status === "failed" ? (
            <>
              <X aria-hidden className="text-destructive size-icon-xs shrink-0" />
              <span className="text-foreground min-w-0 truncate">{progress.label}</span>
            </>
          ) : (
            <>
              <Donut done={statuses.filter((status) => status === "done").length} total={statuses.length} />
              <span className="text-foreground whitespace-nowrap tabular-nums">{phaseCount(statuses)}</span>
            </>
          )}
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
