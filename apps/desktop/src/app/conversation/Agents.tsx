import { useAui } from "@assistant-ui/react";
import {
  ArrowLeft,
  AtSign,
  DotsHorizontal,
  Pause,
  Play,
  Stop,
} from "@openai/apps-sdk-ui/components/Icon";
import { memo, useContext, useMemo, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { isFinal, isWorking } from "@/app/conversation/blocks";
import { HandoffLine, RouteReason } from "@/app/conversation/cards/RouteDetails";
import { isStoppable } from "@/app/conversation/cards/TaskCardView";
import {
  AgentsPanelContext,
  glyphTone,
  taskStateLabel,
  useWorkerName,
  WorkerGlyph,
} from "@/app/conversation/WorkerChip";
import { WorkerThread } from "@/app/conversation/WorkerThread";
import { effortLabel } from "@/components/assistant-ui/elements/model-selector";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useNow } from "@/hooks/use-now";
import type { Task } from "@/ipc/generated";
import { formatAgo } from "@/lib/format";
import { withResetTime } from "@/lib/routing";
import { modelName, useModelGroups } from "@/lib/setup";
import { cn } from "@/lib/utils";
import { pauseTask, resumeTask, stopTask } from "@/state/actions";
import { useBoard, type WorkerSummary } from "@/state/board";
import { toast } from "@/state/toasts";

/** What the UI calls a session's workers (the user chose "Workers" over "Subagents"). */
export const WORKERS_LABEL = "Workers";

/**
 * The session's workers ("agents"): a side panel listing them all, where one opens to show its
 * live transcript, commands and diff. In the thread each has one row (TaskRow).
 */

/** Glyphs stacked in a summary row. */
const STACKED = 4;

/** Several workers' glyphs stacked, overlapping, each sliding in as it joins; a dot on those working. */
export function WorkerGlyphs({
  taskIds,
  working,
}: {
  taskIds: readonly string[];
  working?: ReadonlySet<string>;
}) {
  return (
    <span className="flex shrink-0 items-center -space-x-1">
      {taskIds.slice(0, STACKED).map((id) => (
        <WorkerGlyph
          key={id}
          taskId={id}
          working={working?.has(id)}
          className="animate-glyph-in motion-reduce:animate-none"
        />
      ))}
    </span>
  );
}

/**
 * A worker's live status under its name in the list: at work, its latest reply's first line,
 * or "Thinking" before it said anything; otherwise its state in a word or two.
 */
function statusLine(task: Task, summary: WorkerSummary | undefined, now: number): string | null {
  switch (task.state) {
    case "queued":
      return "Queued";
    case "starting":
      return "Starting";
    case "running":
      return summary?.text ?? "Thinking";
    case "blocked":
      return "Asked the orchestrator";
    case "paused":
      return task.quotaWait
        ? `Waiting for quota: ${withResetTime(task.quotaWait.reason, task.quotaWait.resetsAtMs, now)}`
        : "Paused";
    case "reported":
      return "Reported";
    case "reviewing":
      return "In review";
    case "awaitingApproval":
    case "readyToLand":
      return "Waiting for you";
    case "rejected":
      return "Turned down";
    case "stopped":
      return "Interrupted";
    case "failed":
      return "Failed";
    case "landed":
    case "done":
      return null;
  }
}

/** When it last said something or changed state ("now", "3m ago"). */
function RowTime({ task, summary }: { task: Task; summary: WorkerSummary | undefined }) {
  const now = useNow(60_000);
  const at = Math.max(task.updatedAtMs, isFinal(task) ? 0 : (summary?.atMs ?? 0));
  return (
    <span className="text-muted-foreground shrink-0 text-xs tabular-nums">{formatAgo(at, now)}</span>
  );
}

