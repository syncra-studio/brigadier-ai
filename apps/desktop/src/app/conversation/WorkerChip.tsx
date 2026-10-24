import { createContext, memo, useContext, useState } from "react";

import { rowName, workerStateOf } from "@/app/conversation/activity/workerState";
import { namedTasks, plainLine, workerName } from "@/app/conversation/rowWords";
import { WorkerGlyph } from "@/components/conversation";
import { useNow } from "@/hooks/use-now";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import type { Task, TaskState } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { workersOf } from "@/state/nativeWorkers";
import { useBoard } from "@/state/board";
import { useCliThread, useWorkerRow, workerRowIn } from "@/state/cliThread";

/**
 * A worker named anywhere in the conversation: its chip (glyph and name in a pill, which opens
 * it in the Workers panel), and what the chip is made of, shared by every place that names one.
 */

/** Which worker the Workers tab shows: `null` for the list, `undefined` when the tab is closed. */
export type AgentsPanelState = string | null | undefined;

export const AgentsPanelContext = createContext<{
  panel: AgentsPanelState;
  setPanel: (panel: AgentsPanelState) => void;
}>({ panel: undefined, setPanel: () => {} });

export const TASK_STATE_LABELS: Record<TaskState, string> = {
  queued: "Queued",
  starting: "Starting",
  running: "Running",
  blocked: "Blocked",
  paused: "Paused",
  reported: "Reported",
  takenOver: "In your terminal",
  landing: "Landing",
  readyToLand: "Ready to land",
  landed: "Landed",
  done: "Done",
  rejected: "Rejected",
  stopped: "Stopped",
  failed: "Failed",
};

/** A task's state in words; a task paused for quota waits for it rather than for the user. */
export function taskStateLabel(task: Task): string {
  return task.state === "paused" && task.quotaWait ? "Waiting for quota" : TASK_STATE_LABELS[task.state];
}

/**
 * What a worker is called, rather than its `task-N`: its row's name among its session's rows
 * (`rowName`). A worker no thread read lists yet (a view still loading, a stored run's) keeps the
 * board's name for it (`workerName`).
 */
export function useWorkerName(taskId: string): string | null {
  const fromRow = useCliThread((s) => {
    const found = workerRowIn(s.views, taskId);
    return found ? rowName(workersOf(found.conversationId, s.views[found.conversationId]!), taskId) : null;
  });
  const fromBoard = useBoard((s) => {
    if (fromRow !== null) return null;
    const task = s.board?.tasks[taskId];
    return task && s.board ? workerName(s.board.tasks, task) : null;
  });
  return fromRow ?? fromBoard;
}

/** Worker prose uses job names; paths and code identifiers keep their literal spelling. */
export function useWorkerText(text: string): string {
  return useBoard((s) => namedTasks(text, s.board?.tasks ?? {}).replace(/(?<![\p{L}\p{N}_/-])task-\d+(?![\p{L}\p{N}_/-]|\.\w)/gu, "a worker"));
}

/** A decision's or a waiting item's line as the user reads it: plain, its workers by name. */
export const WorkerLine = memo(function WorkerLine({ text }: { text: string }) {
  return useBoard((s) => plainLine(namedTasks(text, s.board?.tasks ?? {})));
});

/**
 * A worker named in a line: its glyph and name in a pill that truncates a long name, shows it
 * whole with the worker's state on hover, and opens the worker in the panel. The
 * glyph keeps the worker's own colour wherever it shows, so one worker never looks like two;
 * a line that must mark it gives a `tone`. `label` names it shorter where the line says the rest.
 */
export const WorkerChip = memo(function WorkerChip({
  taskId,
  tone,
  label,
  className,
}: {
  taskId: string;
  tone?: string | null | undefined;
  label?: string | undefined;
  className?: string | undefined;
}) {
  const { setPanel } = useContext(AgentsPanelContext);
  const name = useWorkerName(taskId);
  const row = useWorkerRow(taskId);
  const [open, setOpen] = useState(false);
  // Its state as its row says it when the tooltip opens.
  const now = useNow(open ? 30_000 : null);
  const boardNumber = useBoard((s) => (row ? undefined : s.board?.tasks[taskId]?.number));
  const boardLabel = useBoard((s) => {
    const task = row ? undefined : s.board?.tasks[taskId];
    return task ? taskStateLabel(task) : undefined;
  });
  const number = row?.number ?? boardNumber;
  const stateLabel = row ? workerStateOf(row, now).word : boardLabel;
  if (name === null || number === undefined || stateLabel === undefined) {
    return <span className="shrink-0">a worker</span>;
  }
  return (
    <Tooltip open={open} onOpenChange={setOpen}>
      <TooltipTrigger asChild>
        <button
          type="button"
          data-slot="worker-chip"
          data-task={`task-${number}`}
          onClick={() => setPanel(taskId)}
          className={cn(
            "bg-muted/60 border-border text-foreground hover:bg-muted rounded-capsule inline-flex h-control-xs max-w-2xs min-w-0 shrink items-center gap-1.5 border ps-2 pe-2.5 align-middle text-sm leading-normal transition-colors",
            // A ring for keyboard focus only, as on buttons.
            "",
            className,
          )}
        >
          <WorkerGlyph
            taskId={taskId}
            tone={tone ?? undefined}
            className="size-icon-sm animate-glyph-in motion-reduce:animate-none"
          />
          <span className="min-w-0 truncate">{label ?? name}</span>
        </button>
      </TooltipTrigger>
      <TooltipContent
        side="top"
        className="flex-col gap-0.5"
        onEscapeKeyDown={(event) => {
          // Radix handles Escape in document capture, before the chip's key handlers.
          // Consume this Escape for the tooltip; the next reaches the enclosing popup.
          event.preventDefault();
          setOpen(false);
        }}
      >
        <span className="wrap-break-word">{name}</span>
        <span className="text-muted-foreground text-xs">{stateLabel}</span>
      </TooltipContent>
    </Tooltip>
  );
});
