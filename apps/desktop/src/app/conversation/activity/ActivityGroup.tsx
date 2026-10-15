import { ChevronRight, Clock, Globe, Stop } from "@openai/apps-sdk-ui/components/Icon";
import { createContext, type ReactNode, useContext, useEffect, useState } from "react";

import type { GroupItem } from "@/app/conversation/activity/group";
import type { StatusHead } from "@/app/conversation/liveStatus";
import { useCalmLabel } from "@/app/conversation/activity/calmLabel";
import { StoppedBy } from "@/app/conversation/activity/stopped";
import { labelParts, stepLabel, type StepWords, summarize } from "@/app/conversation/activity/words";
import { CHEVRON, OPENS, ROW, ROW_DETAIL, ROW_TOGGLE, WORK_ICONS } from "@/components/assistant-ui/elements/activity-row";
import { ThreadActivity } from "@/components/assistant-ui/elements/thread-activity";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { useNow } from "@/hooks/use-now";
import type { ItemStatus } from "@/ipc/generated";
import { formatDuration } from "@/lib/format";
import { cn } from "@/lib/utils";

/**
 * A step's words and how it stands, as a group reads them; a command's exit code when it says;
 * when it started and ended, when that is known.
 */
export type Described = { words: StepWords; status: ItemStatus; exit?: number | null; startedAtMs?: number | undefined; endedAtMs?: number | undefined };

/** After a step ends, the live group still says it this long, so a quick read is seen, not "Thinking". */
export const STEP_LINGERS_MS = 1000;

/** A row inside an opened group: a running command there reads "Running command for 12s". */
const InGroup = createContext(false);

function StepIcon({ words, stopped = false }: { words: StepWords; stopped?: boolean }) {
  const Icon = stopped ? Stop : words.web ? Globe : words.waiting ? Clock : WORK_ICONS[words.kind];
  return <Icon aria-hidden className="size-4 shrink-0" />;
}

/** A step's words in two tones: the verb in the row's, what it acts on a step fainter; white together on hover. */
function Words({ words, label }: { words: StepWords; label: string }) {
  const { verb, object, rest } = labelParts(words, label);
  if (!object) return <>{label}</>;
  return (
    <>
      {verb}
      <span className="text-row-object group-hover:text-foreground group-focus-visible:text-foreground">{object}</span>
      {rest}
    </>
  );
}

/** "Running command for 12s", ticking each second from when it started (or was first seen). */
function RunningFor({ startedAtMs }: { startedAtMs: number | undefined }) {
  const [seen] = useState(() => Date.now());
  const now = useNow(1000);
  const ms = now - (startedAtMs ?? seen);
  return <>{ms < 1000 ? "Running command" : `Running command for ${formatDuration(ms)}`}</>;
}

/** What a step the user's Stop cut short says: "Stopped npm test after 4s", "Stopped reading a.ts". */
export function stoppedLabel(words: StepWords, startedAtMs: number | undefined, atMs: number | null): string {
  const what = (words.kind === "run" || words.kind === "checks") && words.object
    ? words.object
    : words.doing.charAt(0).toLowerCase() + words.doing.slice(1);
  const ms = startedAtMs !== undefined && atMs !== null ? atMs - startedAtMs : 0;
  return ms >= 1000 ? `Stopped ${what} after ${formatDuration(ms)}` : `Stopped ${what}`;
}

/**
 * One step as a row: "Read RequestBlock.tsx", "Ran pnpm test in 41s"; the verb shimmers while it
 * runs. It opens to `detail` when there is one, also while it runs.
 */
export function StepRow({ words, status, exit = null, startedAtMs, detail, suffix, slot }: Described & {
  detail?: ReactNode;
  /** After the words: "in 41s". */
  suffix?: ReactNode;
  slot?: string;
}) {
  const stoppedBy = useContext(StoppedBy);
  // Still running when the user's Stop ended its thread: it was stopped, not running.
  const stopped = status === "inProgress" && stoppedBy !== null;
  const running = status === "inProgress" && !stopped;
  const inGroup = useContext(InGroup);
  // Inside an opened group, a running command says how long it has run; the group's own row names it.
  const ticking = running && inGroup && (words.kind === "run" || words.kind === "checks");
  return (
    // Its Shell box or diff spans the column, flush under the row.
    <ThreadActivity
      data-slot={slot}
      data-kind={words.kind}
      data-step-state={stopped ? "stopped" : running ? "running" : undefined}
      detail={detail}
      detailClassName={cn(ROW_DETAIL, "ps-0")}
    >
      <StepIcon words={words} stopped={stopped} />
      <span className={cn("min-w-0 truncate", running && "shimmer")}>
        {stopped ? (
          <Words words={words} label={stoppedLabel(words, startedAtMs, stoppedBy.atMs)} />
        ) : ticking ? (
          <RunningFor startedAtMs={startedAtMs} />
        ) : running ? (
          stepLabel(words, status, exit)
        ) : (
          <Words words={words} label={stepLabel(words, status, exit)} />
        )}
      </span>
      {suffix && <span className="shrink-0 tabular-nums">{suffix}</span>}
    </ThreadActivity>
  );
}

