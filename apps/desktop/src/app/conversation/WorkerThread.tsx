import { useWorkerText, WorkerLine } from "@/app/conversation/WorkerChip";
import { ThinkingRow } from "@/app/conversation/ThinkingRow";
import {
  Check,
  ChevronRight,
  Copy,
  ShieldCheck,
} from "@openai/apps-sdk-ui/components/Icon";
import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { FileList } from "@/app/conversation/FileList";
import { ArtifactDialog } from "@/app/conversation/ArtifactDialog";
import { workerDone, workerWorking, workerPreview } from "@/app/conversation/workerPresentation";
import { ActivityGroup, StepRow } from "@/app/conversation/activity/ActivityGroup";
import { actionDetail } from "@/app/conversation/activity/StepDetail";
import { workerActivity } from "@/app/conversation/activity/group";
import { type ActionItem, itemCall, stepWords } from "@/app/conversation/activity/words";
import { ROW } from "@/components/assistant-ui/elements/activity-row";
import { useAction } from "@/app/conversation/useAction";
import { RateItem, RateMenu } from "@/components/assistant-ui/rate-menu";
import { MarkdownBlock } from "@/components/assistant-ui/thread";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import {
  shownCommand,
  type ThreadEntry,
  threadEntries,
} from "@/components/transcript/activity";
import { type TranscriptItem, TranscriptFolder } from "@/components/transcript/transcript";
import { ErrorState } from "@/components/assistant-ui/elements/error-state";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { useNow } from "@/hooks/use-now";
import type { ArtifactRef, ErrorKind, Rating, RawEntry, Task } from "@/ipc/generated";
import { formatDuration, formatSentAt } from "@/lib/format";
import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";
import { loadEarlierWorkerEntries, openWorkerTranscript, rateMessage } from "@/state/actions";
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


/** A worker's error in a few words, by kind; its own words follow in full. */
const ERROR_TITLES: Record<ErrorKind, string> = {
  usageLimit: "Out of usage for now",
  rateLimit: "Rate limited",
  overloaded: "The model is overloaded",
  auth: "Not signed in",
  billing: "A billing problem",
  contextWindow: "Out of context",
  invalidRequest: "The request was refused",
  policy: "Blocked by a usage policy",
  network: "A network problem",
  sandbox: "Blocked by the sandbox",
  server: "The provider had an error",
  process: "The CLI stopped",
  other: "Something went wrong",
  stalled: "It stopped responding",
};

const row = ROW;

/** A worker's command's exit code, when it says. */
const exitOf = (item: ActionItem): number | null => (item.kind === "command" ? item.exitCode : null);

/** A worker's action, as the lead's same step would read; a command or call opens to what it ran. */
function ActionRow({ item }: { item: ActionItem }) {
  const call = itemCall(item);
  const ran = item.kind === "command" && item.status !== "inProgress" && item.durationMs !== null && item.durationMs >= 1000;
  return (
    <StepRow
      words={stepWords(call)}
      status={call.status}
      exit={exitOf(item)}
      detail={actionDetail(item)}
      suffix={ran && item.durationMs !== null ? `in ${formatDuration(item.durationMs)}` : undefined}
    />
  );
}

function WorkerMarkdown({ text, streaming = false }: { text: string; streaming?: boolean }) {
  return <MarkdownBlock text={useWorkerText(text)} streaming={streaming} />;
}

function EntryView({ entry }: { entry: ThreadEntry }) {
  if (entry.kind === "actions") return <>{entry.items.map((item) => <ActionRow key={item.key} item={item} />)}</>;
  const { item } = entry;
  switch (item.kind) {
    case "reasoning":
      // A settled thought sits in its work group; the live one is the live line.
      return null;
    case "message":
      return item.role === "user" ? (
        <div className="bg-secondary rounded-thread ms-8 self-end px-3 py-2 text-sm whitespace-pre-wrap">
          {item.text}
        </div>
      ) : (
        <div className="text-foreground leading-relaxed wrap-break-word">
          <WorkerMarkdown text={item.text} streaming={item.streaming} />
        </div>
      );
    case "approval": {
      // Answered, it leaves no trace: the command's own row tells what came of it.
      if (item.resolution) return null;
      const what = item.request.command ? shownCommand(item.request.command) : item.request.paths.join(", ") || item.request.tool;
      return (
        <div className={cn(row, "text-warning")}>
          <ShieldCheck aria-hidden className="size-4 shrink-0" />
          <span className="min-w-0 truncate">Waiting for your approval: {what}</span>
        </div>
      );
    }
    case "error":
      return (
        <ErrorState
          title={ERROR_TITLES[item.error.kind]}
          detail={item.error.willRetry ? `${item.error.message}\n\nIt tries again on its own.` : item.error.message}
        />
      );
    case "notice":
      return (
        <p className={cn("text-sm", item.level === "warning" ? "text-warning" : "text-muted-foreground")}>
          {item.text}
        </p>
      );
    case "turnCompleted":
      return null;
    default:
      return null;
  }
}

