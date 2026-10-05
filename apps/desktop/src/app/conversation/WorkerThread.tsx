import {
  Book,
  Check,
  ChevronRight,
  Copy,
  EditPencil,
  Folder,
  Globe,
  Reply,
  Search,
  ShieldCheck,
  Terminal,
  Tools,
} from "@openai/apps-sdk-ui/components/Icon";
import {
  type FC,
  type ReactNode,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { isFinal, isWorking } from "@/app/conversation/blocks";
import { QuotaWaitLine } from "@/app/conversation/cards/RouteDetails";
import { TaskDetails } from "@/app/conversation/cards/TaskCardView";
import { useAction } from "@/app/conversation/useAction";
import { taskStateLabel } from "@/app/conversation/WorkerChip";
import { RateItem, RateMenu } from "@/components/assistant-ui/rate-menu";
import { MarkdownBlock } from "@/components/assistant-ui/thread";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import {
  type ActivityKind,
  activityOf,
  shownCommand,
  summarize,
  type ThreadEntry,
  threadEntries,
  unwrapCommand,
} from "@/components/transcript/activity";
import { type TranscriptItem, TranscriptFolder } from "@/components/transcript/transcript";
import { ErrorState } from "@/components/assistant-ui/elements/error-state";
import { searchResults, WebSearch } from "@/components/assistant-ui/elements/web-search";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { useNow } from "@/hooks/use-now";
import type { ErrorKind, Rating, RawEntry, Task } from "@/ipc/generated";
import { formatDuration, formatSentAt } from "@/lib/format";
import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";
import { loadEarlierWorkerEntries, openWorkerTranscript, rateMessage } from "@/state/actions";
import { useBoard } from "@/state/board";

/**
 * A worker's own thread in the workers panel, told like a main-thread answer: "Working for
 * 34s", its replies with its actions summed up between them ("Read files, ran commands"),
 * what it does right now, and its report as the final answer. Once it reported, the work
 * before the report folds into "Worked for 55s".
 */

const NO_ENTRIES: RawEntry[] = [];

/** Milliseconds a task has worked: until now while it is active, until its last update after. */
export function useTaskElapsed(task: Task): number {
  const final = isFinal(task);
  const now = useNow(final ? null : 1000);
  return Math.max(0, (final ? task.updatedAtMs : now) - task.createdAtMs);
}

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

const ICONS: Record<ActivityKind, FC<{ className?: string }>> = {
  read: Book,
  list: Folder,
  search: Search,
  edit: EditPencil,
  run: Terminal,
  report: Reply,
  tool: Tools,
};

type ActionItem = Extract<ThreadEntry, { kind: "actions" }>["items"][number];

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

const row = "text-muted-foreground flex min-h-row-sm min-w-0 items-center gap-2 text-sm";

/** A command's box: "Shell", the command and what it printed, then how it ended. */
function shellCard(item: Extract<ActionItem, { kind: "command" }>): ReactNode {
  const output = item.output.trimEnd();
  const ending =
    item.status === "inProgress"
      ? "Running"
      : item.status === "declined"
        ? "Stopped"
        : item.exitCode !== null && item.exitCode !== 0
          ? `Exit code ${item.exitCode}`
          : item.status === "failed"
            ? "Failed"
            : "Success";
  return (
    <div data-slot="shell-card" className="border-border bg-code-surface rounded-control mt-1 flex flex-col border text-sm">
      <span className="text-muted-foreground px-3 pt-2 text-xs">Shell</span>
      <pre className="text-code max-h-60 overflow-auto px-3 py-1 font-mono whitespace-pre-wrap">
        {`$ ${unwrapCommand(item.command)}`}
        {output ? `\n${output}` : <span className="text-muted-foreground">{"\nNo output"}</span>}
      </pre>
      <span
        className={cn(
          "border-border border-t px-3 py-1.5 text-end text-xs",
          ending === "Success" ? "text-muted-foreground" : ending === "Running" ? "shimmer" : "text-destructive",
        )}
      >
        {ending}
      </span>
    </div>
  );
}

function card(title: string, body: string): ReactNode {
  return (
    <div className="border-border bg-code-surface rounded-control mt-1 flex flex-col gap-1 border px-3 py-2">
      <span className="text-muted-foreground text-xs">{title}</span>
      <pre className="text-code max-h-60 overflow-auto font-mono whitespace-pre-wrap">{body}</pre>
    </div>
  );
}

/** A web search's query, from its call. */
function searchQuery(input: string | null): string | null {
  try {
    const value: unknown = JSON.parse(input ?? "");
    const query = value && typeof value === "object" ? (value as { query?: unknown }).query : null;
    return typeof query === "string" ? query : null;
  } catch {
    return null;
  }
}

/** What an action row opens to: a Shell box for a command, the pages a search found, the call for a tool. */
function actionDetail(item: ActionItem): ReactNode {
  switch (item.kind) {
    case "command":
      return shellCard(item);
    case "tool": {
      const query = item.name === "WebSearch" ? searchQuery(item.input) : null;
      if (query !== null && item.status === "failed") {
        // A failed search found nothing: its query, then what went wrong in full.
        return (
          <div className="flex flex-col gap-2 ps-6 pt-1 pb-2">
            <WebSearch query={query} results={[]} />
            <ErrorState title="The search failed" detail={item.output || null} />
          </div>
        );
      }
      if (query !== null) {
        return (
          <WebSearch
            className="ps-6 pt-1 pb-2"
            query={query}
            results={searchResults(item.output)}
            searching={item.status === "inProgress"}
          />
        );
      }
      if (!item.input && !item.output) return null;
      return card(item.name, [item.input, item.output].filter(Boolean).join("\n\n"));
    }
    case "files":
      return card(
        "Files",
        item.changes.map((change) => `${change.kind} ${change.path}`).join("\n"),
      );
    case "image":
      return item.path || item.prompt ? card("Image", item.path ?? item.prompt ?? "") : null;
  }
}

/** One action as a grey line; a command or tool call opens to what it ran. */
function ActionRow({ item }: { item: ActionItem }) {
  const activity = activityOf(item);
  const Icon = activity.web ? Globe : ICONS[activity.kind];
  const running = item.status === "inProgress";
  const label = (
    <>
      <Icon aria-hidden className="size-icon-md shrink-0" />
      <span className="min-w-0 truncate">{running ? activity.doing : activity.done}</span>
      {item.status === "failed" && <span className="text-destructive shrink-0">failed</span>}
      {item.kind === "command" && !running && item.durationMs !== null && item.durationMs >= 1000 && (
        <span className="shrink-0 tabular-nums">in {formatDuration(item.durationMs)}</span>
      )}
    </>
  );
  const detail = actionDetail(item);
  if (!detail) return <div className={row}>{label}</div>;
  return (
    <Collapsible>
      <CollapsibleTrigger className={cn(row, "group hover:text-foreground w-full text-start")}>
        {label}
        <ChevronRight
          aria-hidden
          className="size-icon-xs shrink-0 opacity-0 transition-[rotate,opacity] group-hover:opacity-100 group-data-[state=open]:rotate-90 group-data-[state=open]:opacity-100"
        />
      </CollapsibleTrigger>
      <CollapsibleContent>{detail}</CollapsibleContent>
    </Collapsible>
  );
}

/** A run of actions between two replies, summed up in one line; it opens to each of them. */
function ActionRun({ items }: { items: readonly ActionItem[] }) {
  const [first] = items;
  if (items.length === 1 && first) return <ActionRow item={first} />;
  const activities = items.map(activityOf);
  const counts = new Map<ActivityKind, number>();
  for (const activity of activities) counts.set(activity.kind, (counts.get(activity.kind) ?? 0) + 1);
  // Edits win the icon; otherwise the most frequent kind does.
  const [dominant] = counts.has("edit")
    ? ["edit" as const]
    : ([...counts].toSorted((a, b) => b[1] - a[1])[0] ?? ["run" as const]);
  const Icon = ICONS[dominant];
  return (
    <Collapsible>
      <CollapsibleTrigger className={cn(row, "group hover:text-foreground w-full text-start")}>
        <Icon aria-hidden className="size-icon-md shrink-0" />
        <span className="min-w-0 truncate">{summarize(activities)}</span>
        <ChevronRight
          aria-hidden
          className="size-icon-xs shrink-0 opacity-0 transition-[rotate,opacity] group-hover:opacity-100 group-data-[state=open]:rotate-90 group-data-[state=open]:opacity-100"
        />
      </CollapsibleTrigger>
      <CollapsibleContent className="max-h-action-list overflow-y-auto ps-6">
        {items.map((item) => (
          <ActionRow key={item.key} item={item} />
        ))}
      </CollapsibleContent>
    </Collapsible>
  );
}

function EntryView({ entry }: { entry: ThreadEntry }) {
  if (entry.kind === "actions") return <ActionRun items={entry.items} />;
  const { item } = entry;
  switch (item.kind) {
    case "message":
      return item.role === "user" ? (
        <div className="bg-secondary rounded-thread ms-8 self-end px-3 py-2 text-sm whitespace-pre-wrap">
          {item.text}
        </div>
      ) : (
        <div className="text-foreground leading-relaxed wrap-break-word">
          <MarkdownBlock text={item.text} streaming={item.streaming} />
        </div>
      );
    case "approval": {
      // Answered, it leaves no trace: the command's own row tells what came of it.
      if (item.resolution) return null;
      const what = item.request.command ? shownCommand(item.request.command) : item.request.paths.join(", ") || item.request.tool;
      return (
        <div className={cn(row, "text-warning")}>
          <ShieldCheck aria-hidden className="size-icon-md shrink-0" />
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
      return <p className="text-muted-foreground text-sm">Its turn {item.status}.</p>;
    default:
      return null;
  }
}

/** What the worker does right now, as the thread's last line, when no row of its says it. */
function liveLabel(entries: readonly ThreadEntry[]): string | null {
  const last = entries.at(-1);
  // A running action says so on its own row.
  if (last?.kind === "actions" && last.items.at(-1)?.status === "inProgress") return null;
  if (last?.kind === "item" && last.item.kind === "message" && last.item.streaming) return null;
  return "Thinking";
}

function header(task: Task, elapsed: number): string {
  const time = formatDuration(elapsed);
  if (isWorking(task)) return elapsed < 1000 ? "Working" : `Working for ${time}`;
  switch (task.state) {
    case "failed":
      return `Failed after ${time}`;
    case "stopped":
      return `Stopped after ${time}`;
    case "paused":
      return task.quotaWait ? `Waiting for quota after ${time}` : `Paused after ${time}`;
    default:
      return `Worked for ${time}`;
  }
}

/** The report as the thread's answer, with copy and rate; when it came shows on hover. */
function Answer({ task }: { task: Task }) {
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  const now = useNow(60_000);
  const subject = `task:${task.id}`;
  const rated = useBoard((s) => s.board?.ratings[subject] ?? null);
  const action = useAction();
  const summary = task.report?.summary;
  if (!summary) return null;
  const rate = (rating: Rating) =>
    action.run(() => rateMessage(task.conversationId, subject, rating));
  return (
    <div className="group/answer flex flex-col gap-2">
      <div className="text-foreground leading-relaxed wrap-break-word">
        <MarkdownBlock text={summary} />
      </div>
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

export function WorkerThread({ task, model }: { task: Task; model: string }) {
  const transcript = useBoard((s) => s.board?.transcripts[task.id]);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    void openWorkerTranscript(task.conversationId, task.id).catch((cause: unknown) =>
      setError(cause instanceof Error ? cause.message : String(cause)),
    );
  }, [task.conversationId, task.id]);

  const [folding] = useState(() => new IncrementalFold());
  const raw = transcript?.entries ?? NO_ENTRIES;
  const entries = useMemo(() => threadEntries(folding.fold(raw)), [folding, raw]);
  const elapsed = useTaskElapsed(task);
  const working = isWorking(task);
  // With its report in and nothing running, the work before it folds.
  const foldable = !working && task.report !== null && entries.length > 0;
  const [open, setOpen] = useState(false);

  // Follow new output while the view is scrolled to the bottom.
  const scrollRef = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);
  useLayoutEffect(() => {
    const element = scrollRef.current;
    if (pinned.current && element && entries.length > 0) element.scrollTop = element.scrollHeight;
  }, [entries]);

  const label = header(task, elapsed);
  const now = working ? liveLabel(entries) : null;
  return (
    <div
      ref={scrollRef}
      data-slot="worker-thread"
      data-selectable
      className="min-h-0 flex-1 overflow-y-auto px-4 py-3"
      onScroll={(event) => {
        const element = event.currentTarget;
        pinned.current =
          element.scrollHeight - element.scrollTop - element.clientHeight < tokenPx("--spacing-row");
      }}
    >
      <div className="flex flex-col gap-3">
        {transcript?.hasMore && (!foldable || open) && (
          <Button
            size="xs"
            variant="ghost"
            className="self-center"
            disabled={transcript.loading}
            onClick={() => void loadEarlierWorkerEntries(task.conversationId, task.id)}
          >
            Load earlier
          </Button>
        )}
        {foldable ? (
          <button
            type="button"
            aria-expanded={open}
            onClick={() => setOpen(!open)}
            className="group border-border flex h-control-sm items-center gap-1 self-stretch border-b text-start"
          >
            <span className="text-muted-foreground text-sm">{label}</span>
            <ChevronRight
              aria-hidden
              className={cn(
                "text-muted-foreground size-icon-xs transition-[rotate,opacity] duration-200 motion-reduce:transition-none",
                open ? "rotate-90" : "opacity-0 group-hover:opacity-100",
              )}
            />
          </button>
        ) : (
          <div className="border-border flex h-control-sm items-center border-b">
            <span
              className={cn(
                "text-sm",
                working ? "shimmer motion-reduce:animate-none" : "text-muted-foreground",
                task.state === "failed" && "text-destructive",
              )}
            >
              {label}
            </span>
          </div>
        )}
        {error && (
          <p role="alert" className="text-destructive text-xs">
            {error}
          </p>
        )}
        {(!foldable || open) &&
          entries.map((entry) => (
            <EntryView
              key={entry.kind === "actions" ? entry.key : entry.item.key}
              entry={entry}
            />
          ))}
        {now && <div className="shimmer truncate text-sm motion-reduce:animate-none">{now}</div>}
        {!working && !task.report && !isFinal(task) && (
          <p className="text-muted-foreground text-sm">{taskStateLabel(task)}</p>
        )}
        <Answer task={task} />
        {task.error && <p className="text-destructive text-sm">{task.error}</p>}
        {task.quotaWait ? (
          <p className="text-muted-foreground text-sm">
            <QuotaWaitLine wait={task.quotaWait} />
          </p>
        ) : (
          task.blockedReason && <p className="text-warning text-sm">{task.blockedReason}</p>
        )}
        <div className="flex flex-col gap-3 pt-2">
          <TaskDetails task={task} model={model} inThread />
        </div>
      </div>
    </div>
  );
}
