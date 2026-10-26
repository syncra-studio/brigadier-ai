/**
 * Mockup only, for `composer-rails.tsx`: the session's own subagents and its own background
 * commands as strips above the composer, drawn from fixed data in the Workers strip's own markup
 * (`WorkersStrip`, `StripRow`). Nothing here runs, stops or reads a daemon.
 */
import { type FC, type ReactNode, useState } from "react";

import type { BackgroundState } from "@/app/conversation/backgroundRows";
import { StripRow } from "@/app/conversation/activity/StripRow";
import { stripWords } from "@/app/conversation/activity/stripView";
import { StateIcon, WorkerGlyph } from "@/components/conversation";
import { Button, TooltipIconButton } from "@/components/controls";
import { Agent, ChevronRight, Clock, StopCircle, Terminal } from "@/components/icons";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import { cn } from "@/lib/utils";

/** One subagent or one command, as its row reads. */
export type MockLine = {
  id: string;
  /** What it is: "Explore", "Core tests". */
  name: string;
  /** What it is for, in plain words. */
  doing: string;
  state: "working" | "queued" | "pausing" | "done" | "passed" | "failed";
  /** Its state in a word when it is not at work: "passed", "starts after Desktop build". */
  word?: string;
  time?: string;
  /** The session waits on this one before it goes on. */
  waitedOn?: boolean;
  /** The command itself, on hover. */
  command?: string;
};

export type MockKind = "subagents" | "checks";

const KIND = {
  subagents: { one: "subagent", many: "subagents", heading: "Subagents", icon: Agent, stopTip: "Stop every subagent still working", rowStop: "Stop this subagent" },
  checks: { one: "check", many: "checks", heading: "Checks", icon: Terminal, stopTip: "Stop every check still running", rowStop: "Stop this check" },
} as const;

const ICON_STATE: Record<MockLine["state"], BackgroundState> = {
  working: "checking", queued: "paused", pausing: "waiting", done: "done", passed: "done", failed: "failed",
};

const live = (line: MockLine) => line.state === "working" || line.state === "pausing";

/**
 * "Waiting on Core tests · 2 checks running · 1 passed": what the session waits on first, then
 * the counts, the first naming them, zeros left out (as `stripWords`).
 */
export function mockWords(kind: MockKind, lines: readonly MockLine[]): string {
  const { one, many } = KIND[kind];
  const count = (state: MockLine["state"]) => lines.filter((line) => line.state === state).length;
  const counts = kind === "subagents"
    ? [[count("working"), "working"], [count("done"), "done"], [count("failed"), "failed"]] as const
    : [[count("working") + count("pausing"), "running"], [count("queued"), "queued"], [count("passed"), "passed"], [count("failed"), "failed"]] as const;
  const words = counts
    .filter(([n]) => n > 0)
    .map(([n, word], index) => (index === 0 ? `${n} ${n === 1 ? one : many} ${word}` : `${n} ${word}`));
  const waited = lines.find((line) => line.waitedOn);
  return [waited && `Waiting on ${waited.name}`, ...words].filter(Boolean).join(" · ");
}

/** The leading icon of a strip's line: its kind's icon, so it reads apart from the workers' glyphs. */
const KindIcon: FC<{ kind: MockKind }> = ({ kind }) => {
  const Icon = KIND[kind].icon;
  return <Icon aria-hidden className="size-4 shrink-0" />;
};

/** The toggle the Workers strip has: icons, words, a chevron right after them. */
const Toggle: FC<{ open: boolean; onToggle: () => void; icons: ReactNode; words: string; className?: string }> = ({ open, onToggle, icons, words, className }) => (
  <button
    type="button"
    aria-expanded={open}
    data-state={open ? "open" : "closed"}
    onClick={onToggle}
    className={cn("group/toggle hover:text-foreground text-foreground/60 rounded-control flex min-w-0 flex-1 items-center gap-1.5 px-2 py-1 text-start text-sm", className)}
  >
    <span className="flex shrink-0 items-center gap-1">{icons}</span>
    <span className="flex min-w-0 items-center">
      <span className="min-w-0 truncate">{words}</span>
      <span aria-hidden className="size-icon-button-sm flex shrink-0 items-center justify-center">
        <ChevronRight className={cn("text-row-chevron size-chevron group-hover/toggle:text-foreground transition-[rotate,color] duration-300 ease-standard motion-reduce:transition-none", open && "rotate-90")} />
      </span>
    </span>
  </button>
);