/** What the worker does right now, as the thread's last line, when no row of its says it. */
function liveLabel(entries: readonly ThreadEntry[]): string | null {
  const last = entries.at(-1);
  // A running action says so on its own row.
  if (last?.kind === "actions" && last.items.at(-1)?.status === "inProgress") return null;
  if (last?.kind === "item" && (last.item.kind === "message" || last.item.kind === "reasoning") && last.item.streaming && last.item.text.trim()) return null;
  return "Thinking";
}

/** The brief, as the thread's first bubble: three lines, then "Show brief" for the rest. */
function Brief({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  const [long, setLong] = useState(false);
  const body = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const element = body.current;
    if (element && !open) setLong(element.scrollHeight > element.clientHeight + 1);
  }, [open]);
  return (
    <div data-slot="worker-brief" className="bg-secondary rounded-thread ms-8 flex flex-col gap-1 self-end px-3 py-2 text-sm">
      <div ref={body} data-slot="worker-instructions" className={cn("leading-relaxed wrap-break-word", !open && "line-clamp-3")}>
        <WorkerMarkdown text={text} />
      </div>
      {(long || open) && (
        <button type="button" onClick={() => setOpen(!open)} className="text-foreground/60 hover:text-foreground self-start text-xs">
          {open ? "Show less" : "Show brief"}
        </button>
      )}
    </div>
  );
}

/** "Working for 1m 2s" while it works, "Worked for 3m 10s" once done, over a hairline. */
function WorkHeader({ task }: { task: Task }) {
  const done = workerDone(task);
  const now = useNow(done ? null : 1000);
  const start = task.attempts[0]?.startedAtMs ?? task.createdAtMs;
  const took = formatDuration(Math.max(0, (done ? task.updatedAtMs : now) - start));
  return (
    <div data-slot="worker-work-header" className="text-foreground/50 border-border border-b pb-2 text-sm tabular-nums">
      {done ? `Worked for ${took}` : `Working for ${took}`}
    </div>
  );
}

