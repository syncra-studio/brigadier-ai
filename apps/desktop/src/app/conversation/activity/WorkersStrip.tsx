import { type FC, useMemo, useState } from "react";

import { type StripView, stripWords, workersStrip } from "@/app/conversation/activity/stripView";
import { StripRow } from "@/app/conversation/activity/StripRow";
import { useAction } from "@/app/conversation/useAction";
import { WorkerGlyph } from "@/components/conversation";
import { Button } from "@/components/controls";
import { ChevronRight, StopCircle } from "@/components/icons";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import { useNow } from "@/hooks/use-now";
import { stopWorkers } from "@/state/actions";
import { useWorkerRows } from "@/state/cliThread";

/** The Workers strip's view, live from the session's worker rows: `null` while it has nothing to show. */
export function useWorkersStrip(conversationId: string): StripView | null {
  const rows = useWorkerRows(conversationId);
  // A worker turns quiet after minutes: the clock is read again now and then.
  const now = useNow(rows.length > 0 ? 30_000 : null);
  const json = useMemo(() => {
    const view = workersStrip(rows, now);
    return view ? JSON.stringify(view) : null;
  }, [rows, now]);
  return useMemo(() => (json ? (JSON.parse(json) as StripView) : null), [json]);
}

/**
 * The Workers strip, a row of the composer's status slot (THREAD-UX-PLAN.md §3.5,
 * SESSION-FIXES-PLAN.md Q8): while workers run or wait, those only; finished ones are the
 * context card's. Collapsed it counts them; open, one row each, and Stop all. Each worker's
 * progress lives here, not under the thread's live line.
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
          {view.rows.map((id) => <StripRow key={id} conversationId={conversationId} taskId={id} />)}
        </ul>
      )}
    </div>
  );
};
