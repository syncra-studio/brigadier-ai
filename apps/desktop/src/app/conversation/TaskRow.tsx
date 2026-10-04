import { ChevronRight } from "@openai/apps-sdk-ui/components/Icon";
import { type FC, memo, useContext } from "react";
import { useShallow } from "zustand/react/shallow";

import { isWorking } from "@/app/conversation/blocks";
import { STEP_ROW } from "@/app/conversation/OrchestratorSteps";
import {
  checkersOf,
  checkResult,
  checkRounds,
  checksCount,
  decisionWords,
  ownerKey,
  ROLE_LABELS,
  type RowState,
  taskDecisions,
  taskRowDetail,
  taskState,
} from "@/app/conversation/rowWords";
import { AgentsPanelContext, useWorkerName, WorkerChip, WorkerGlyph, WorkerLine } from "@/app/conversation/WorkerChip";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import type { Gate, PhaseState, Task } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { type Board, useBoard } from "@/state/board";

/**
 * A worker's one row in the thread, updated in place as it works, is checked, fixed and lands:
 * "[◆ Add tests] · Landed · checked by 1 review + 1 verify · 1 fix". Its name opens the worker;
 * the chevron opens its checks round by round (each checker opens too) and what was decided
 * about it.
 */

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

/** The chevron that opens a row's checks. */
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

/** What was decided about a task, each with why, oldest first. */
const TaskDecisions: FC<{ taskId: string }> = ({ taskId }) => {
  const decisions = useBoard(useShallow((s) => taskDecisions(s.board?.decisions ?? [], taskId)));
  if (decisions.length === 0) return null;
  return (
    <ul className="flex flex-col gap-1">
      {decisions.map((decision) => {
        const { what, why } = decisionWords(decision);
        return (
          <li key={decision.id} className="flex flex-col">
            <span className="text-foreground/80 wrap-break-word"><WorkerLine text={what} /></span>
            {why && <span className="wrap-break-word"><WorkerLine text={why} /></span>}
          </li>
        );
      })}
    </ul>
  );
};

/** What a task's row says, as primitives, so the row re-renders only when its words change. */
function rowFacts(board: Board | null | undefined, taskId: string): { word: string | null; tone: RowState["tone"]; detail: string } {
  const task = board?.tasks[taskId];
  if (!board || !task) return { word: null, tone: "quiet", detail: "" };
  const state = taskState(task);
  return { word: state.word, tone: state.tone, detail: taskRowDetail(task, checkersOf(board.tasks, [`task:${taskId}`])) };
}

export const TaskRow = memo(function TaskRow({ taskId }: { taskId: string }) {
  const { setPanel } = useContext(AgentsPanelContext);
  const name = useWorkerName(taskId);
  const working = useBoard((s) => {
    const task = s.board?.tasks[taskId];
    return task ? isWorking(task) : false;
  });
  const { word, tone, detail } = useBoard(useShallow((s) => rowFacts(s.board, taskId)));
  const gate = useBoard((s) => s.board?.tasks[taskId]?.gate ?? null);
  const checkerIds = useCheckerIds([`task:${taskId}`]);
  const decided = useBoard((s) => (s.board ? taskDecisions(s.board.decisions, taskId).length : 0));
  if (name === null || word === null) return null;
  const opens = checkerIds.length > 0 || decided > 0;
  const line = (
    <div data-slot="task-row" className={STEP_ROW}>
      <button
        type="button"
        onClick={() => setPanel(taskId)}
        title={name}
        className="hover:text-foreground focus-visible:ring-ring/50 rounded-control flex min-w-0 items-center gap-1.5 outline-none focus-visible:ring-1"
      >
        <WorkerGlyph taskId={taskId} working={working} className="size-icon-sm shrink-0" />
        <span className="text-foreground/90 min-w-0 truncate">{name}</span>
      </button>
      {/* Capped so a narrow thread truncates the detail, never the task's title. */}
      <span className="flex max-w-3/5 min-w-0 shrink-0 items-center gap-1.5 whitespace-nowrap">
        <span aria-hidden>·</span>
        <span className={cn("shrink-0", TONES[tone])}>{word}</span>
        {detail && (
          <>
            <span aria-hidden>·</span>
            <span className="min-w-0 truncate" title={detail}>
              {detail}
            </span>
          </>
        )}
      </span>
      {opens && <Opener label={`Show the checks of ${name}`} />}
    </div>
  );
  if (!opens) return line;
  return (
    <Collapsible data-slot="task-row-group">
      {line}
      <CollapsibleContent className="text-muted-foreground flex flex-col gap-2 ps-6 pt-1 pb-1 text-sm">
        <ChecksList checkerIds={checkerIds} gates={{ [`task:${taskId}`]: gate }} />
        <TaskDecisions taskId={taskId} />
      </CollapsibleContent>
    </Collapsible>
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