const StopAll: FC<{ kind: MockKind }> = ({ kind }) => (
  <Tooltip>
    <TooltipTrigger asChild>
      <Button size="xs" variant="ghost" className="text-foreground/60 hover:text-foreground">
        <StopCircle />
        Stop all
      </Button>
    </TooltipTrigger>
    <TooltipContent side="top">{KIND[kind].stopTip}</TooltipContent>
  </Tooltip>
);

/** One row: `[state] Name · what it is for · its word · 1m 2s`, and its Stop while it runs. */
export const MockRow: FC<{ kind: MockKind; line: MockLine }> = ({ kind, line }) => (
  <li>
    <div className="flex min-w-0 items-start">
      <button
        type="button"
        data-slot="mock-strip-row"
        title={line.command}
        className="hover:bg-foreground/5 rounded-control flex min-w-0 flex-1 flex-col items-stretch px-2 text-start text-sm"
      >
        <span className="min-h-row flex min-w-0 items-center gap-1.5">
          <span className={cn("flex size-4 shrink-0 items-center justify-center", line.state === "failed" ? "text-destructive" : "text-foreground/60")}>
            {line.state === "pausing" ? <Clock /> : <StateIcon state={ICON_STATE[line.state]} />}
          </span>
          <span className="text-foreground/80 max-w-1/3 shrink-0 truncate">{line.name}</span>
          <span aria-hidden className="text-foreground/40">·</span>
          <span className={cn("min-w-0 flex-1 truncate", line.state === "working" ? "shimmer" : "text-foreground/60")}>{line.doing}</span>
          {line.waitedOn && (
            <span data-slot="mock-waited-on" className="text-foreground/80 bg-foreground/5 rounded-control flex shrink-0 items-center gap-1 px-1.5 text-xs">
              <Clock aria-hidden className="size-3" />
              Session waits on this
            </span>
          )}
          {line.word && <span className={cn("shrink-0", line.state === "failed" ? "text-destructive" : "text-foreground/60", live(line) && "shimmer")}>{line.word}</span>}
          {line.time && <span className="text-foreground/50 shrink-0 tabular-nums">{line.time}</span>}
        </span>
      </button>
      <span className="h-row flex shrink-0 items-center">
        {live(line) && (
          <TooltipIconButton tooltip={KIND[kind].rowStop} aria-label={`Stop ${line.name}`} className="text-foreground/60 hover:text-foreground shrink-0">
            <StopCircle />
          </TooltipIconButton>
        )}
      </span>
    </div>
  </li>
);

/** Variant A: a strip of its own, the Workers strip's look line for line. */
export const MockStrip: FC<{ kind: MockKind; lines: readonly MockLine[]; defaultOpen?: boolean }> = ({ kind, lines, defaultOpen = false }) => {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <div data-slot={`${kind}-strip`} data-state={open ? "open" : "closed"} className="group flex flex-col px-1 py-1">
      <div className="flex min-h-row items-center gap-1">
        <Toggle open={open} onToggle={() => setOpen(!open)} icons={<KindIcon kind={kind} />} words={mockWords(kind, lines)} />
        {lines.some(live) && <StopAll kind={kind} />}
      </div>
      {open && (
        <ul className="max-h-queue-max flex flex-col overflow-y-auto">
          {lines.map((line) => <MockRow key={line.id} kind={kind} line={line} />)}
        </ul>
      )}
    </div>
  );
};

/** The workers' part of variants B and C, from the real rows. */
export type WorkersPart = { conversationId: string; rows: string[]; working: number; waiting: number };

const workerIcons = (workers: WorkersPart) => workers.rows.slice(0, 4).map((id) => <WorkerGlyph key={id} taskId={id} className="size-4" />);
const workerRows = (workers: WorkersPart) => workers.rows.map((id) => <StripRow key={id} conversationId={workers.conversationId} taskId={id} />);

type Part = "workers" | MockKind;

/**
 * Variant B: one line with a part for each kind that has something; a part opens its own list
 * below, one at a time, with its Stop all at the line's end.
 */
