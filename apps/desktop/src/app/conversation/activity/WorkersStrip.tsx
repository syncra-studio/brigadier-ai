import { ChevronRight, StopCircle } from "@openai/apps-sdk-ui/components/Icon";
import { type FC, memo, useContext, useMemo, useState } from "react";

import { stripWords, workersStrip } from "@/app/conversation/activity/stripView";
import { useAction } from "@/app/conversation/useAction";
import { useTaskActivityLine } from "@/app/conversation/WorkerActivity";
import { AgentsPanelContext, useWorkerName, WorkerGlyph } from "@/app/conversation/WorkerChip";
import { workerDone, workerState } from "@/app/conversation/workerPresentation";
import { Changes, workerStat } from "@/app/conversation/WorkerSummary";
import { CHEVRON } from "@/components/assistant-ui/elements/activity-row";
import { ComposerRailItem } from "@/components/assistant-ui/elements/composer-rail";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useNow } from "@/hooks/use-now";
import { formatDuration } from "@/lib/format";
import { cn } from "@/lib/utils";
import { stopWorkers } from "@/state/actions";
import { useBoard } from "@/state/board";

/** How long a worker has been at it, ticking each second; nothing once it is done. */
function Elapsed({ taskId }: { taskId: string }) {
  const start = useBoard((s) => {
    const task = s.board?.tasks[taskId];
    return task && !workerDone(task) ? (task.attempts.at(-1)?.startedAtMs ?? task.createdAtMs) : null;
  });
  const now = useNow(start === null ? null : 1000);
  if (start === null) return null;
  return <span className="text-foreground/50 shrink-0 tabular-nums">{formatDuration(Math.max(0, now - start))}</span>;
}

/** One worker: `[g] Name · what it is at · 1m 2s · +12 −3`. Opens its thread. */
const StripRow = memo(function StripRow({ taskId }: { taskId: string }) {
  const name = useWorkerName(taskId);
  const { setPanel } = useContext(AgentsPanelContext);
  const { ref, first, firstWorking } = useTaskActivityLine(taskId);
  const done = useBoard((s) => {
    const task = s.board?.tasks[taskId];
    return task && workerDone(task) ? workerState(task) : null;
  });
  const stat = useBoard((s) => {
    const task = s.board?.tasks[taskId];
    return task ? workerStat(task, s.board?.diffs) : undefined;
  });
  return (
    <li>
      <div ref={ref}>
      <button
        type="button"
        data-slot="workers-strip-row"
        onClick={() => setPanel(taskId)}
        className="hover:bg-foreground/5 rounded-control min-h-row flex w-full min-w-0 items-center gap-1.5 px-2 text-start text-sm"
      >
        <WorkerGlyph taskId={taskId} className="size-4 shrink-0" />
        <span className="text-foreground/80 max-w-1/3 shrink-0 truncate">{name}</span>
        <span aria-hidden className="text-foreground/40">·</span>
        <span className={cn("min-w-0 flex-1 truncate", done ? "text-foreground/50" : firstWorking ? "shimmer" : "text-foreground/60")}>
          {done ?? first}
        </span>
        <Elapsed taskId={taskId} />
        {stat && <Changes insertions={stat.insertions} deletions={stat.deletions} />}
      </button>
      </div>
    </li>
  );
});

/**
 * The Workers strip on top of the composer (THREAD-UX-PLAN.md §3.5): while workers run, wait,
 * or finished since the user's last message. Collapsed it counts them; open, one row each, and
 * Stop all. Each worker's progress lives here, not under the thread's live line.
 */
export const WorkersStrip: FC<{ conversationId: string }> = ({ conversationId }) => {
  const json = useBoard((s) => {
    const board = s.board;
    if (board?.conversationId !== conversationId) return null;
    const view = workersStrip(Object.values(board.tasks), Object.values(board.requests), board.workerSteps);
    return view ? JSON.stringify(view) : null;
  });
  const view = useMemo(() => (json ? (JSON.parse(json) as ReturnType<typeof workersStrip>) : null), [json]);
  const [open, setOpen] = useState(false);
  const stop = useAction();
  if (!view) return null;
  return (
    <ComposerRailItem label="Workers">
      <div data-slot="workers-strip" data-state={open ? "open" : "closed"} className="group flex flex-col px-1 py-1">
        <div className="flex min-h-row items-center gap-1">
          <button
            type="button"
            aria-expanded={open}
            onClick={() => setOpen(!open)}
            className="hover:text-foreground text-foreground/60 rounded-control flex min-w-0 flex-1 items-center gap-1.5 px-2 py-1 text-start text-sm"
          >
            <span className="flex shrink-0 items-center gap-1">
              {view.rows.slice(0, 4).map((id) => <WorkerGlyph key={id} taskId={id} className="size-4" />)}
            </span>
            <span className="min-w-0 truncate">{stripWords(view)}</span>
            <ChevronRight aria-hidden className={cn(CHEVRON, "-rotate-90 group-data-[state=open]:rotate-90")} />
          </button>
          {view.stoppable && (
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  size="xs"
                  variant="ghost"
                  data-slot="workers-stop-all"
                  disabled={stop.busy}
                  onClick={() => stop.run(() => stopWorkers(conversationId))}
                  className="text-foreground/60 hover:text-foreground"
                >
                  <StopCircle />
                  Stop all
                </Button>
              </TooltipTrigger>
              <TooltipContent side="top">Stop every worker running or waiting to run</TooltipContent>
            </Tooltip>
          )}
        </div>
        {stop.error && <p role="alert" className="text-destructive px-2 text-xs">{stop.error}</p>}
        {open && (
          <ul className="max-h-queue-max flex flex-col overflow-y-auto">
            {view.rows.map((id) => <StripRow key={id} taskId={id} />)}
          </ul>
        )}
      </div>
    </ComposerRailItem>
  );
};
