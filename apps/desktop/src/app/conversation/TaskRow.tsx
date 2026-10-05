import { ChevronRight } from "@openai/apps-sdk-ui/components/Icon";
import { type FC, memo, useContext } from "react";
import { useShallow } from "zustand/react/shallow";

import { isFinal, isWorking } from "@/app/conversation/blocks";
import { STEP_ROW } from "@/app/conversation/OrchestratorSteps";
import {
  checkersOf,
  checkResult,
  checkRounds,
  checksCount,
  ownerKey,
  ROLE_LABELS,
  type RowState,
} from "@/app/conversation/rowWords";
import { TaskActivity } from "@/app/conversation/WorkerActivity";
import { AgentsPanelContext, useWorkerName, WorkerChip, WorkerGlyph } from "@/app/conversation/WorkerChip";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import type { Gate, PhaseState, Task } from "@/ipc/generated";
import { useNow } from "@/hooks/use-now";
import { formatDuration } from "@/lib/format";
import { cn } from "@/lib/utils";
import { type Board, useBoard } from "@/state/board";

const TONES: Record<RowState["tone"], string> = {
  live: "shimmer",
  done: "",
  warning: "text-warning",
  failed: "text-destructive",
  quiet: "",
};

/** The ids of an owner's checkers, so a row re-renders only when they change. */
function useCheckerIds(owners: readonly string[]): string[] {
  return useBoard(
    useShallow((s) => (s.board ? checkersOf(s.board.tasks, owners).map((task) => task.id) : [])),
  );
}

/** The chevron that opens a phase's checks. */
const Opener: FC<{ label: string }> = ({ label }) => (
  <CollapsibleTrigger
    aria-label={label}
    className="group/opener hover:text-foreground rounded-control focus-visible:ring-ring/50 -my-1 flex size-control-xs shrink-0 items-center justify-center outline-none focus-visible:ring-1"
  >
    <ChevronRight
      aria-hidden
      className="size-icon-xs transition-[rotate] group-data-[state=open]/opener:rotate-90 motion-reduce:transition-none"
    />
  </CollapsibleTrigger>
);

/**
 * Checks round by round: "Round 2 · [Review] passed · [Verify] found problems". `gates` are
 * the owners' current rounds by owner key, which hold the newest results.
 */
export function ChecksList({
  checkerIds,
  gates,
}: {
  checkerIds: readonly string[];
  gates: Readonly<Record<string, Gate | null>>;
}) {
  const checkers = useBoard(useShallow((s) => checkerIds.flatMap((id) => s.board?.tasks[id] ?? [])));
  return (
    <ol className="flex flex-col gap-1">
      {checkRounds(checkers).map((round, index) => (
        <li key={round[0]?.id ?? index} className="flex min-w-0 flex-wrap items-center gap-x-1.5 gap-y-1">
          <span className="shrink-0">Round {index + 1}</span>
          {round.map((checker) => (
            <span key={checker.id} className="flex min-w-0 items-center gap-1">
              <span aria-hidden>·</span>
              <WorkerChip
                taskId={checker.id}
                label={ROLE_LABELS[checker.gateLink?.role ?? "review"]}
                className="shrink-0"
              />
              <span className="shrink-0">
                {checkResult(checker, checker.gateLink ? (gates[ownerKey(checker.gateLink.owner)] ?? null) : null)}
              </span>
            </span>
          ))}
        </li>
      ))}
    </ol>
  );
}

/** How a worker's row says where it is: "started working", "finished", "failed". */
export function lifecycleWords(task: Task): RowState {
  switch (task.state) {
    case "queued":
    case "blocked":
    case "paused":
      if (task.quotaWait) return { word: "is waiting for quota", tone: "quiet" };
      return { word: task.state === "queued" ? "is queued" : task.state === "paused" ? "is paused" : "is waiting", tone: "quiet" };
    case "starting":
      return { word: "is starting", tone: "live" };
    case "running":
      return { word: "started working", tone: "live" };
    case "reported":
    case "landing":
    case "readyToLand":
    case "landed":
    case "done":
      return { word: "finished", tone: "done" };
    case "rejected":
      return { word: "was turned down", tone: "quiet" };
    case "stopped":
      return { word: "was stopped", tone: "warning" };
    case "failed":
      return { word: "failed", tone: "failed" };
  }
}

/** What a task's row says, as primitives, so the row re-renders only when its words change. */
function rowFacts(board: Board | null | undefined, taskId: string): { word: string | null; tone: RowState["tone"]; final: boolean; from: number; to: number } {
  const task = board?.tasks[taskId];
  if (!board || !task) return { word: null, tone: "quiet", final: false, from: 0, to: 0 };
  const { word, tone } = lifecycleWords(task);
  return { word, tone, final: isFinal(task), from: task.createdAtMs, to: task.updatedAtMs };
}

