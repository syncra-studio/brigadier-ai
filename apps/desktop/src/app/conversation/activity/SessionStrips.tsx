import { type FC, type ReactNode, useMemo, useState } from "react";

import { SubagentRow } from "@/app/conversation/activity/StripRow";
import { type SubagentsView, subagentsStrip, subagentWords } from "@/app/conversation/activity/stripView";
import { useAction } from "@/app/conversation/useAction";
import { Button } from "@/components/controls";
import { Agent, ChevronRight, StopCircle } from "@/components/icons";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import { useNow } from "@/hooks/use-now";
import { stopBackgroundTask } from "@/state/actions";
import { useWorkerRows } from "@/state/cliThread";

/** A view kept by its JSON, so the same rows keep the same strip while the clock ticks. */
function useSteady<T>(view: T | null): T | null {
  const json = view ? JSON.stringify(view) : null;
  return useMemo(() => (json ? (JSON.parse(json) as T) : null), [json]);
}

/** The Subagents strip's view, live from the session's rows: `null` while it has nothing to show. */
export function useSubagentsStrip(conversationId: string): SubagentsView | null {
  const rows = useWorkerRows(conversationId);
  const now = useNow(rows.length > 0 ? 1000 : null);
  return useSteady(useMemo(() => subagentsStrip(rows, now), [rows, now]));
}

/**
 * A strip in the Workers strip's look: its icon, what it is about in words, a chevron right after
 * them, and Stop all while something runs; open, its rows.
 */
const Strip: FC<{
  slot: string;
  icon: ReactNode;
  words: string;
  stops: readonly string[];
  stopTip: string;
  onStop: () => Promise<unknown>;
  children: ReactNode;
}> = ({ slot, icon, words, stops, stopTip, onStop, children }) => {
  const [open, setOpen] = useState(false);
  const stop = useAction();
  return (
    <div data-slot={slot} data-state={open ? "open" : "closed"} className="group flex flex-col px-1 py-1">
      <div className="flex min-h-row items-center gap-1">
        <button
          type="button"
          aria-expanded={open}
          onClick={() => setOpen(!open)}
          className="group/toggle hover:text-foreground text-foreground/60 rounded-control flex min-w-0 flex-1 items-center gap-1.5 px-2 py-1 text-start text-sm"
        >
          <span className="flex shrink-0 items-center gap-1">{icon}</span>
          <span className="flex min-w-0 items-center">
            <span className="min-w-0 truncate">{words}</span>
            <span aria-hidden className="size-icon-button-sm flex shrink-0 items-center justify-center">
              <ChevronRight className="text-row-chevron size-chevron group-hover/toggle:text-foreground group-focus-visible/toggle:text-foreground transition-[rotate,color] duration-300 ease-standard group-data-[state=open]:rotate-90 motion-reduce:transition-none" />
            </span>
          </span>
        </button>
        {stops.length > 0 && (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                size="xs"
                variant="ghost"
                data-slot={`${slot}-stop-all`}
                disabled={stop.busy}
                onClick={() => stop.run(onStop)}
                className="text-foreground/60 hover:text-foreground"
              >
                <StopCircle />
                Stop all
              </Button>
            </TooltipTrigger>
            <TooltipContent side="top">{stopTip}</TooltipContent>
          </Tooltip>
        )}
      </div>
      {stop.error && <p role="alert" className="text-destructive px-2 text-xs">{stop.error}</p>}
      {open && <ul className="max-h-queue-max flex flex-col overflow-y-auto">{children}</ul>}
    </div>
  );
};

/**
 * The Subagents strip, under the Workers strip: the session CLI's own agents, each a plain row
 * (`SubagentRow`: its task and time, no model's icon; its transcript on a click, its Stop).
 */
export const SubagentsStrip: FC<{ conversationId: string; view: SubagentsView }> = ({ conversationId, view }) => (
  <Strip
    slot="subagents-strip"
    icon={<Agent aria-hidden className="size-4" />}
    words={subagentWords(view)}
    stops={view.stops}
    stopTip="Stop every subagent still working"
    onStop={() => Promise.all(view.stops.map((id) => stopBackgroundTask(conversationId, id, undefined, view.stopStarts?.[id])))}
  >
    {view.rows.map((id) => <SubagentRow key={id} conversationId={conversationId} taskId={id} />)}
  </Strip>
);
