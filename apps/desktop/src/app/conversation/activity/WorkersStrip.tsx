import { type FC, useMemo, useState } from "react";

import { lastReplies, type StripView, stripWords, workersStrip } from "@/app/conversation/activity/stripView";
import { StripRow } from "@/app/conversation/activity/StripRow";
import { useAction } from "@/app/conversation/useAction";
import { WorkerGlyph } from "@/app/conversation/WorkerChip";
import { Button } from "@/components/controls";
import { ChevronRight, StopCircle } from "@/components/icons";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import { stopWorkers } from "@/state/actions";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";

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
