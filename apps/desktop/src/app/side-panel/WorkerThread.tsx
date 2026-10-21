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
import { WorkerAnswer } from "@/app/side-panel/WorkerAnswer";
import { WorkerBrief } from "@/app/side-panel/WorkerBrief";
import { WorkerMarkdown } from "@/app/side-panel/WorkerMarkdown";
import { receivedMessage, workerDone, workerOutcome, workerWorking, workerPreview } from "@/app/conversation/workerPresentation";
import { ActivityGroup } from "@/app/conversation/activity/ActivityGroup";
import { workerActivity, workerCliNotice } from "@/app/conversation/activity/group";
import { StoppedBy } from "@/app/conversation/activity/stopped";
import { itemCall, stepWords } from "@/app/conversation/activity/words";
import { type ThreadEntry, threadEntries, type TranscriptItem, TranscriptFolder } from "@/components/conversation";
import { Button } from "@/components/controls";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/layout";
import { ChevronRight } from "@/components/icons";
import { useNow } from "@/hooks/use-now";
import type { RawEntry, Task } from "@/ipc/generated";
import { formatDuration } from "@/lib/format";
import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";
import { loadEarlierWorkerEntries, openWorkerTranscript } from "@/state/actions";
import { useBoard } from "@/state/board";

/** Read-only worker replies and activity, with instructions and early steps folded above. */

const NO_ENTRIES: RawEntry[] = [];

/**
 * Folds a growing transcript incrementally: live entries are applied on top of what was
 * folded; a reload or an older page (a different first entry) folds from scratch.
 */
class IncrementalFold {
  private folder = new TranscriptFolder();
  private first: RawEntry | undefined;

  fold(entries: readonly RawEntry[]): TranscriptItem[] {
    if (entries[0] !== this.first) {
      this.folder = new TranscriptFolder();
      this.first = entries[0];
    }
    return this.folder.push(entries).items;
  }
}


/** An earlier report, where the worker sent it: its answer of that turn. */
function EarlierAnswer({ text }: { text: string }) {
  return (
    <div data-slot="worker-earlier-report" className="text-foreground min-w-0 leading-relaxed wrap-anywhere">
      <WorkerMarkdown text={text} />
    </div>
  );
}

/** What the worker does right now, as the thread's last line, when no row of its says it. */
function liveLabel(entries: readonly ThreadEntry[]): string | null {
  const last = entries.at(-1);
  // A running action says so on its own row.
  if (last?.kind === "actions" && last.items.at(-1)?.status === "inProgress") return null;
  if (last?.kind === "item" && (last.item.kind === "message" || last.item.kind === "reasoning") && last.item.streaming && last.item.text.trim()) return null;
  return "Thinking";
}

/**
 * "Working for 1m 2s" while it works, "Worked for 3m 10s" once done, "Stopped after 3m 10s" when
 * it was stopped before its work was used, over a hairline.
 */
function WorkHeader({ task }: { task: Task }) {
  const done = workerDone(task);
  const now = useNow(done ? null : 1000);
  const start = task.attempts[0]?.startedAtMs ?? task.createdAtMs;
  const took = formatDuration(Math.max(0, (done ? task.updatedAtMs : now) - start));
  return (
    <div data-slot="worker-work-header" className="text-foreground/50 border-border border-b pb-2 text-sm tabular-nums">
      {workerOutcome(task) === "stopped" ? `Stopped after ${took}` : done ? `Worked for ${took}` : `Working for ${took}`}
    </div>
  );
}


/** A message the worker was sent, among its thread's entries. */
const received = (entry: ThreadEntry) => entry.kind === "item" && entry.item.kind === "message" && entry.item.role === "user";