/** The report as the thread's answer, with copy and rate; when it came shows on hover. */
function Answer({ task, text }: { task: Task; text?: string | undefined }) {
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  const now = useNow(60_000);
  const subject = `task:${task.id}`;
  const rated = useBoard((s) => s.board?.ratings[subject] ?? null);
  const action = useAction();
  const summary = useWorkerText(text ?? task.report?.summary ?? "");
  const [artifact, setArtifact] = useState<ArtifactRef | null>(null);
  const files = [...new Map([...(task.report?.artifacts ?? []), ...task.outputs].map((file) => [file.id, file])).values()];
  if (!summary) return null;
  const rate = (rating: Rating) =>
    action.run(() => rateMessage(task.conversationId, subject, rating));
  return (
    <div data-slot="worker-report" className="group/answer flex scroll-mt-4 flex-col gap-2">
      <div className="text-foreground leading-relaxed wrap-break-word">
        <MarkdownBlock text={summary} />
      </div>
      {files.length > 0 && <FileList artifacts={files} onView={setArtifact} />}
      {task.report?.needsUser.map((need, index) => <p key={index} className="leading-relaxed"><WorkerLine text={need} /></p>)}
      <ArtifactDialog artifact={artifact} onOpenChange={(next) => !next && setArtifact(null)} />
      <div className="text-muted-foreground -ms-1 flex items-center gap-1">
        <TooltipIconButton
          tooltip={isCopied ? "Copied" : "Copy"}
          onClick={() => copyToClipboard(summary)}
        >
          {isCopied ? <Check /> : <Copy />}
        </TooltipIconButton>
        <RateMenu rated={rated}>
          <RateItem rating="good" onSelect={() => rate("good")} />
          <RateItem rating="bad" onSelect={() => rate("bad")} />
        </RateMenu>
        <span className="ps-1 text-xs tabular-nums opacity-0 transition-opacity group-hover/answer:opacity-100">
          {formatSentAt(task.updatedAtMs, now)}
        </span>
        {action.error && (
          <span role="alert" className="text-destructive ps-1 text-xs">
            {action.error}
          </span>
        )}
      </div>
    </div>
  );
}

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
  const entries = useMemo(() => threadEntries(folding.fold(raw)).filter((entry) => !(entry.kind === "item" && entry.item.kind === "message" && entry.item.role === "user")), [folding, raw]);
  const working = workerWorking(task);
  // The prompt and early steps belong to the same previous-messages disclosure.
  const tail = entries.at(-1);
  const finalReply = workerDone(task) && tail?.kind === "item" && tail.item.kind === "message" && tail.item.role === "assistant" ? tail.item.text : undefined;
  const visibleEntries = finalReply ? entries.slice(0, -1) : entries;
  const activity = workerActivity(visibleEntries, working);
  const previous = activity.slice(0, Math.max(0, activity.length - 3));
  const previousCount = task.messages.length + previous.reduce((count, item) => count + (item.type === "group" ? item.items.length : 1), 0);
  const recent = activity.slice(previous.length);
  const last = visibleEntries.at(-1);
  // A thought with no words yet is the live line's "Thinking".
  const thinking = working && last?.kind === "item" && last.item.kind === "reasoning" && last.item.streaming && last.item.text.trim() ? last.item : null;
  const show = (item: (typeof activity)[number], index: number) =>
    item.type === "group" ? (
      <ActivityGroup
        key={`group:${item.key}`}
        items={item.items}
        live={working && index === activity.length - 1}
        describe={(step) => {
          const call = itemCall(step);
          return { words: stepWords(call), status: call.status, exit: exitOf(step) };
        }}
        renderStep={(step, key) => <ActionRow key={key} item={step} />}
      />
    ) : (
      <EntryView key={item.entry.kind === "actions" ? item.entry.key : item.entry.item.key} entry={item.entry} />
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

  const now = working ? liveLabel(entries) : null;
  return (
    <div
      ref={scrollRef}
      data-slot="worker-thread"
      data-selectable
      className="min-h-0 flex-1 overflow-y-auto px-8 pb-6"
      onScroll={(event) => {
        const element = event.currentTarget;
        pinned.current =
          element.scrollHeight - element.scrollTop - element.clientHeight < tokenPx("--spacing-row");
      }}
    >
      <div className="max-w-thread mx-auto gap-activity flex flex-col pt-5">
        <Brief text={task.spec} />
        <WorkHeader task={task} />
        {(previousCount > 0 || transcript?.hasMore) && <Collapsible open={open} onOpenChange={setOpen}>
          <CollapsibleTrigger className="text-foreground/50 border-border hover:text-foreground flex min-h-7 w-full items-center gap-1 border-b text-start text-sm">
            {previousCount} previous {previousCount === 1 ? "message" : "messages"}
            <ChevronRight aria-hidden className={cn("size-3", open ? "-rotate-90" : "rotate-90")} />
          </CollapsibleTrigger>
          <CollapsibleContent className="gap-activity flex flex-col py-4">
            {transcript?.hasMore && <Button size="xs" variant="ghost" className="self-start" disabled={transcript.loading}
              onClick={() => void loadEarlierWorkerEntries(task.conversationId, task.id)}>Load earlier</Button>}
            {task.messages.map((text, index) => <WorkerMarkdown key={index} text={text} />)}
            {previous.map((item, index) => show(item, index))}
          </CollapsibleContent>
        </Collapsible>}
        {error && (
          <p role="alert" className="text-destructive text-xs">
            {error}
          </p>
        )}
        {recent.map((item, index) => show(item, previous.length + index))}
        {thinking && <ThinkingRow text={thinking.text} startedAtMs={thinking.startedAtMs} endedAtMs={thinking.endedAtMs} live />}
        {now && <div className="shimmer truncate text-sm motion-reduce:animate-none">{now}</div>}
        {!working && !workerDone(task) && <p className="text-foreground/65 text-sm">{workerPreview(task)}</p>}
        <Answer task={task} text={finalReply} />
        {task.error && <p className="text-destructive text-sm">{task.error}</p>}
      </div>
    </div>
  );
}
