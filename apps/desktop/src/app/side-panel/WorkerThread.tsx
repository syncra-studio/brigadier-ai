import { ThinkingLine } from "@/app/conversation/ThinkingLine";
import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { ActionRow, exitOf } from "@/app/side-panel/ActionRow";
import { EntryView } from "@/app/side-panel/EntryView";
import { WorkerBrief } from "@/app/side-panel/WorkerBrief";
import { rowFinished, workerStateOf } from "@/app/conversation/activity/workerState";
import { ActivityGroup } from "@/app/conversation/activity/ActivityGroup";
import { workerActivity, workerCliNotice } from "@/app/conversation/activity/group";
import { StoppedBy } from "@/app/conversation/activity/stopped";
import { itemCall, stepWords } from "@/app/conversation/activity/words";
import { foldTranscript, type ThreadEntry, threadEntries } from "@/components/conversation";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/layout";
import { ChevronRight } from "@/components/icons";
import { useNow } from "@/hooks/use-now";
import type { ThreadView, WorkerRow } from "@/ipc/generated";
import { formatDuration } from "@/lib/format";
import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";
import { loadWorkerThread, useWorkerThreads } from "@/state/cliThread";

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
  if (last?.kind === "item" && (last.item.kind === "message" || last.item.kind === "reasoning") && last.item.streaming && last.item.text.trim()) return null;
  return "Thinking";
}

/**
 * "Working for 1m 2s" while it works, "Worked for 3m 10s" once finished, "Stopped after 3m 10s"
 * when it was stopped, over a hairline: from its start to its CLI's last word.
 */
function WorkHeader({ row }: { row: WorkerRow }) {
  const clock = useNow(60_000);
  const done = rowFinished(row, clock);
  const now = useNow(done ? null : 1000);
  const took = formatDuration(Math.max(0, (done ? (row.lastEventAtMs ?? now) : now) - row.startedAtMs));
  const stopped = workerStateOf(row, clock).kind === "stopped";
  return (
    <div data-slot="worker-work-header" className="text-foreground/50 border-border border-b pb-2 text-sm tabular-nums">
      {stopped ? `Stopped after ${took}` : done ? `Worked for ${took}` : `Working for ${took}`}
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
  const { view, error } = useWorkerThread(conversationId, row);
  const clock = useNow(60_000);
  const state = workerStateOf(row, clock);
  const working = row.working;
  const failed = state.kind === "failed";
  // Its own items, not its subagents': their Agent call's row stands for them.
  const items = useMemo(
    () => foldTranscript((view?.entries ?? []).filter((entry) => !entry.parent).map((entry, index) => ({ streamSeq: index, atMs: entry.atMs ?? 0, event: entry.event }))).items,
    [view],
  );
  const all = useMemo(() => threadEntries(items).filter((entry) => !workerCliNotice(entry, failed)), [items, failed]);
  // Its first message is its brief, as the thread's first bubble.
  const briefAt = all.findIndex(received);
  const brief = briefAt >= 0 ? all[briefAt] : undefined;
  const entries = useMemo(() => all.filter((_, index) => index !== briefAt), [all, briefAt]);
  const activity = workerActivity(entries);
  const previous = activity.slice(0, Math.max(0, activity.length - 3));
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
        renderStep={(step, key) => <ActionRow key={key} item={step} />}
      />
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
        <WorkHeader row={row} />
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
        {recent.map((item, index) => show(item, previous.length + index))}
        {thinking && <ThinkingLine text={thinking.text} />}
        {now && !nowInGroup && <div className="shimmer truncate text-sm motion-reduce:animate-none">{now}</div>}
        {/* What it waits for, from its row: the user (in its session), or a limit's reset. */}
        {(state.kind === "needsYou" || state.kind === "limit") && <p data-slot="worker-waits" className="text-foreground/65 text-sm">{state.word}</p>}
      </div>
      </StoppedBy.Provider>
    </div>
  );
}