/** Reads, searches and listings that still run show once done inside an opened group: its row names them. */
const exploring = (described: Described) =>
  described.status === "inProgress" && (described.words.kind === "read" || described.words.kind === "code");

/**
 * Whether the newest step still names the live group: while it runs, and for `STEP_LINGERS_MS`
 * after it ended; it re-renders when that time is up.
 */
function useStepNames(described: Described | undefined, live: boolean): boolean {
  const end = described?.status === "inProgress" ? undefined : described?.endedAtMs;
  // The end whose second is up.
  const [lapsed, setLapsed] = useState<number | null>(null);
  useEffect(() => {
    if (!live || end === undefined) return;
    const timer = window.setTimeout(() => setLapsed(end), Math.max(0, end + STEP_LINGERS_MS - Date.now()));
    return () => window.clearTimeout(timer);
  }, [live, end]);
  return live && (described?.status === "inProgress" || (end !== undefined && lapsed !== end));
}

/**
 * A group of work as one row (THREAD-UX-PLAN.md §3.2). Closed, it sums its steps up ("Read
 * files, ran commands"); while it is the turn's open group, it says the current step
 * ("Reading blocks.ts"), also for a second after that step ended, and with nothing running the
 * turn's live line ("Thinking", the thought's heading, "Waiting for your answer"), so the turn
 * has one live line. Its live words change at most once a second. Opened, it lists each step,
 * also while it runs. A group of one step is that step's own row, except while it says the live
 * line or sums up work a follow-up cut short.
 */
export function ActivityGroup<S>({ items, live, describe, renderStep, reveal, liveLabel = null, summary = false }: {
  items: readonly GroupItem<S>[];
  /** It is the live turn's last group: it says what happens now. */
  live: boolean;
  /** The turn's live line, said by this row unless its step runs (what waits on the user wins even then). */
  liveLabel?: StatusHead | null | undefined;
  /** The user steered a message in after it: it sums its steps up at once ("Ran a command"), still or not. */
  summary?: boolean | undefined;
  describe: (step: S) => Described;
  renderStep: (step: S, key: string) => ReactNode;
  /** The step a jump asks to show, if it is one of these: the group opens to it. */
  reveal?: ((step: S) => boolean) | undefined;
}) {
  const [open, setOpen] = useState(false);
  // A jump to one of its steps opens it, once per jump.
  const holds = !!reveal && items.some((item) => reveal(item.step));
  const [revealed, setRevealed] = useState(false);
  if (holds !== revealed) {
    setRevealed(holds);
    if (holds) setOpen(true);
  }
  const described = items.map((item) => describe(item.step));
  const current = described.at(-1);
  const first = described[0];
  const stepNames = useStepNames(current, !summary && live);
  // The live line, unless the step running (or just ended) says it better; waiting on the user always wins.
  const line = live && liveLabel && (!stepNames || liveLabel.tone === "needsYou") ? liveLabel : null;
  const oneStep = !summary && (live || described.length === 1);
  const head = oneStep ? current : first;
  const naming = stepNames && !line && current !== undefined;
  const label = line
    ? line.text
    : !current
      ? ""
      : naming
        ? current.words.doing
        : oneStep
          ? stepLabel(current.words, current.status, current.exit)
          : summarize(described.map((step) => step.words));
  // Live words change at most once a second; a summary shows at once.
  const calm = useCalmLabel(label, live && !summary);
  const [only] = items;
  if (!only) return null;
  if (items.length === 1 && !line && !summary) return <>{renderStep(only.step, only.key)}</>;
  return (
    <Collapsible data-slot="work-group" open={open} onOpenChange={setOpen}>
      <CollapsibleTrigger className={cn(ROW, ROW_TOGGLE)}>
        {/* The live line ("Thinking") is no step's: it shows without a step's icon. */}
        {head && !(line && calm.shown === line.text) && <StepIcon words={head.words} />}
        {/* A live group's step changes in place: its words cross-fade, with no jump. */}
        <span
          key={live ? calm.shown : undefined}
          data-slot={line ? "group-live-line" : undefined}
          data-tone={line?.tone}
          data-pending={calm.pending || undefined}
          data-lingers={(naming && current.status !== "inProgress") || undefined}
          className={cn(
            "min-w-0 truncate",
            live && "animate-in fade-in duration-160 motion-reduce:animate-none",
            (naming || (line && line.tone !== "still")) && "shimmer",
          )}
        >
          {current && oneStep && !naming && !line ? <Words words={current.words} label={calm.shown} /> : calm.shown}
        </span>
        <ChevronRight aria-hidden className={CHEVRON} />
      </CollapsibleTrigger>
      <CollapsibleContent className={OPENS}>
        {/* Up to 224px of rows 4px apart, then it scrolls, its edges fading while there is more. */}
        <div className="max-h-group-list scroll-edge-fade gap-activity-row-gap flex min-w-0 flex-col overflow-y-auto pt-1 [--spacing-scroll-fade-bottom:var(--spacing-group-fade)] [--spacing-scroll-fade-top:var(--spacing-group-fade)]">
          <InGroup.Provider value>
            {items.map((item, index) => (described[index] && exploring(described[index]) ? null : renderStep(item.step, item.key)))}
          </InGroup.Provider>
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}