const AgentRow = memo(function AgentRow({ taskId }: { taskId: string }) {
  const task = useBoard((s) => s.board?.tasks[taskId]);
  const summary = useBoard((s) => s.board?.summaries[taskId]);
  const name = useWorkerName(taskId);
  const { setPanel } = useContext(AgentsPanelContext);
  const now = useNow(task?.quotaWait ? 60_000 : null);
  if (!task) return null;
  const status = statusLine(task, summary, now);
  return (
    <li>
      <button
        type="button"
        data-task={`task-${task.number}`}
        title={`task-${task.number} · ${taskStateLabel(task)}`}
        onClick={() => setPanel(task.id)}
        className="hover:bg-foreground/5 rounded-control flex w-full items-start gap-3 px-2 py-2 text-start transition-colors"
      >
        <WorkerGlyph taskId={task.id} tone={glyphTone(task.state)} className="size-6" />
        <span className="flex min-w-0 flex-1 flex-col">
          <span className="flex items-baseline gap-2">
            <span className="min-w-0 flex-1 truncate text-sm">{name}</span>
            <RowTime task={task} summary={summary} />
          </span>
          {status && (
            <span
              className={cn(
                "line-clamp-2 text-sm",
                task.state === "failed" ? "text-destructive" : "text-muted-foreground",
              )}
            >
              {status}
            </span>
          )}
        </span>
      </button>
    </li>
  );
});

/**
 * One section of the list, a page at a time: the first `page` rows, then "Show N more" for
 * the next page. The Done section counts its rows in its title.
 */
function AgentSection({
  title,
  ids,
  page,
  counted,
  trailing,
  empty,
  className,
}: {
  title: string;
  ids: readonly string[];
  page: number;
  counted?: boolean;
  trailing?: string | undefined;
  empty?: string;
  className?: string;
}) {
  const [pages, setPages] = useState(1);
  if (ids.length === 0 && !empty) return null;
  const shown = ids.slice(0, page * pages);
  const hidden = ids.length - shown.length;
  return (
    <section className={cn("flex flex-col", className)}>
      <h3 className="text-muted-foreground mb-2 flex items-center gap-2 px-2 text-sm">
        <span className="flex-1">{counted ? `${title} · ${ids.length}` : title}</span>
        {trailing && <span className="text-xs tabular-nums">{trailing}</span>}
      </h3>
      {ids.length === 0 ? (
        <p className="text-muted-foreground px-2 text-sm">{empty}</p>
      ) : (
        <ul className="flex flex-col gap-0.5">
          {shown.map((id) => (
            <AgentRow key={id} taskId={id} />
          ))}
        </ul>
      )}
      {hidden > 0 && (
        <button
          type="button"
          onClick={() => setPages(pages + 1)}
          className="text-muted-foreground hover:text-foreground ms-9 mt-1 self-start px-2 py-1 text-sm transition-colors"
        >
          Show {Math.min(hidden, page)} more
        </button>
      )}
    </section>
  );
}

/**
 * The worker detail's ⋯ menu: pause or resume it, stop it, or mention it in the composer so the
 * next message goes to the orchestrator about it.
 */
