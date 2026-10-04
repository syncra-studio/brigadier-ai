import { ChevronDown } from "@openai/apps-sdk-ui/components/Icon";
import { type FC, memo, useContext, useEffect, useMemo, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { WORKERS_LABEL, WorkerGlyphs } from "@/app/conversation/Agents";
import { isFinal, isWorking } from "@/app/conversation/blocks";
import { TaskActivity } from "@/app/conversation/WorkerActivity";
import {
  AgentsPanelContext,
  useWorkerName,
  WorkerGlyph,
} from "@/app/conversation/WorkerChip";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { DiffStat, Task } from "@/ipc/generated";
import { modelName, useModelGroups } from "@/lib/setup";
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
    case "reviewing":
      return "is working";
    case "paused":
      return task.quotaWait ? "is waiting for quota" : "is awaiting instruction";
    case "blocked":
    case "awaitingApproval":
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

/** How a worker in a summary row is doing: its state in words, then its +N −N. */
const RowState: FC<{ task: Task }> = ({ task }) => (
  <>
    <span
      className={cn(
        "shrink-0",
        task.state === "failed"
          ? "text-destructive"
          : isFinal(task)
            ? "text-muted-foreground"
            : "text-foreground/70",
      )}
    >
      {stateLine(task)}
    </span>
    <WorkerChanges task={task} />
  </>
);

/** One worker in a summary: its glyph (with a dot while it works), name, state and +N −N. */
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
    <button
      type="button"
      data-slot="worker-summary-row"
      data-state={task.state}
      onClick={() => setPanel(task.id)}
      className={cn(
        "hover:bg-foreground/5 rounded-control flex h-control-sm items-center gap-2 text-start text-sm transition-colors",
        className,
      )}
    >
      <WorkerGlyph taskId={task.id} working={isWorking(task)} />
      <span className="min-w-0 flex-1 truncate">{name}</span>
      <RowState task={task} />
    </button>
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

/** Active workers listed in the summary before "N more". */
const ROWS = 4;

/**
 * The pinned summary's Workers section, folding under its title: one line with the stacked
 * glyphs and the counts ("2 working · 7 workers · 37 checks", which opens the list), then a row
 * per worker still at it. Checkers count apart: each opens from the row of what it checks.
 */
export function WorkersSummary({ conversationId }: { conversationId: string }) {
  const { setPanel } = useContext(AgentsPanelContext);
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
  const checks = useBoard((s) =>
    s.board?.conversationId === conversationId
      ? Object.values(s.board.tasks).filter((task) => task.gateLink !== null).length
      : 0,
  );
  const { active, all, working } = useMemo(() => {
    const id = (mark: string) => mark.replace(/^[af]\*?/, "");
    return {
      active: marked.filter((mark) => mark.startsWith("a")).map(id),
      all: marked.map(id),
      working: new Set(marked.filter((mark) => mark.includes("*")).map(id)),
    };
  }, [marked]);
  const [open, setOpen] = useState(true);
  if (all.length === 0) return null;
  const waiting = active.length - working.size;
  const hidden = active.length - ROWS;
  const counts = [
    working.size > 0 && `${working.size} working`,
    waiting > 0 && `${waiting} waiting`,
    `${all.length} ${all.length === 1 ? "worker" : "workers"}`,
    checks > 0 && `${checks} ${checks === 1 ? "check" : "checks"}`,
  ]
    .filter(Boolean)
    .join(" · ");
  return (
    <Collapsible open={open} onOpenChange={setOpen} className="flex flex-col gap-1">
      <CollapsibleTrigger className="group text-muted-foreground hover:text-foreground flex h-control-xs items-center justify-between text-xs transition-colors">
        {WORKERS_LABEL}
        <ChevronDown
          aria-hidden
          className="size-icon-xs transition-[rotate] group-data-[state=closed]:-rotate-90 motion-reduce:transition-none"
        />
      </CollapsibleTrigger>
      <CollapsibleContent className="flex flex-col">
        <button
          type="button"
          data-slot="workers-summary-counts"
          onClick={() => setPanel(null)}
          className="hover:bg-foreground/5 rounded-control -mx-1 flex h-control-sm items-center gap-2 px-1 text-start text-sm transition-colors"
        >
          <WorkerGlyphs taskIds={all} working={working} />
          <span className="min-w-0 flex-1 truncate">{counts}</span>
        </button>
        {active.slice(0, ROWS).map((id) => (
          <WorkerSummaryRow key={id} taskId={id} className="-mx-1 px-1" />
        ))}
        {hidden > 0 && (
          <button
            type="button"
            onClick={() => setPanel(null)}
            className="text-muted-foreground hover:text-foreground flex h-control-sm items-center ps-6 text-start text-sm transition-colors"
          >
            {hidden} more
          </button>
        )}
      </CollapsibleContent>
    </Collapsible>
  );
}