export function WorkerThread({ task }: { task: Task }) {
  const transcript = useBoard((s) => s.board?.transcripts[task.id]);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    void openWorkerTranscript(task.conversationId, task.id).catch((cause: unknown) =>
      setError(cause instanceof Error ? cause.message : String(cause)),
    );
  }, [task.conversationId, task.id]);

  const [folding] = useState(() => new IncrementalFold());
  const raw = transcript?.entries ?? NO_ENTRIES;
  const failed = task.state === "failed";
  // Of its CLI's input, the thread shows the messages it was sent after its brief, where they came.
  const entries = useMemo(
    () =>
      threadEntries(folding.fold(raw)).filter(
        (entry) =>
          !(entry.kind === "item" && entry.item.kind === "message" && entry.item.role === "user" && receivedMessage(entry.item.text, task.messages) === null) &&
          !workerCliNotice(entry, failed),
      ),
    [folding, raw, failed, task.messages],
  );
  // A message shown where it came is not listed again with the previous messages.
  const unplaced = task.messages.filter((message) => !entries.some((entry) =>
    entry.kind === "item" && entry.item.kind === "message" && entry.item.role === "user" && receivedMessage(entry.item.text, task.messages) === message));
  const working = workerWorking(task);
  const done = workerDone(task);
  // Its latest report is the answer at the bottom once it is done; sent back to work (and maybe
  // stopped at that), the report stays where it sent it, above the message that sent it back.
  const reportAt = entries.findLastIndex((entry) => entry.kind === "report");
  const lastReport = entries[reportAt];
  const reportInPlace = reportAt >= 0 && (!done || entries.slice(reportAt + 1).some(received));
  // The prompt and early steps belong to the same previous-messages disclosure.
  const tail = entries.at(-1);
  // The report is the answer when there is one; a message after it ("Reported.") stays a message.
  const finalReply = done && !task.report?.summary && tail?.kind === "item" && tail.item.kind === "message" && tail.item.role === "assistant" ? tail.item.text : undefined;
  const visibleEntries = finalReply ? entries.slice(0, -1) : entries;
  const activity = workerActivity(visibleEntries);
  const previous = activity.slice(0, Math.max(0, activity.length - 3));
  const previousCount = unplaced.length + previous.reduce((count, item) => count + (item.type === "group" ? item.items.length : 1), 0);
  const recent = activity.slice(previous.length);
  const last = visibleEntries.at(-1);
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
    ) : item.entry.kind === "report" ? (
      item.entry === lastReport ? (reportInPlace ? <WorkerAnswer key={item.entry.key} task={task} earlier /> : null)
        : item.entry.summary && <EarlierAnswer key={item.entry.key} text={item.entry.summary} />
    ) : (
      <EntryView key={item.entry.kind === "actions" ? item.entry.key : item.entry.item.key} entry={item.entry} sent={task.messages} />
    );
  const [open, setOpen] = useState(false);

  // A finished worker opens at its report's top; a live one follows new output while the view
  // is scrolled to the bottom.
  const scrollRef = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);
  const placed = useRef(false);
  useLayoutEffect(() => {
    const element = scrollRef.current;
    if (!element || entries.length === 0) return;
    const report = element.querySelector<HTMLElement>('[data-slot="worker-report"]');
    if (!placed.current && !working && report) {
      placed.current = true;
      pinned.current = false;
      report.scrollIntoView({ block: "start" });
      return;
    }
    placed.current = true;
    if (pinned.current) element.scrollTop = element.scrollHeight;
  }, [entries, working]);

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
      <StoppedBy.Provider value={task.state === "stopped" ? { atMs: task.updatedAtMs } : null}>
      <div className="max-w-thread mx-auto min-w-0 gap-activity flex flex-col pt-5">
        <WorkerBrief text={task.spec} />
        <WorkHeader task={task} />
        {(previousCount > 0 || transcript?.hasMore) && <Collapsible open={open} onOpenChange={setOpen}>
          <CollapsibleTrigger className="text-foreground/50 border-border hover:text-foreground flex min-h-7 w-full items-center gap-1 border-b text-start text-sm">
            {previousCount} previous {previousCount === 1 ? "message" : "messages"}
            <ChevronRight aria-hidden className={cn("size-3", open ? "-rotate-90" : "rotate-90")} />
          </CollapsibleTrigger>
          <CollapsibleContent className="gap-activity flex flex-col py-4">
            {transcript?.hasMore && <Button size="xs" variant="ghost" className="self-start" disabled={transcript.loading}
              onClick={() => void loadEarlierWorkerEntries(task.conversationId, task.id)}>Load earlier</Button>}
            {unplaced.map((text, index) => <WorkerMarkdown key={index} text={text} />)}
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
        {!working && !done && <p className="text-foreground/65 text-sm">{workerPreview(task)}</p>}
        {!reportInPlace && <WorkerAnswer task={task} text={finalReply} />}
        {task.error && <p className="text-destructive text-sm">{task.error}</p>}
      </div>
      </StoppedBy.Provider>
    </div>
  );
}
