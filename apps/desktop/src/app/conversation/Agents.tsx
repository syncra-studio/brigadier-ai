import { ArrowLeft } from "@openai/apps-sdk-ui/components/Icon";
import { type ReactNode, memo, useContext, useMemo, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { workerDone, workerWorking, workerState, workerPreview } from "@/app/conversation/workerPresentation";
import { StopWorker } from "@/app/conversation/activity/StopWorker";
import { TaskDetails } from "@/app/conversation/TaskDetails";
import { MarkdownBlock } from "@/components/assistant-ui/thread";
import {
  AgentsPanelContext,
  useWorkerName,
  useWorkerText,
  WorkerGlyph,
} from "@/app/conversation/WorkerChip";
import { WorkerThread } from "@/app/conversation/WorkerThread";
import { effortLabel } from "@/components/assistant-ui/elements/model-selector";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useNow } from "@/hooks/use-now";
import type { Task } from "@/ipc/generated";
import { formatAgo, formatDuration } from "@/lib/format";
import { modelName, useModelGroups } from "@/lib/setup";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";

/** What the UI calls a session's workers (the user chose "Workers" over "Subagents"). */
export const WORKERS_LABEL = "Workers";

/** The session's flat worker list and read-only worker threads. */

/** Glyphs stacked in a summary row. */
const STACKED = 4;

/** Up to four workers' seeded glyphs in a row. */
export function WorkerGlyphs({
  taskIds,
}: {
  taskIds: readonly string[];
}) {
  return (
    <span className="flex shrink-0 items-center gap-1.5">
      {taskIds.slice(0, STACKED).map((id) => (
        <WorkerGlyph
          key={id}
          taskId={id}
          className="animate-glyph-in motion-reduce:animate-none"
        />
      ))}
    </span>
  );
}

/** Live elapsed time; done rows show recency and remain openable. */
function RowTime({ task }: { task: Task }) {
  const done = workerDone(task);
  const now = useNow(done ? 60_000 : 1000);
  const start = task.attempts.at(-1)?.startedAtMs ?? task.createdAtMs;
  return <span className="text-foreground/50 shrink-0 text-xs tabular-nums">
    {done ? formatAgo(task.updatedAtMs, now) : formatDuration(Math.max(0, now - start))}
  </span>;
}

/**
 * One worker on one line: its glyph, its name (a soft "working" while it works), and how long it
 * has worked or how long ago it finished; Stop in the time's place on hover. A click opens it.
 */