/**
 * A worker's one row in the thread, updated in place as it works: "Lead · Phase 1 started
 * working" with what it does right now under it, then "Lead · Phase 1 finished · 4m 12s". The
 * row opens the worker's own thread.
 */
export const TaskRow = memo(function TaskRow({ taskId }: { taskId: string }) {
  const { setPanel } = useContext(AgentsPanelContext);
  const name = useWorkerName(taskId);
  const { word, tone, final, from, to } = useBoard(useShallow((s) => rowFacts(s.board, taskId)));
  const live = tone === "live";
  const now = useNow(final ? null : 1000);
  if (name === null || word === null) return null;
  const elapsed = formatDuration(Math.max(0, (final ? to : now) - from));
  return (
    <div className="min-w-0">
      <button
        type="button"
        data-slot="task-row"
        onClick={() => setPanel(taskId)}
        aria-label={`Open ${name}`}
        title={name}
        className={cn(STEP_ROW, "group hover:text-foreground focus-visible:ring-ring/50 rounded-control w-full text-start outline-none focus-visible:ring-1")}
      >
        <WorkerGlyph taskId={taskId} working={live} className="size-icon-sm shrink-0" />
        <span className="min-w-0 truncate">
          <span className="text-foreground/90">{name}</span>{" "}
          <span aria-live="polite" className={TONES[tone]}>{word}</span>
        </span>
        <span className="shrink-0 tabular-nums">· {elapsed}</span>
        <ChevronRight
          aria-hidden
          className="size-icon-xs shrink-0 opacity-0 transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100"
        />
      </button>
      {!final && <TaskActivity taskId={taskId} className="ps-6 pb-1" />}
    </div>
  );
});

/** A phase's whole-phase checks as one row ("Whole-phase checks · passed · 1 review + 1 verify + 1 judge"). */
export const PhaseChecksRow = memo(function PhaseChecksRow({ runId, phaseId }: { runId: string; phaseId: string }) {
  const owner = `phase:${runId}:${phaseId}`;
  const checkerIds = useCheckerIds([owner]);
  const gate = useBoard((s) => s.board?.overnight[runId]?.phases.find((phase) => phase.id === phaseId)?.gate ?? null);
  // Phase 0 keeps no round of its own: its judge's verdict settles the planning phase.
  const planning = useBoard((s) => (phaseId === PLANNING_PHASE ? (s.board?.overnight[runId]?.planning?.state ?? null) : null));
  const counted = useBoard((s) => (s.board ? checksCount(checkersOf(s.board.tasks, [owner])) : ""));
  const working = useBoard((s) => checkerIds.some((id) => {
    const task: Task | undefined = s.board?.tasks[id];
    return task ? isWorking(task) : false;
  }));
  if (checkerIds.length === 0) return null;
  const outcome = working ? "checking" : gate || !planning ? gateWord(gate) : planningWord(planning);
  return (
    <Collapsible data-slot="phase-checks-row">
      <div className={STEP_ROW}>
        <span className="text-foreground/90 shrink-0">Whole-phase checks</span>
        <span className="flex min-w-0 items-center gap-1.5 truncate">
          <span aria-hidden>·</span>
          <span className={cn(working && "shimmer")}>{outcome}</span>
          <span aria-hidden>·</span>
          <span className="truncate">{counted}</span>
        </span>
        <Opener label="Show the whole-phase checks" />
      </div>
      <CollapsibleContent className="text-muted-foreground ps-6 pt-1 pb-1 text-sm">
        <ChecksList checkerIds={checkerIds} gates={{ [owner]: gate }} />
      </CollapsibleContent>
    </Collapsible>
  );
});

/** The phase id of Phase 0, where the plan is written and judged. */
const PLANNING_PHASE = "phase-0";

/** Phase 0's checks in a word, from what the planning phase came to. */
function planningWord(state: PhaseState): string {
  switch (state) {
    case "verified":
      return "passed";
    case "partial":
    case "blocked":
      return "found gaps";
    case "skipped":
      return "skipped";
    default:
      return "checking";
  }
}

/** A finished round of checks in a word. */
export function gateWord(gate: Gate | null): string {
  switch (gate?.outcome?.type) {
    case "passed":
      return "passed";
    case "failed":
      return "found gaps";
    case "unverified":
      return "couldn’t verify";
    case "noResult":
      return "no result";
    case "superseded":
      return "cut short";
    default:
      return gate ? "checking" : "done";
  }
}