function WorkerMenu({ task }: { task: Task }) {
  const aui = useAui();
  const run = (label: string, action: () => Promise<unknown>) => {
    action().catch((cause: unknown) => {
      const reason = cause instanceof Error ? cause.message : String(cause);
      toast(`Couldn't ${label} ${task.title}: ${reason}`, { tone: "error" });
    });
  };
  const mention = () => {
    const composer = aui.composer();
    const text = composer.getState().text;
    const tag = `@task-${task.number} `;
    composer.setText(text && !text.endsWith(" ") ? `${text} ${tag}` : `${text}${tag}`);
  };
  const stoppable = isStoppable(task);
  const working = task.state === "running" || task.state === "starting";
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <TooltipIconButton tooltip={`${WORKERS_LABEL.slice(0, -1)} actions`} size="icon-sm">
          <DotsHorizontal />
        </TooltipIconButton>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        {task.state === "paused" && !task.quotaWait && (
          <DropdownMenuItem onSelect={() => run("resume", () => resumeTask(task.id))}>
            <Play />
            Resume
          </DropdownMenuItem>
        )}
        {working && (
          <DropdownMenuItem
            title={
              task.route.choice.provider === "codex"
                ? "Pausing Codex lets its current command finish first."
                : undefined
            }
            onSelect={() => run("pause", () => pauseTask(task.id))}
          >
            <Pause />
            Pause
          </DropdownMenuItem>
        )}
        {stoppable && (
          <DropdownMenuItem onSelect={() => run("stop", () => stopTask(task.id))}>
            <Stop />
            Stop
          </DropdownMenuItem>
        )}
        {stoppable && <DropdownMenuSeparator />}
        <DropdownMenuItem onSelect={mention}>
          <AtSign />
          Mention
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** Active workers first, then the finished ones, each by number. */
export function useWorkerIds(conversationId: string): { active: string[]; finished: string[] } {
  const ids = useBoard(
    useShallow((s) => {
      if (s.board?.conversationId !== conversationId) return [];
      const tasks = Object.values(s.board.tasks).toSorted((a, b) => a.number - b.number);
      // `|` separates the two lists.
      return [
        ...tasks.filter((task) => !isFinal(task)).map((task) => task.id),
        "|",
        ...tasks.filter(isFinal).map((task) => task.id),
      ];
    }),
  );
  return useMemo(() => {
    const split = ids.indexOf("|");
    return { active: ids.slice(0, Math.max(split, 0)), finished: ids.slice(split + 1) };
  }, [ids]);
}

/** Rows a section shows before "Show N more": 4 active and 10 done. */
const ACTIVE_PAGE = 4;
const DONE_PAGE = 10;

/** Every worker of the session. The rows hold still while the pointer is over them. */
function WorkerList({ conversationId }: { conversationId: string }) {
  const lists = useWorkerIds(conversationId);
  const [held, setHeld] = useState<typeof lists | null>(null);
  const waiting = useBoard(
    (s) =>
      s.board?.conversationId === conversationId
        ? Object.values(s.board.tasks).filter((task) => !isFinal(task) && !isWorking(task))
            .length
        : 0,
  );
  const shown = held ?? lists;
  if (lists.active.length + lists.finished.length === 0) {
    return (
      <p className="text-muted-foreground p-4 text-sm">
        No {WORKERS_LABEL.toLowerCase()} yet.
      </p>
    );
  }
  // A worker that started while the list was held joins it at the end.
  const heldIds = new Set([...shown.active, ...shown.finished]);
  const joined = [...lists.active, ...lists.finished].filter((id) => !heldIds.has(id));
  return (
    <div
      className="min-h-0 flex-1 overflow-y-auto px-3 py-5"
      onPointerEnter={() => setHeld(lists)}
      onPointerLeave={() => setHeld(null)}
    >
      <div className="max-w-thread mx-auto flex w-full flex-col">
        <AgentSection
          title="Active"
          ids={[...shown.active, ...joined]}
          page={ACTIVE_PAGE}
          trailing={waiting > 0 ? `${waiting} waiting` : undefined}
          empty={`No active ${WORKERS_LABEL.toLowerCase()}`}
        />
        <AgentSection
          title="Done"
          ids={shown.finished}
          page={DONE_PAGE}
          counted
          className="mt-6"
        />
      </div>
    </div>
  );
}

/** One worker: its glyph, name and model in the header, its thread below. */
function WorkerDetail({ task }: { task: Task }) {
  const { setPanel } = useContext(AgentsPanelContext);
  const name = useWorkerName(task.id);
  const groups = useModelGroups();
  const choice = task.route.choice;
  const model = `${modelName(groups, choice)}${choice.effort ? ` · ${effortLabel(choice.effort)}` : ""}`;
  return (
    <>
      <header className="border-border flex h-12 shrink-0 items-center gap-2 border-b px-4">
        <TooltipIconButton
          tooltip={`Back to ${WORKERS_LABEL.toLowerCase()}`}
          size="icon-sm"
          onClick={() => setPanel(null)}
        >
          <ArrowLeft />
        </TooltipIconButton>
        <WorkerGlyph taskId={task.id} className="size-6" />
        <h2
          className="min-w-0 flex-1 truncate text-sm font-medium"
          title={`${name ?? task.title} · task-${task.number}`}
        >
          {name ?? task.title}
        </h2>
        <span className="text-muted-foreground shrink-0 text-xs">{model}</span>
        <WorkerMenu task={task} />
      </header>
      {(task.route.reason || task.attempts.length > 1) && (
        <div className="border-border flex shrink-0 flex-col gap-0.5 border-b px-4 py-1.5">
          <RouteReason task={task} />
          <HandoffLine task={task} groups={groups} />
        </div>
      )}
      <WorkerThread key={task.id} task={task} model={model} />
    </>
  );
}

/** The Workers tab of the side panel: every worker of the session, or one of them working. */
export function WorkersTab({ conversationId }: { conversationId: string }) {
  const { panel } = useContext(AgentsPanelContext);
  const selected = useBoard((s) => (panel ? s.board?.tasks[panel] : undefined));
  if (selected) return <WorkerDetail task={selected} />;
  return <WorkerList conversationId={conversationId} />;
}
