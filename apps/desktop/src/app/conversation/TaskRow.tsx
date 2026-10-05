import { ChevronRight } from "@openai/apps-sdk-ui/components/Icon";
import { memo, useContext } from "react";
import { useShallow } from "zustand/react/shallow";

import { isFinal } from "@/app/conversation/blocks";
import { STEP_ROW } from "@/app/conversation/OrchestratorSteps";
import { type RowState } from "@/app/conversation/rowWords";
import { TaskActivity } from "@/app/conversation/WorkerActivity";
import { AgentsPanelContext, useWorkerName, WorkerGlyph } from "@/app/conversation/WorkerChip";
import type { Task } from "@/ipc/generated";
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
