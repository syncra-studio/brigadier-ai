import { type FC, memo, type ReactNode, useMemo, useState } from "react";

import { checkLines, checkWords, type StripLine, type StripLines } from "@/app/conversation/activity/sessionTasks";
import { StripRow } from "@/app/conversation/activity/StripRow";
import { type SubagentsView, subagentsStrip, subagentWords } from "@/app/conversation/activity/stripView";
import { useAction } from "@/app/conversation/useAction";
import { StateIcon } from "@/components/conversation";
import { Button, TooltipIconButton } from "@/components/controls";
import { Agent, ChevronRight, Clock, StopCircle, Terminal } from "@/components/icons";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import { useNow } from "@/hooks/use-now";
import type { BackgroundTask, FileEntry } from "@/ipc/generated";
import { formatDuration } from "@/lib/format";
import { cn } from "@/lib/utils";
import { stopBackgroundTask } from "@/state/actions";
import { useCliThread, useWorkerRows } from "@/state/cliThread";
import { openPanelTab } from "@/state/sessionTabs";

const NO_TASKS: readonly BackgroundTask[] = [];
const NO_ENTRIES: readonly FileEntry[] = [];

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

/** The Checks strip's rows, live from the session's background tasks: `null` while it has nothing to show. */
export function useChecksStrip(conversationId: string): StripLines | null {
  const tasks = useCliThread((s) => s.views[conversationId]?.background ?? NO_TASKS);
  const entries = useCliThread((s) => s.views[conversationId]?.entries ?? NO_ENTRIES);
  const now = useNow(tasks.length > 0 ? 1000 : null);
  return useSteady(useMemo(() => {
    const lines = checkLines(tasks, entries, now);
    return lines.lines.length > 0 ? lines : null;
  }, [tasks, entries, now]));
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
 * The Subagents strip, under the Workers strip: the session CLI's own agents, each a worker's
 * strip row (its transcript on a click, what it does now, its Stop).
 */
export const SubagentsStrip: FC<{ conversationId: string; view: SubagentsView }> = ({ conversationId, view }) => (
  <Strip
    slot="subagents-strip"
    icon={<Agent aria-hidden className="size-4" />}
    words={subagentWords(view)}
    stops={view.stops}
    stopTip="Stop every subagent still working"
    onStop={() => Promise.all(view.stops.map((id) => stopBackgroundTask(conversationId, id)))}
  >
    {view.rows.map((id) => <StripRow key={id} conversationId={conversationId} taskId={id} />)}
  </Strip>
);

/**
 * The Checks strip, nearest the composer: the session's own tests, builds and waits. Collapsed it
 * says what the session waits on first and counts the rest.
 */
export const ChecksStrip: FC<{ conversationId: string; view: StripLines }> = ({ conversationId, view }) => {
  const stops = view.stops ?? view.lines.flatMap((line) => line.stops);
  return (
    <Strip
      slot="checks-strip"
      icon={<Terminal aria-hidden className="size-4" />}
      words={checkWords(view)}
      stops={stops}
      stopTip="Stop every check still running"
      onStop={() => Promise.all(stops.map((id) => stopBackgroundTask(conversationId, id)))}
    >
      {view.lines.map((line) => <CheckRow key={line.id} conversationId={conversationId} line={line} />)}
    </Strip>
  );
};

/** How long it has gone, ticking while it runs; how long it ran once it ended. */
function LineTime({ line }: { line: StripLine }) {
  const now = useNow(line.endedAtMs === null ? 1000 : null);
  if (line.state === "queued") return null;
  return <span className="text-foreground/50 shrink-0 tabular-nums">{formatDuration(Math.max(0, (line.endedAtMs ?? now) - line.startedAtMs))}</span>;
}

const ICON = { running: "checking", queued: "paused", done: "done", failed: "failed", stopped: "stopped" } as const;

/**
 * One command: `[state] What it is · what it last printed · its word · 1m 2s`, "Session waits on
 * this" when a wait watches it, and its Stop while it runs. A click opens what it printed.
 */
const CheckRow = memo(function CheckRow({ conversationId, line }: { conversationId: string; line: StripLine }) {
  const stop = useAction();
  const words = (
    <span className="min-h-row flex min-w-0 items-center gap-1.5">
      <span className={cn("flex size-4 shrink-0 items-center justify-center", line.state === "failed" ? "text-destructive" : "text-foreground/60")}>
        <StateIcon state={ICON[line.state]} />
      </span>
      <span className="text-foreground/80 max-w-1/3 shrink-0 truncate">{line.name}</span>
      {line.detail ? (
        <>
          <span aria-hidden className="text-foreground/40">·</span>
          <span className={cn("min-w-0 flex-1 truncate", line.state === "running" ? "shimmer" : "text-foreground/60")}>{line.detail}</span>
        </>
      ) : <span className="flex-1" />}
      {line.waitedOn && (
        <span data-slot="strip-waited-on" className="text-foreground/80 bg-foreground/5 rounded-control flex shrink-0 items-center gap-1 px-1.5 text-xs">
          <Clock aria-hidden className="size-3" />
          Session waits on this
        </span>
      )}
      {line.word && <span className={cn("shrink-0", line.state === "failed" ? "text-destructive" : "text-foreground/60")}>{line.word}</span>}
      <LineTime line={line} />
    </span>
  );
  const row = "rounded-control flex min-w-0 flex-1 flex-col items-stretch px-2 text-start text-sm";
  return (
    <li>
      <div className="flex min-w-0 items-start">
        {line.output ? (
          <button
            type="button"
            data-slot="check-row"
            data-state={line.state}
            title={line.command ?? undefined}
            onClick={() => openPanelTab(conversationId, { type: "output", source: "task", id: line.id, startedAtMs: line.startedAtMs }, line.name)}
            className={cn("hover:bg-foreground/5", row)}
          >
            {words}
          </button>
        ) : (
          <div data-slot="check-row" data-state={line.state} title={line.command ?? undefined} className={row}>{words}</div>
        )}
        <span className="h-row flex shrink-0 items-center">
          {line.stops.length > 0 && (
            <TooltipIconButton
              tooltip="Stop this check"
              aria-label={`Stop ${line.name}`}
              data-slot="check-stop"
              disabled={stop.busy}
              onClick={() => stop.run(() => Promise.all(line.stops.map((id) => stopBackgroundTask(conversationId, id))))}
              className="text-foreground/60 hover:text-foreground shrink-0"
            >
              <StopCircle />
            </TooltipIconButton>
          )}
          {stop.error && <span role="alert" className="text-destructive shrink-0 text-xs">{stop.error}</span>}
        </span>
      </div>
    </li>
  );
});