const AgentRow = memo(function AgentRow({ taskId }: { taskId: string }) {
  const task = useBoard((s) => s.board?.tasks[taskId]);
  const name = useWorkerName(taskId);
  const { setPanel } = useContext(AgentsPanelContext);
  if (!task) return null;
  const working = workerWorking(task);
  const stoppable = !workerDone(task);
  return (
    <li className="group/row relative flex min-w-0">
      <button
        type="button"
        data-task={`task-${task.number}`}
        aria-label={`${name} ${workerState(task)}`}
        title={workerPreview(task) ?? undefined}
        onClick={() => setPanel(task.id)}
        className="hover:bg-foreground/5 rounded-lg flex h-row min-w-0 flex-1 items-center gap-3 px-2 text-start transition-colors"
      >
        <WorkerGlyph taskId={task.id} className="size-5" />
        <span className="min-w-0 flex-1 truncate text-label">
          {name}
          {working && <span className="text-muted-foreground shimmer"> working</span>}
        </span>
        <span className={cn(stoppable && "group-hover/row:invisible group-focus-within/row:invisible")}>
          <RowTime task={task} />
        </span>
      </button>
      {/* Its Stop on the row, without opening the worker first. */}
      {stoppable && (
        <span className="absolute inset-y-0 end-1 hidden items-center group-focus-within/row:flex group-hover/row:flex">
          <StopWorker taskId={task.id} name={name ?? task.title} />
        </span>
      )}
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
      <h3 className="text-muted-foreground mb-2 flex items-center gap-2 px-2 text-label">
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

/** Active workers first, then finished workers, newest first within each group. */
export function useWorkerIds(conversationId: string): { active: string[]; finished: string[] } {
  const ids = useBoard(
    useShallow((s) => {
      if (s.board?.conversationId !== conversationId) return [];
      const tasks = Object.values(s.board.tasks).toSorted((a, b) => b.createdAtMs - a.createdAtMs || b.number - a.number);
      // `|` separates the two lists.
      return [
        ...tasks.filter((task) => !workerDone(task)).map((task) => task.id),
        "|",
        ...tasks.filter(workerDone).map((task) => task.id),
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

/** Every worker of the session, ordered by recency. */
function WorkerList({ conversationId }: { conversationId: string }) {
  const lists = useWorkerIds(conversationId);
  const waiting = useBoard(
    (s) =>
      s.board?.conversationId === conversationId
        ? Object.values(s.board.tasks).filter((task) => !workerDone(task) && !workerWorking(task))
            .length
        : 0,
  );
  return (
    <div
      className="min-h-0 flex-1 overflow-y-auto px-3 py-5"
    >
      <div className="max-w-thread mx-auto flex w-full flex-col">
        <AgentSection
          title="Active"
          counted
          ids={lists.active}
          page={ACTIVE_PAGE}
          trailing={waiting > 0 ? `${waiting} waiting` : undefined}
          empty={`No active ${WORKERS_LABEL.toLowerCase()}`}
        />
        <AgentSection
          title="Done"
          ids={lists.finished}
          page={DONE_PAGE}
          counted
          className="mt-6"
        />
      </div>
    </div>
  );
}

/** One worker: its glyph, name and model in the header, its thread below. */
function WorkerDetail({ task, headerActions }: { task: Task; headerActions?: ReactNode }) {
  const { setPanel } = useContext(AgentsPanelContext);
  const name = useWorkerName(task.id);
  const [details, setDetails] = useState(false);
  const instructions = useWorkerText(task.spec);
  const groups = useModelGroups();
  const choice = task.route.choice;
  const model = `${modelName(groups, choice)}${choice.effort ? ` · ${effortLabel(choice.effort)}` : ""}`;
  return (
    <section aria-label={`Worker: ${name ?? task.title}`} className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header data-slot="worker-header" className="border-border flex h-page-toolbar min-w-0 shrink-0 items-center gap-2 border-b px-2">
        <TooltipIconButton
          tooltip={details ? "Back to worker" : `Back to ${WORKERS_LABEL.toLowerCase()}`}
          size="icon-sm"
          onClick={() => details ? setDetails(false) : setPanel(null)}
        >
          <ArrowLeft />
        </TooltipIconButton>
        <WorkerGlyph taskId={task.id} className="size-6" />
        <h2 className="min-w-8 truncate text-label font-medium" title={name ?? task.title}>
          {name ?? task.title}
        </h2>
        <span className="flex-1" />
        {/* Its model at the end, as a worker's conversation reads; a click shows its details.
            Where the header is short it gives way first: the title keeps its room. */}
        <Tooltip>
          <TooltipTrigger asChild>
            <button type="button" aria-label={`Worker details: ${model}`} aria-pressed={details}
              data-state={details ? "open" : "closed"}
              className="text-muted-foreground hover:text-foreground data-[state=open]:text-foreground flex min-w-0 shrink-1000 rounded-sm px-1 text-label transition-colors"
              onClick={() => setDetails((open) => !open)}>
              <span className="truncate">{model}</span>
            </button>
          </TooltipTrigger>
          <TooltipContent>Worker details · {model}</TooltipContent>
        </Tooltip>
        <StopWorker taskId={task.id} name={name ?? task.title} />
        {headerActions}
      </header>
      {details ? <div data-slot="worker-details" data-selectable className="min-h-0 min-w-0 flex-1 overflow-y-auto px-4 py-5 wrap-anywhere">
        <div className="flex flex-col gap-4">
          <h3 className="text-sm font-medium">Details</h3>
          <TaskDetails task={task} model={model} inThread detailsPage />
          <section className="flex flex-col gap-2">
            <h3 className="text-sm font-medium">Instructions</h3>
            <MarkdownBlock text={instructions} />
          </section>
        </div>
      </div> : <WorkerThread key={task.id} task={task} />}
    </section>
  );
}

/** The Workers tab of the side panel: every worker of the session, or one of them working. */
export function WorkersTab({ conversationId, headerActions }: { conversationId: string; headerActions?: ReactNode }) {
  const { panel } = useContext(AgentsPanelContext);
  const selected = useBoard((s) => (panel ? s.board?.tasks[panel] : undefined));
  if (selected) return <WorkerDetail key={selected.id} task={selected} headerActions={headerActions} />;
  return <WorkerList conversationId={conversationId} />;
}