export const MockSplitStrip: FC<{
  workers?: WorkersPart | undefined;
  subagents?: readonly MockLine[] | undefined;
  checks?: readonly MockLine[] | undefined;
  defaultOpen?: Part | null;
}> = ({ workers, subagents, checks, defaultOpen = null }) => {
  const [open, setOpen] = useState<Part | null>(defaultOpen);
  const toggle = (part: Part) => setOpen(open === part ? null : part);
  const short = (kind: MockKind, lines: readonly MockLine[]) => {
    const waited = lines.find((line) => line.waitedOn);
    if (waited) return `Waiting on ${waited.name}`;
    const working = lines.filter(live).length;
    const { one, many } = KIND[kind];
    return working > 0 ? `${working} ${working === 1 ? one : many} ${kind === "checks" ? "running" : "working"}` : `${lines.length} ${lines.length === 1 ? one : many}`;
  };
  const stoppable = open === "workers" ? true : open === "subagents" ? subagents?.some(live) : open === "checks" ? checks?.some(live) : false;
  return (
    <div data-slot="split-strip" className="group flex flex-col px-1 py-1">
      <div className="flex min-h-row items-center gap-1">
        {workers && (
          <Toggle open={open === "workers"} onToggle={() => toggle("workers")} icons={workerIcons(workers)} words={stripWords(workers)} className="flex-initial" />
        )}
        {subagents && (
          <Toggle open={open === "subagents"} onToggle={() => toggle("subagents")} icons={<KindIcon kind="subagents" />} words={short("subagents", subagents)} className="flex-initial" />
        )}
        {checks && (
          <Toggle open={open === "checks"} onToggle={() => toggle("checks")} icons={<KindIcon kind="checks" />} words={short("checks", checks)} className="flex-initial" />
        )}
        <span className="flex-1" />
        {open && stoppable && (open === "workers" ? <StopAllWorkers /> : <StopAll kind={open} />)}
      </div>
      {open && (
        <ul className="max-h-queue-max flex flex-col overflow-y-auto">
          {open === "workers" && workers && workerRows(workers)}
          {open === "subagents" && subagents?.map((line) => <MockRow key={line.id} kind="subagents" line={line} />)}
          {open === "checks" && checks?.map((line) => <MockRow key={line.id} kind="checks" line={line} />)}
        </ul>
      )}
    </div>
  );
};

const StopAllWorkers: FC = () => (
  <Tooltip>
    <TooltipTrigger asChild>
      <Button size="xs" variant="ghost" className="text-foreground/60 hover:text-foreground">
        <StopCircle />
        Stop all
      </Button>
    </TooltipTrigger>
    <TooltipContent side="top">Stop every worker running or waiting to run</TooltipContent>
  </Tooltip>
);

/** A group's heading in variant C's list, with its Stop all. */
const heading = (title: string, stop: ReactNode) => (
  <li className="text-foreground/50 flex min-h-row items-center gap-1 px-2 pt-1 text-xs">
    <span className="flex-1">{title}</span>
    {stop}
  </li>
);

/**
 * Variant C: one "live work" line naming all three; open, one list under three headings, each
 * heading with its own Stop all.
 */
export const MockGroupedStrip: FC<{
  workers?: WorkersPart | undefined;
  subagents?: readonly MockLine[] | undefined;
  checks?: readonly MockLine[] | undefined;
  defaultOpen?: boolean;
}> = ({ workers, subagents, checks, defaultOpen = false }) => {
  const [open, setOpen] = useState(defaultOpen);
  const waited = checks?.find((line) => line.waitedOn);
  const words = [
    waited && `Waiting on ${waited.name}`,
    workers && `${workers.rows.length} ${workers.rows.length === 1 ? "worker" : "workers"}`,
    subagents && `${subagents.length} ${subagents.length === 1 ? "subagent" : "subagents"}`,
    checks && `${checks.length} ${checks.length === 1 ? "check" : "checks"}`,
  ].filter(Boolean).join(" · ");
  const icons = <>
    {workers && workerIcons(workers)}
    {subagents && <KindIcon kind="subagents" />}
    {checks && <KindIcon kind="checks" />}
  </>;
  return (
    <div data-slot="grouped-strip" className="group flex flex-col px-1 py-1">
      <div className="flex min-h-row items-center gap-1">
        <Toggle open={open} onToggle={() => setOpen(!open)} icons={icons} words={words} />
      </div>
      {open && (
        <ul className="max-h-queue-max flex flex-col overflow-y-auto">
          {workers && <>{heading(stripWords(workers), <StopAllWorkers />)}{workerRows(workers)}</>}
          {subagents && <>{heading(mockWords("subagents", subagents), subagents.some(live) && <StopAll kind="subagents" />)}{subagents.map((line) => <MockRow key={line.id} kind="subagents" line={line} />)}</>}
          {checks && <>{heading(mockWords("checks", checks), checks.some(live) && <StopAll kind="checks" />)}{checks.map((line) => <MockRow key={line.id} kind="checks" line={line} />)}</>}
        </ul>
      )}
    </div>
  );
};
