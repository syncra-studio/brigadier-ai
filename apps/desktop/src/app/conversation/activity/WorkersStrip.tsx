import { type FC, memo, useContext, useMemo, useState } from "react";

import { lastReplies, type StripView, stripWords, workersStrip } from "@/app/conversation/activity/stripView";
import { StopWorker } from "@/app/conversation/activity/StopWorker";
import { Counts } from "@/app/conversation/Counts";
import { RowTime } from "@/app/conversation/RowTime";
import { useAction } from "@/app/conversation/useAction";
import { useTaskActivityLine } from "@/app/conversation/WorkerActivity";
import { AgentsPanelContext, useWorkerName, WorkerGlyph } from "@/app/conversation/WorkerChip";
import { workerDone, workerState } from "@/app/conversation/workerPresentation";
import { workerStat } from "@/app/conversation/WorkerSummary";
import { Button } from "@/components/ui/button";
import { ChevronRight, StopCircle } from "@/components/icons";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { stopWorkers } from "@/state/actions";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";

/** One worker: `[g] Name · what it is at · 1m 2s · +12 −3`, and its Stop while it runs. Opens its thread. */
const StripRow = memo(function StripRow({ taskId }: { taskId: string }) {
  const name = useWorkerName(taskId);
  const { setPanel } = useContext(AgentsPanelContext);
  const { ref, first, firstWorking } = useTaskActivityLine(taskId, false);
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
      <div ref={ref} className="flex min-w-0 items-center">
      <button
        type="button"
        data-slot="workers-strip-row"
        onClick={() => setPanel(taskId)}
        className="hover:bg-foreground/5 rounded-control min-h-row flex min-w-0 flex-1 items-center gap-1.5 px-2 text-start text-sm"
      >
        <WorkerGlyph taskId={taskId} className="size-4 shrink-0" />
        <span className="text-foreground/80 max-w-1/3 shrink-0 truncate">{name}</span>
        <span aria-hidden className="text-foreground/40">·</span>
        <span className={cn("min-w-0 flex-1 truncate", done ? "text-foreground/50" : firstWorking ? "shimmer" : "text-foreground/60")}>
          {done ?? first}
        </span>
        <RowTime taskId={taskId} />
        {stat && stat.insertions + stat.deletions > 0 && <Counts insertions={stat.insertions} deletions={stat.deletions} />}
      </button>
      <StopWorker taskId={taskId} name={name ?? "this worker"} />
      </div>
    </li>
  );
});

/** The Workers strip's view, live: `null` while it has nothing to show. */
export function useWorkersStrip(conversationId: string): StripView | null {
  const items = useApp((s) => s.threads[conversationId]?.items);
  const hasMore = useApp((s) => s.threads[conversationId]?.hasMore ?? false);
  const requests = useBoard((s) => (s.board?.conversationId === conversationId ? s.board.requests : undefined));
  const replies = useMemo(() => lastReplies(items ?? [], requests), [items, requests]);
  const loadedFromMs = useMemo(
    () => (hasMore && items && items.length > 0 ? Math.min(...items.map((message) => message.createdAtMs)) : null),
    [hasMore, items],
  );
  const json = useBoard((s) => {
    const board = s.board;
    if (board?.conversationId !== conversationId) return null;
    const view = workersStrip(Object.values(board.tasks), replies, board.workerSteps, loadedFromMs);
    return view ? JSON.stringify(view) : null;
  });
  return useMemo(() => (json ? (JSON.parse(json) as StripView) : null), [json]);
}

/**
 * The Workers strip, a row of the composer's status slot (THREAD-UX-PLAN.md §3.5,
 * SESSION-FIXES-PLAN.md Q8): while workers run or wait, and until the thread's answer reports
 * those that finished. Collapsed it counts them; open, one row each, and Stop all. Each
 * worker's progress lives here, not under the thread's live line.
 */
export const WorkersStrip: FC<{ conversationId: string; view: StripView }> = ({ conversationId, view }) => {
  const [open, setOpen] = useState(false);
  const stop = useAction();
  return (
    <div data-slot="workers-strip" data-state={open ? "open" : "closed"} className="group flex flex-col px-1 py-1">
      <div className="flex min-h-row items-center gap-1">
        <button
          type="button"
          aria-expanded={open}
          onClick={() => setOpen(!open)}
          className="group/toggle hover:text-foreground text-foreground/60 rounded-control flex min-w-0 flex-1 items-center gap-1.5 px-2 py-1 text-start text-sm"
        >
          <span className="flex shrink-0 items-center gap-1">
            {view.rows.slice(0, 4).map((id) => <WorkerGlyph key={id} taskId={id} className="size-4" />)}
          </span>
          {/* Its chevron right after the words, in a 24px box: right while closed, down open. */}
          <span className="flex min-w-0 items-center">
            <span className="min-w-0 truncate">{stripWords(view)}</span>
            <span aria-hidden className="size-icon-button-sm flex shrink-0 items-center justify-center">
              <ChevronRight
                data-slot="workers-strip-chevron"
                className="text-row-chevron size-chevron group-hover/toggle:text-foreground group-focus-visible/toggle:text-foreground transition-[rotate,color] duration-300 ease-standard group-data-[state=open]:rotate-90 motion-reduce:transition-none"
              />
            </span>
          </span>
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
  );
};
