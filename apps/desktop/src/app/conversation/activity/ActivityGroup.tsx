import { type ReactNode, useContext, useState } from "react";

import type { GroupItem } from "@/app/conversation/activity/group";
import type { StatusHead } from "@/app/conversation/liveStatus";
import { useCalmLabel } from "@/app/conversation/activity/calmLabel";
import { GroupStepList } from "@/app/conversation/activity/GroupStepList";
import { InFold, InGroup, LeadPaused } from "@/app/conversation/activity/groupContext";
import { StoppedBy } from "@/app/conversation/activity/stopped";
import { useFindSearching } from "@/app/conversation/ThreadFind";
import { labelParts, stepLabel, type StepWords, summarize, summaryLead } from "@/app/conversation/activity/words";
import { CHEVRON, OPENS, ROW, ROW_DETAIL, ROW_TOGGLE, RowOpens, StepIcon, ThreadActivity, useKeptState } from "@/components/conversation";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/layout";
import { ChevronRightAlt, Tasks } from "@/components/icons";
import { useNow } from "@/hooks/use-now";
import type { ItemStatus } from "@/ipc/generated";
import { formatDuration } from "@/lib/format";
import { cn } from "@/lib/utils";

/**
 * A step's words and how it stands, as a group reads them; a command's exit code when it says;
 * when it started and ended, when that is known.
 */
export type Described = {
  words: StepWords; status: ItemStatus; exit?: number | null; startedAtMs?: number | undefined; endedAtMs?: number | undefined;
  /** It failed only because the user's Stop ended it: it reads stopped, not failed. */
  stopped?: boolean | undefined;
};

