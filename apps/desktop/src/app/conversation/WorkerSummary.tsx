import { type FC, memo, useContext, useEffect, useMemo, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { WORKERS_LABEL } from "@/app/conversation/Agents";
import { isFinal, isWorking } from "@/app/conversation/blocks";
import { TaskActivity } from "@/app/conversation/WorkerActivity";
import { useTaskElapsed } from "@/app/conversation/WorkerThread";
import {
  AgentsPanelContext,
  useWorkerName,
  WorkerGlyph,
} from "@/app/conversation/WorkerChip";
import { SummaryRowButton, SummarySection } from "@/components/assistant-ui/elements/summary-section";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { DiffStat, Task } from "@/ipc/generated";
import { modelName, useModelGroups } from "@/lib/setup";
import { formatDuration } from "@/lib/format";
import { cn } from "@/lib/utils";
import { refreshWorkerDiffs } from "@/state/actions";
import { useBoard } from "@/state/board";

/**
 * The session's workers summed up: glyph, name, state and diff totals in the pinned summary's
 * Workers section; glyph, name and live activity on the composer's background-workers strip.
 */

/** What a worker is at, in plain words ("is working", "is awaiting instruction"). */
export function stateLine(task: Task): string {
  switch (task.state) {
    case "queued":
    case "starting":
    case "running":
    case "landing":
      return "is working";
    case "paused":
      return task.quotaWait ? "is waiting for quota" : "is awaiting instruction";
    case "blocked":
    case "readyToLand":
      return "is awaiting instruction";
    case "failed":
      return "failed";
    case "stopped":
      return "was interrupted";
    case "reported":
      return "reported";
    case "landed":
    case "done":
    case "rejected":
      return "is done";
  }
}

/** "+12 −3", once there is a change; `monochrome` for a total, in the text's colour. */
export const Changes: FC<{ insertions: number; deletions: number; monochrome?: boolean }> = ({
  insertions,
  deletions,
  monochrome = false,
}) =>
  insertions + deletions > 0 ? (
    <span className="shrink-0 font-mono text-xs tabular-nums">
      <span className={cn(!monochrome && "text-success")}>+{insertions}</span>{" "}
      <span className={cn(!monochrome && "text-destructive")}>−{deletions}</span>
    </span>
  ) : null;

/**
 * States in which a write task's +N −N is read from its worktree: it still changes, or it
 * reported and its candidate commit isn't made yet.
 */
const EDITING: ReadonlySet<Task["state"]> = new Set(["running", "blocked", "paused", "reported"]);

function writes(task: Task): boolean {
  return task.kind === "implement" || task.kind === "merge";
}

/** A worker's +N −N: its worktree so far while it works, then its candidate commit's. */
export function workerStat(
  task: Task,
  diffs: Readonly<Record<string, DiffStat>> | undefined,
): DiffStat | undefined {
  return (EDITING.has(task.state) ? diffs?.[task.id] : undefined) ?? task.candidate?.diffStat;
}

const WorkerChanges: FC<{ task: Task }> = ({ task }) => {
  const live = useBoard((s) => (EDITING.has(task.state) ? s.board?.diffs[task.id] : undefined));
  const stat = live ?? task.candidate?.diffStat;
  return stat ? <Changes insertions={stat.insertions} deletions={stat.deletions} /> : null;
};

/** The soonest a burst of edits reads the worktrees again, and the least time between reads. */
const DIFF_SETTLE_MS = 500;
const DIFF_INTERVAL_MS = 3_000;

/**
 * Keeps the board's live +N −N of the conversation's workers current: read again after
 * a worker at work on a change edits files, runs a command or changes state, at most every
 * few seconds.
 * It renders nothing, so what it watches re-renders only itself.
 */
export function WorkerDiffs({ conversationId }: { conversationId: string }) {
  // Changes whenever a write task at work edits files, runs a command or changes state.
  const key = useBoard((s) => {
    const board = s.board;
    if (board?.conversationId !== conversationId) return "";
    let edited = "";
    for (const task of Object.values(board.tasks)) {
      if (writes(task) && EDITING.has(task.state)) {
        edited += `${task.id}:${task.state}:${board.edits[task.id] ?? 0};`;
      }
    }
    return edited;
  });
  const last = useRef(0);
  useEffect(() => {
    if (!key) return;
    const delay = Math.max(DIFF_SETTLE_MS, last.current + DIFF_INTERVAL_MS - Date.now());
    const timer = setTimeout(() => {
      last.current = Date.now();
      // Until git can tell (a worktree being set up), the last read stays.
      refreshWorkerDiffs(conversationId).catch(() => {});
    }, delay);
    return () => clearTimeout(timer);
  }, [conversationId, key]);
  return null;
}

/** How a worker in a summary row is doing: its state in words (shimmering while it works), then how long it has worked. */
const RowState: FC<{ task: Task }> = ({ task }) => {
  const elapsed = useTaskElapsed(task);
  return (
    <>
      <span
        className={cn(
          "min-w-0 truncate",
          task.state === "failed" && "text-destructive",
          isWorking(task) && "shimmer",
        )}
      >
        {stateLine(task)}
      </span>
      {elapsed >= 1000 && <span className="shrink-0 tabular-nums">{formatDuration(elapsed)}</span>}
    </>
  );
};

/** One worker in a summary: its glyph (with a dot while it works), name and +N −N over its state and time. */
export const WorkerSummaryRow = memo(function WorkerSummaryRow({
  taskId,
  className,
}: {
  taskId: string;
  className?: string;
}) {
  const task = useBoard((s) => s.board?.tasks[taskId]);
  const name = useWorkerName(taskId);
  const { setPanel } = useContext(AgentsPanelContext);
  if (!task) return null;
  return (
    <SummaryRowButton
      data-slot="worker-summary-row"
      data-state={task.state}
      onClick={() => setPanel(task.id)}
      icon={<WorkerGlyph taskId={task.id} working={isWorking(task)} />}
      description={<RowState task={task} />}
      meta={<WorkerChanges task={task} />}
      className={className}
    >
      {name}
    </SummaryRowButton>
  );
});

/**
 * One worker on the composer's background-workers strip: its glyph, name and state in words
 * as a quiet button that opens it, its kind and model on hover, then its live activity.
 */
export const WorkerStripRow = memo(function WorkerStripRow({
  taskId,
  className,
}: {
  taskId: string;
  className?: string;
}) {
  const task = useBoard((s) => s.board?.tasks[taskId]);
  const name = useWorkerName(taskId);
  const { setPanel } = useContext(AgentsPanelContext);
  const groups = useModelGroups();
  if (!task) return null;
  const choice = task.route.choice;
  return (
    <div
      data-slot="worker-strip-row"
      data-state={task.state}
      className={cn("flex min-h-control-sm min-w-0 flex-col items-start text-sm", className)}
    >
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            type="button"
            onClick={() => setPanel(task.id)}
            className="hover:bg-foreground/5 rounded-control focus-visible:ring-ring/50 -mx-1 flex h-control-xs min-w-0 max-w-full items-center gap-1.5 px-1 text-start outline-none transition-colors focus-visible:ring-1"
          >
            <WorkerGlyph taskId={task.id} className="size-icon-sm" />
            <span className="text-foreground min-w-0 truncate">{name}</span>
          </button>
        </TooltipTrigger>
        <TooltipContent side="top" className="flex-col gap-0.5">
          <span className="wrap-break-word">{name}</span>
          <span className="text-muted-foreground text-xs">
            Uses {modelName(groups, choice)}
            {choice.effort && ` · ${choice.effort}`}
          </span>
        </TooltipContent>
      </Tooltip>
      <TaskActivity taskId={taskId} className="w-full ps-5 pb-1" />
    </div>
  );
});

/** Workers listed in the summary before "Show N more". */
const ROWS = 5;

/**
 * The pinned summary's Workers section: a row per worker (its glyph, name, state, time and
 * +N −N), those still at it first; folded, it says "2 working · 1 done". Checkers are not
 * listed: each opens from the row of what it checks.
 */
export function WorkersSummary({ conversationId }: { conversationId: string }) {
  // Workers by number, each marked active (`a`) or finished (`f`), and whether it works now (`*`).
  const marked = useBoard(
    useShallow((s) =>
      s.board?.conversationId === conversationId
        ? Object.values(s.board.tasks)
            .filter((task) => task.gateLink === null)
            .toSorted((a, b) => a.number - b.number)
            .map((task) => `${isFinal(task) ? "f" : "a"}${isWorking(task) ? "*" : ""}${task.id}`)
        : [],
    ),
  );
  const [all, setAll] = useState(false);
  const { ordered, working, done } = useMemo(() => {
    const id = (mark: string) => mark.replace(/^[af]\*?/, "");
    return {
      ordered: [
        ...marked.filter((mark) => mark.startsWith("a")),
        ...marked.filter((mark) => mark.startsWith("f")),
      ].map(id),
      working: marked.filter((mark) => mark.includes("*")).length,
      done: marked.filter((mark) => mark.startsWith("f")).length,
    };
  }, [marked]);
  if (ordered.length === 0) return null;
  const waiting = ordered.length - done - working;
  const summary = [
    working > 0 && `${working} working`,
    waiting > 0 && `${waiting} waiting`,
    done > 0 && `${done} done`,
  ]
    .filter(Boolean)
    .join(" · ");
  const hidden = ordered.length - ROWS;
  return (
    <SummarySection
      foldKey="workers"
      title={WORKERS_LABEL}
      count={ordered.length}
      summary={summary}
      aria-label="Session workers"
    >
      {(all ? ordered : ordered.slice(0, ROWS)).map((id) => (
        <WorkerSummaryRow key={id} taskId={id} />
      ))}
      {hidden > 0 && (
        <SummaryRowButton muted onClick={() => setAll(!all)}>
          {all ? "Show less" : `Show ${hidden} more`}
        </SummaryRowButton>
      )}
    </SummarySection>
  );
}
