import { ThinkingLine } from "@/app/conversation/ThinkingLine";
import { ProviderTransitionDivider } from "@/app/conversation/activity/ProviderTransition";
import { placeSwitches, transitionSteps } from "@/app/conversation/providerTransitions";
import { useBoard } from "@/state/board";
import {
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { ActionRow, exitOf } from "@/app/side-panel/ActionRow";
import { EntryView } from "@/app/side-panel/EntryView";
import { withSeams } from "@/app/side-panel/workerSeams";
import { WorkerBrief } from "@/app/side-panel/WorkerBrief";
import { pickedRow } from "@/app/conversation/activity/Briefs";
import { rowFinished, workerStateOf } from "@/app/conversation/activity/workerState";
import { ActivityGroup, StepRow } from "@/app/conversation/activity/ActivityGroup";
import { workerActivity, workerCliNotice } from "@/app/conversation/activity/group";
import { StoppedBy } from "@/app/conversation/activity/stopped";
import { type ActionItem, isSubagentStart, itemCall, stepWords } from "@/app/conversation/activity/words";
import { ranFor } from "@/app/conversation/activity/LeadStep";
import { AgentsPanelContext } from "@/app/conversation/WorkerChip";
import { foldTranscript, type ThreadEntry, threadEntries } from "@/components/conversation";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/layout";
import { ChevronRight } from "@/components/icons";
import { useNow } from "@/hooks/use-now";
import type { ThreadTurn, ThreadView, WorkerRow } from "@/ipc/generated";
import { formatDuration } from "@/lib/format";
import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";
import { request } from "@/ipc/client";
import { isNativeWorker, type NativeWorker, subagentPanel } from "@/state/nativeWorkers";
import { loadWorkerThread, useCliThread, useWorkerThreads } from "@/state/cliThread";

/**
 * A worker's thread, read only: its own CLI's session files (`workerThread`), with its brief and
 * early steps folded above. It has no composer: the user answers a worker in its session, which
 * relays it (G19.Q6).
 */

/** How often a working worker's thread is read again while its panel is open. */
export const WORKER_THREAD_EVERY_MS = 1_500;

/** What the worker does right now, as the thread's last line, when no row of its says it. */
function liveLabel(entries: readonly ThreadEntry[]): string | null {
  const last = entries.at(-1);
  // A running action says so on its own row.
  if (last?.kind === "actions" && last.items.at(-1)?.status === "inProgress") return null;
  // A reply shows from its first word: until then it still thinks.
  if (last?.kind === "item" && last.item.kind === "message" && last.item.streaming && last.item.text.trim()) return null;
  if (last?.kind === "item" && last.item.kind === "reasoning" && last.item.streaming && last.item.text.trim()) return null;
  return "Thinking";
}

/**
 * How long worker `row` worked: its turns' time by its CLI's timestamps, so the waits between its
 * messages (and an app restart) don't count; from its start to its CLI's last word without them.
 */
export function workedMs(row: WorkerRow, turns: readonly ThreadTurn[], done: boolean, now: number): number {
  const end = done ? (row.lastEventAtMs ?? now) : now;
  const timed = turns.filter((turn) => turn.startedAtMs !== null);
  if (timed.length === 0) return Math.max(0, end - row.startedAtMs);
  // A turn with no end is the one at work, or one cut off: up to its CLI's last word.
  return timed.reduce((sum, turn) => sum + Math.max(0, (turn.endedAtMs ?? end) - (turn.startedAtMs ?? 0)), 0);
}

/**
 * How long a worker had worked by `atMs` (its Stop): its turns' time up to then, the sum its
 * panel's header says, so the lead's "Stopped … after" row reads the same number.
 */
export function workedBy(turns: readonly ThreadTurn[], atMs: number): number {
  return turns.reduce((sum, turn) => (turn.startedAtMs !== null && turn.startedAtMs < atMs ? sum + Math.max(0, Math.min(turn.endedAtMs ?? atMs, atMs) - turn.startedAtMs) : sum), 0);
}

/**
 * "Working for 1m 2s" while it works, "Worked for 3m 10s" once finished, "Ended after 3m 10s"
 * when it was ended, over a hairline.
 */
function WorkHeader({ row, turns }: { row: WorkerRow; turns: readonly ThreadTurn[] }) {
  const clock = useNow(60_000);
  const done = rowFinished(row, clock);
  const now = useNow(done ? null : 1000);
  const took = formatDuration(workedMs(row, turns, done, now));
  const stopped = workerStateOf(row, clock).kind === "stopped";
  return (
    <div data-slot="worker-work-header" className="text-foreground/50 border-border border-b pb-2 text-sm tabular-nums">
      {stopped ? `Ended after ${took}` : done ? `Worked for ${took}` : `Working for ${took}`}
    </div>
  );
}

/**
 * A worker's thread as its CLI keeps it, read when its panel opens, again whenever its row
 * changes, and every 1.5 s while it works.
 */
function useWorkerThread(conversationId: string, row: WorkerRow): { view: ThreadView | null; error: string | null } {
  const view = useWorkerThreads((s) => s.views[row.id] ?? null);
  const [error, setError] = useState<string | null>(null);
  // Any change of its row (an event of its CLI, a wait, its git) reads it again.
  const changed = JSON.stringify(row);
  useEffect(() => {
    let current = true;
    const read = () =>
      loadWorkerThread(conversationId, row.id).then(
        () => current && setError(null),
        (cause: unknown) => current && setError(cause instanceof Error ? cause.message : String(cause)),
      );
    void read();
    const timer = row.working ? window.setInterval(() => void read(), WORKER_THREAD_EVERY_MS) : null;
    return () => {
      current = false;
      if (timer !== null) window.clearInterval(timer);
    };
    // Its row's every change reads it again.
    // oxlint-disable-next-line react/exhaustive-effect-dependencies
  }, [conversationId, row.id, row.working, changed]);
  return { view, error };
}

/** A message the worker was sent, among its thread's entries. */
const received = (entry: ThreadEntry) => entry.kind === "item" && entry.item.kind === "message" && entry.item.role === "user";

export function WorkerThread({ conversationId, row }: { conversationId: string; row: WorkerRow }) {
  return isNativeWorker(row)
    ? <NativeWorkerThread conversationId={conversationId} row={row} />
    : <BrigadierWorkerThread conversationId={conversationId} row={row} />;
}

function BrigadierWorkerThread({ conversationId, row }: { conversationId: string; row: WorkerRow }) {
  const { view, error } = useWorkerThread(conversationId, row);
  return <WorkerTranscript row={row} view={view} error={error} />;
}

/**
 * A subagent's thread: its entries in its parent's thread (its session's, or a worker's), as that
 * thread is kept (read, live news, read again at its turn's end), so its last words show even
 * when they come after its call ended. Its entries come only from its parent's files, which live
 * news doesn't carry: they are read on open, every 1.5 s while it works and as it ends, and the
 * fuller of the two shows (a transcript only grows).
 */
function NativeWorkerThread({ conversationId, row }: { conversationId: string; row: NativeWorker }) {
  const { callId, owner } = row.native;
  // A worker's own subagent: its entries are in that worker's thread (`workerThread`).
  const ofWorker = owner !== conversationId;
  const [polled, setPolled] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // A string, so a change of the parent's thread that is not the subagent's repaints nothing.
  const lead = useCliThread((s) => {
    const parent = ofWorker ? undefined : s.views[conversationId];
    return parent ? JSON.stringify(ownEntries(parent, callId)) : null;
  });
  const worker = useWorkerThreads((s) => {
    const parent = ofWorker ? s.views[owner] : undefined;
    return parent ? JSON.stringify(ownEntries(parent, callId)) : null;
  });
  const json = lead ?? worker;
  // Read once on open and once as it ends too: its lead's turn may run on long after it, and
  // only that turn's end reads the session's thread again (D50).
  useEffect(() => {
    let current = true;
    // One read at a time: a slow one never piles up behind the timer.
    let reading = false;
    const read = () => {
      if (reading) return;
      reading = true;
      return (ofWorker ? loadWorkerThread(conversationId, owner).then(() => null) : request({ method: "threadView", conversationId }).then(({ view: parent }) => JSON.stringify(ownEntries(parent, callId))))
        .then(
          (entries) => {
            if (!current) return;
            if (entries !== null) setPolled(entries);
            setError(null);
          },
          (cause: unknown) => current && setError(cause instanceof Error ? cause.message : String(cause)),
        )
        .finally(() => (reading = false));
    };
    void read();
    if (!row.working) return () => void (current = false);
    const timer = window.setInterval(() => void read(), WORKER_THREAD_EVERY_MS);
    return () => {
      current = false;
      window.clearInterval(timer);
    };
  }, [conversationId, ofWorker, owner, callId, row.working]);
  const view = useMemo<ThreadView | null>(() => {
    const held = json === null ? null : (JSON.parse(json) as ThreadView["entries"]);
    const read = polled === null ? null : (JSON.parse(polled) as ThreadView["entries"]);
    const entries = read && (!held || read.length > held.length) ? read : held;
    return entries ? { entries, turns: [] } : null;
  }, [json, polled]);
  return <WorkerTranscript row={row} view={view} error={view ? null : error} native />;
}

/** A subagent's own entries in its parent's thread `parent`. */
function ownEntries(parent: ThreadView, callId: string): ThreadView["entries"] {
  return parent.entries.filter((entry) => entry.parent === callId);
}

/** A worker's own subagent's call, as the lead's same row reads (D96): it opens its transcript. */
function SubagentActionRow({ item, owner, view }: { item: Extract<ActionItem, { kind: "tool" }>; owner: string; view: ThreadView | null }) {
  const { setPanel } = useContext(AgentsPanelContext);
  const call = itemCall(item);
  const entry = view?.entries.find(({ event, parent }) => !parent && event.type === "toolCall" && `tool:${event.itemId}` === item.key);
  const callId = entry?.event.type === "toolCall" ? entry.event.itemId : null;
  const ran = item.status === "inProgress" || entry?.atMs === undefined ? undefined : (entry.updatedAtMs ?? entry.atMs) - entry.atMs;
  return <StepRow words={stepWords(call)} status={call.status} trail={ranFor(ran)} onOpen={callId === null ? undefined : () => setPanel(subagentPanel(owner, callId))} />;
}

function WorkerTranscript({ row, view, error, native = false }: { row: WorkerRow; view: ThreadView | null; error: string | null; native?: boolean }) {
  const clock = useNow(60_000);
  const state = workerStateOf(row, clock);
  const working = row.working;
  const failed = state.kind === "failed";
  // Its own items, not its subagents': their Agent call's row stands for them.
  const items = useMemo(
    () => foldTranscript((view ? withSeams(view) : []).filter((entry) => native || !entry.parent).map((entry, index) => ({ streamSeq: index, atMs: entry.atMs ?? 0, event: entry.event }))).items,
    [view, native],
  );
  const all = useMemo(() => threadEntries(items).filter((entry) => !workerCliNotice(entry, failed)), [items, failed]);
  // Its first message is its brief, as the thread's first bubble.
  const briefAt = all.findIndex(received);
  const brief = briefAt >= 0 ? all[briefAt] : undefined;
  const entries = useMemo(() => all.filter((_, index) => index !== briefAt), [all, briefAt]);
  const activity = workerActivity(entries);
  const switches = useWorkerSwitches(row.id, entries);
  const picked = useBoard((s) => pickedRow(s, row.id));
  // As the lead's work does: all of it in view while it works (or was stopped), its history
  // folded once it is over, so no row moves into the fold while it is watched.
  const previous = working || state.kind === "stopped" ? [] : activity.slice(0, Math.max(0, activity.length - 3));
  const previousCount = previous.reduce((count, item) => count + (item.type === "group" ? item.items.length : 1), 0);
  const recent = activity.slice(previous.length);
  const last = entries.at(-1);
  // A thought with no words yet is the live line's "Thinking".
  const thinking = working && last?.kind === "item" && last.item.kind === "reasoning" && last.item.streaming && last.item.text.trim() ? last.item : null;
  const now = working ? liveLabel(entries) : null;
  // The newest group's own row says the live line, one step or several, as the lead's thread does.
  const nowInGroup = !!now && activity.at(-1)?.type === "group";
  const show = (item: (typeof activity)[number], index: number) =>
    item.type === "group" ? (
      <ActivityGroup
        key={`group:${item.key}`}
        items={item.items}
        live={working && index === activity.length - 1}
        liveLabel={now && nowInGroup && index === activity.length - 1 ? { text: now, tone: "busy" } : null}
        describe={(step) => {
          const call = itemCall(step);
          return { words: stepWords(call), status: call.status, exit: exitOf(step) };
        }}
        renderStep={(step, key) => !native && step.kind === "tool" && isSubagentStart(step.name)
          ? <SubagentActionRow key={key} item={step} owner={row.id} view={view} />
          : <ActionRow key={key} item={step} />}
      />
    ) : item.entry.kind === "item" && switches.at.has(item.entry.item.key) ? (
      <ProviderTransitionDivider key={item.entry.item.key} transition={switches.at.get(item.entry.item.key)!} own />
    ) : (
      <EntryView key={item.entry.kind === "actions" ? item.entry.key : item.entry.item.key} entry={item.entry} sent={[]} />
    );
  const [open, setOpen] = useState(false);

  // A live one follows new output while the view is scrolled to the bottom.
  const scrollRef = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);
  useLayoutEffect(() => {
    const element = scrollRef.current;
    if (!element || entries.length === 0) return;
    if (pinned.current) element.scrollTop = element.scrollHeight;
  }, [entries]);

  return (
    <div
      ref={scrollRef}
      data-slot="worker-thread"
      data-selectable
      className="min-h-0 min-w-0 flex-1 overflow-y-auto px-4 pb-6 wrap-anywhere"
      onScroll={(event) => {
        const element = event.currentTarget;
        pinned.current =
          element.scrollHeight - element.scrollTop - element.clientHeight < tokenPx("--spacing-row");
      }}
    >
      {/* An action still running when the user stopped the worker reads "Stopped …". */}
      <StoppedBy.Provider value={state.kind === "stopped" ? { atMs: row.lastEventAtMs ?? row.startedAtMs } : null}>
      <div className="max-w-thread mx-auto min-w-0 gap-activity flex flex-col pt-5">
        {brief?.kind === "item" && brief.item.kind === "message" && <WorkerBrief text={brief.item.text} />}
        {/* Clicking its start row opens this panel, so the row's pick is said here too (goal 12). */}
        {picked && <p data-slot="worker-picked" className="text-muted-foreground text-sm">{picked}</p>}
        <WorkHeader row={row} turns={view?.turns ?? []} />
        {previousCount > 0 && <Collapsible open={open} onOpenChange={setOpen}>
          <CollapsibleTrigger className="text-foreground/50 border-border hover:text-foreground flex min-h-7 w-full items-center gap-1 border-b text-start text-sm">
            {previousCount} previous {previousCount === 1 ? "message" : "messages"}
            <ChevronRight aria-hidden className={cn("size-3", open ? "-rotate-90" : "rotate-90")} />
          </CollapsibleTrigger>
          <CollapsibleContent className="gap-activity flex flex-col py-4">
            {previous.map((item, index) => show(item, index))}
          </CollapsibleContent>
        </Collapsible>}
        {error && (
          <p role="alert" className="text-destructive text-xs">
            {error}
          </p>
        )}
        {native && !view && !error && <p className="text-muted-foreground text-sm">Loading transcript…</p>}
        {native && view && all.length === 0 && !error && <p className="text-muted-foreground text-sm">The CLI has not made this agent’s transcript available.</p>}
        {recent.map((item, index) => show(item, previous.length + index))}
        {switches.rest.map((transition) => <ProviderTransitionDivider key={transition.id} transition={transition} own />)}
        {thinking && <ThinkingLine text={thinking.text} />}
        {now && !nowInGroup && <div className="shimmer truncate text-sm motion-reduce:animate-none">{now}</div>}
        {/* What it waits for, from its row: the user (in its session), or a limit's reset. */}
        {(state.kind === "needsYou" || state.kind === "limit") && <p data-slot="worker-waits" className="text-foreground/65 text-sm">{state.word}</p>}
      </div>
      </StoppedBy.Provider>
    </div>
  );
}

/** Its confirmed switches (goal 1), each once, placed by `placeSwitches`. */
function useWorkerSwitches(taskId: string, entries: readonly ThreadEntry[]) {
  const steps = useBoard((s) => s.board?.orchestratorSteps);
  return useMemo(() => placeSwitches(transitionSteps(steps ?? []).flatMap((step) =>
    step.kind.type === "providerTransition" && step.kind.taskId === taskId ? [step.kind] : []), entries), [steps, taskId, entries]);
}
