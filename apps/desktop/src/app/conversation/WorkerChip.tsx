import { createContext, memo, useContext, useState } from "react";

import { namedTasks, plainLine, workerName } from "@/app/conversation/rowWords";
import { IdGlyph } from "@/components/glyphs/worker-glyphs";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { Task, TaskState } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { type Board, useBoard } from "@/state/board";

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
 * A worker's own glyph and colour, the same wherever it appears; `working` adds its status dot,
 * `tone` recolours it for a state.
 */
export function WorkerGlyph({
  taskId,
  working,
  tone,
  className,
}: {
  taskId: string;
  working?: boolean | undefined;
  tone?: string | undefined;
  className?: string | undefined;
}) {
  return <IdGlyph id={taskId} dot={working} className={cn(className, tone)} />;
}

/** The task a checker (a review or a verifier) checks, while the board still has it. */
function checkSubject(board: Board | null | undefined, task: Task | undefined): Task | undefined {
  if ((task?.kind !== "review" && task?.kind !== "verify") || !task.subject) return undefined;
  return board?.tasks[task.subject];
}

/** What a worker is called (see `workerName`), rather than its `task-N`. */
export function useWorkerName(taskId: string): string | null {
  return useBoard((s) => {
    const task = s.board?.tasks[taskId];
    return task && s.board ? workerName(s.board.tasks, task) : null;
  });
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
  const number = useBoard((s) => s.board?.tasks[taskId]?.number);
  const stateLabel = useBoard((s) => {
    const task = s.board?.tasks[taskId];
    return task ? taskStateLabel(task) : undefined;
  });
  const [open, setOpen] = useState(false);
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
            "focus-visible:border-ring focus-visible:ring-ring/50 outline-none focus-visible:ring-1",
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

/**
 * How a line names a worker: its chip, and for a check of another worker the check's chip,
 * then "of" and that worker's chip ("[Review] of [Add tests]"). Items of a flex line.
 */
export const WorkerMention = memo(function WorkerMention({
  taskId,
  tone,
}: {
  taskId: string;
  tone?: string | null | undefined;
}) {
  const subject = useBoard((s) => checkSubject(s.board, s.board?.tasks[taskId])?.id ?? null);
  const kind = useBoard((s) => s.board?.tasks[taskId]?.kind);
  if (subject === null) return <WorkerChip taskId={taskId} tone={tone} />;
  return (
    <>
      <WorkerChip taskId={taskId} tone={tone} label={kind === "verify" ? "Check" : "Review"} className="shrink-0" />
      <span className="shrink-0">of</span>
      <WorkerChip taskId={subject} />
    </>
  );
});