/** A step's words in two tones: the verb in the row's, what it acts on a step fainter; white together on hover. */
function Words({ words, label, objectNode }: { words: StepWords; label: string; objectNode?: ReactNode }) {
  const { verb, object, rest } = labelParts(words, label);
  if (!object) return <>{label}</>;
  return (
    <>
      {verb}
      {objectNode ?? <span className="text-row-object group-hover:text-foreground group-focus-visible:text-foreground">{object}</span>}
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
  const what = words.kind === "run" && words.object
    ? words.object
    : words.doing.charAt(0).toLowerCase() + words.doing.slice(1);
  const ms = startedAtMs !== undefined && atMs !== null ? atMs - startedAtMs : 0;
  return ms >= 1000 ? `Stopped ${what} after ${formatDuration(ms)}` : `Stopped ${what}`;
}

/**
 * One step as a row: "Read RequestBlock.tsx", "Ran pnpm test" ("in 41s" inside an opened group); the verb shimmers while it
 * runs. It opens to `detail` when there is one, also while it runs.
 */
export function StepRow({ words: said, status, exit = null, startedAtMs, endedAtMs, stopped: stoppedCall = false, detail, suffix, slot, objectNode, trail }: Described & {
  detail?: ReactNode;
  /** After the words: "in 41s". */
  suffix?: ReactNode;
  slot?: string;
  /** What the step acts on, drawn in place of its words' object (an edited file's link). */
  objectNode?: ReactNode;
  /** After the words, in and out of a group: an edit's "+7 −2". */
  trail?: ReactNode;
}) {
  const [open, setOpen] = useKeptState("open", false);
  // In a fold of its repeats, a call names what it was given: the fold's row already says the tool.
  const given = useContext(InFold) ? said.given : undefined;
  const words = given ? { ...said, doing: `${said.doing} ${given}`, done: `${said.done} ${given}`, object: given } : said;
  // An edit opened to its diff, whose card says its counts: the row keeps the file's name, so it
  // still says which file once the card has scrolled away or several are open (#175).
  const opened = open && words.kind === "edit" && status === "completed" && words.object;
  const stoppedBy = useContext(StoppedBy);
  const paused = useContext(LeadPaused) && status === "inProgress";
  // Still running when the user's Stop ended its thread, or failing as it did: it was stopped,
  // not running or failed.
  const stopped = (status === "inProgress" && stoppedBy !== null) || stoppedCall;
  const running = status === "inProgress" && !stopped && !paused;
  const inGroup = useContext(InGroup);
  // Inside an opened group, a running command says how long it has run; the group's own row names it.
  const ticking = running && inGroup && words.kind === "run";
  return (
    // Its Shell box or diff spans the column, flush under the row.
    <ThreadActivity
      data-slot={slot}
      data-kind={words.kind}
      data-step-state={stopped ? "stopped" : paused ? "paused" : running ? "running" : status === "declined" ? "declined" : undefined}
      detail={detail}
      onOpenChange={setOpen}
      // In a group its detail doesn't scroll on its own: the group's list scrolls it, a diff's header kept in view.
      detailClassName={cn(ROW_DETAIL, "ps-0", inGroup && "max-h-none overflow-visible")}
    >
      <StepIcon words={words} stopped={stopped} />
      <span className={cn("min-w-0 truncate", running && "shimmer")}>
        {stopped ? (
          <Words words={words} label={stoppedLabel(words, startedAtMs, stoppedCall ? (endedAtMs ?? null) : (stoppedBy?.atMs ?? null))} />
        ) : paused ? (
          <Words words={words} label={`Paused · ${words.doing}`} />
        ) : ticking ? (
          <RunningFor startedAtMs={startedAtMs} />
        ) : running ? (
          stepLabel(words, status, exit)
        ) : (
          <Words words={words} label={stepLabel(words, status, exit)} objectNode={objectNode} />
        )}
      </span>
      {/* "in 41s" only on a row inside an opened group: a row on its own says what it did. */}
      {suffix && inGroup && <span className="shrink-0 tabular-nums">{suffix}</span>}
      {/* Opened, an edit's diff card says its counts: the row reads "Edited math.js" alone. */}
      {!opened && trail}
    </ThreadActivity>
  );
}

/**
 * A group of work as one row (THREAD-UX-PLAN.md §3.2). Closed, it sums its steps up ("Read
 * files, ran commands"); while it is the turn's open group, it says the step running now (the
 * newest of those running together, "Reading blocks.ts") and with nothing running the turn's
 * live line ("Thinking", the thought's heading, "Waiting for your answer"), so the turn has one
 * live line. A live label shows at least a second before the next takes its place; the summary
 * shows at once. Opened, it lists each step, also while it runs. While the turn works
 * (`working`) every run of steps is such a row, one step too, so a row never turns into another;
 * in a turn done, a group of one step is that step's own row, unless it sums up work a follow-up
 * cut short.
 */
export function ActivityGroup<S>({ items, live, working, describe, renderStep, reveal, liveLabel = null, summary: steeredAfter = false }: {
  items: readonly GroupItem<S>[];
  /** It is the live turn's last group: it says what happens now. */
  live: boolean;
  /**
   * Its turn still works (its newest group or not): it stays a group's row, one step too. Not
   * given (a worker's thread), or paused for quota, a lone step is its own row except while it
   * says the live line.
   */
  working?: boolean | undefined;
  /** The turn's live line, said by this row unless its step runs (what waits on the user wins even then). */
  liveLabel?: StatusHead | null | undefined;
  /** The user steered a message in after it: it sums its steps up at once ("Ran a command"), once none still runs. */
  summary?: boolean | undefined;
  describe: (step: S) => Described;
  renderStep: (step: S, key: string) => ReactNode;
  /** The step a jump asks to show, if it is one of these: the group opens to it. */
  reveal?: ((step: S) => boolean) | undefined;
}) {
  const [open, setKept] = useKeptState("open", false);
  const setOpen = setKept;
  // While Find searches, closed it keeps its rows, hidden, to be found.
  const searched = useFindSearching();
  // A lone step's row the user opened: swapping it for the live line would close it.
  const [rowOpen, setRowOpen] = useKeptState("row-open", false);
  // A jump to one of its steps opens it, once per jump.
  const holds = !!reveal && items.some((item) => reveal(item.step));
  const [revealed, setRevealed] = useState(false);
  if (holds !== revealed) {
    setRevealed(holds);
    if (holds) setKept(true);
  }
  const paused = useContext(LeadPaused);
  const described = items.map((item) => describe(item.step));
  // A follow-up that joined is read once the command ends (#437): a step still running keeps its words (#445).
  const summary = steeredAfter && described.every((step) => step.status !== "inProgress");
  // The step running now (the newest of those running together), else the last.
  const current = described.findLast((step) => step.status === "inProgress") ?? described.at(-1);
  const first = described[0];
  const stepNames = !paused && !summary && live && current?.status === "inProgress";
  // The live line, unless the step running (or just ended) says it better; waiting on the user always wins.
  const line = live && liveLabel && (!stepNames || liveLabel.tone === "needsYou") ? liveLabel : null;
  const oneStep = !summary && (live || described.length === 1);
  // A summary's icon is its first segment's ("Ran commands, read files" → a terminal).
  const head = oneStep ? current?.words : (summaryLead(described.map((step) => step.words)) ?? first?.words);
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
  // The live line shows once the label before it has stood its second.
  const saysLine = !!line && calm.shown === line.text;
  const [only] = items;
  if (!only) return null;
  // Paused for quota, a lone step keeps its own row, which says it is paused.
  const ownRow = working === undefined || paused ? !line || rowOpen : !working || rowOpen;
  if (items.length === 1 && ownRow && !summary) {
    return <RowOpens.Provider value={setRowOpen}>{renderStep(only.step, only.key)}</RowOpens.Provider>;
  }
  const list = <GroupStepList items={items} described={described} renderStep={renderStep} />;
  return (
    <Collapsible data-slot="work-group" open={open} onOpenChange={setOpen}>
      <CollapsibleTrigger className={cn(ROW, ROW_TOGGLE)}>
        {/* The live line ("Thinking") is no step's: it shows without a step's icon, but a
            question waiting for its answer has a checklist. */}
        {saysLine
          ? line.waitsOn === "answer" && <Tasks aria-hidden data-icon="tasks" className="size-4 shrink-0" />
          : head && <StepIcon words={head} />}
        {/* A live group's step changes in place, with no jump and no fade: a fade-in's first
            frame is invisible, and WebKit can leave it there, and the fade's animation would
            replace the shimmer's. Keyed, a new label's sweep starts over. */}
        <span
          key={live ? calm.shown : undefined}
          data-slot={saysLine ? "group-live-line" : undefined}
          data-tone={saysLine ? line.tone : undefined}
          data-pending={calm.pending || undefined}
          className={cn(
            "min-w-0 truncate",
            (naming || (line && line.tone !== "still")) && "shimmer",
            line?.waitsOn === "approval" && "shimmer-quiet",
          )}
        >
          {current && oneStep && !naming && !line ? <Words words={current.words} label={calm.shown} /> : calm.shown}
        </span>
        <ChevronRightAlt aria-hidden className={CHEVRON} />
      </CollapsibleTrigger>
      <CollapsibleContent className={OPENS}>{list}</CollapsibleContent>
      {/* Closed while Find searches: its rows kept, inert, with no height and nothing shown, so
          Find counts them; going to one opens the group. */}
      {!open && searched && (
        <div data-slot="work-group-kept" inert className="invisible h-0 overflow-hidden">
          {list}
        </div>
      )}
    </Collapsible>
  );
}
