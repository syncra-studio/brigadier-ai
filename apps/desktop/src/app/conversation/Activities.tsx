import { useEffect, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { type BackgroundRow, type BackgroundState, backgroundRows, appTaskRows, workerAppTaskRows } from "@/app/conversation/backgroundRows";
import { endWords, taskEnd } from "@/app/conversation/backgroundTasks";
import { commandWords, elapsedWords } from "@/app/conversation/liveStatus";
import { previewWord } from "@/app/conversation/previewStatus";
import { rowName } from "@/app/conversation/activity/workerState";
import { StateIcon, SummaryRow, SummaryRowButton, SummarySection } from "@/components/conversation";
import { Clock, Terminal } from "@/components/icons";
import { TooltipIconButton, Button } from "@/components/controls";
import type { BackgroundTask, FileEntry, Preview } from "@/ipc/generated";
import { stopBackgroundTask, stopPreview } from "@/state/actions";
import { useBackgroundHistoryCount } from "@/state/backgroundHistory";
import { useBoard } from "@/state/board";
import { useCliThread, useWorkerRows } from "@/state/cliThread";
import { openLink } from "@/state/links";
import { openPanelTab } from "@/state/sessionTabs";
import { toast } from "@/state/toasts";
import { cn } from "@/lib/utils";

/**
 * One thing the session has going: a preview (the thread's or a worker's), or a background task
 * of its CLI's or a worker's CLI's.
 */
type Activity = BackgroundRow;

/** A process at work: its word shimmers. Paused or done, it rests. */
const live = (state: BackgroundState) => !["paused", "done", "finished", "failed", "stopped"].includes(state);

/** What a row is for, first letter up: "Dev app", "Core tests". A command stands as it is. */
const title = (words: string) => words.charAt(0).toUpperCase() + words.slice(1);

const label = (activity: Activity) =>
  activity.type === "task"
      ? activity.task.kind === "terminal" ? commandWords(activity.task.label) : activity.task.label
      : activity.preview.name === activity.preview.command ? commandWords(activity.preview.command) : title(activity.preview.name);

/** No background tasks. */
const NO_TASKS: readonly BackgroundTask[] = [];
const NO_ENTRIES: readonly FileEntry[] = [];
/** No previews. */
const NO_PREVIEWS: readonly Preview[] = [];

/** Previews and proven live native apps, with their own output/Stop and durable History. */
export function ActivitiesSummary({ conversationId }: { conversationId: string }) {
  const previews = useBoard(useShallow((s) => (s.board?.conversationId === conversationId ? Object.values(s.board.previews) : NO_PREVIEWS)));
  const workers = useWorkerRows(conversationId);
  const cliTasks = useCliThread((s) => s.views[conversationId]?.background ?? NO_TASKS);
  const entries = useCliThread((s) => s.views[conversationId]?.entries ?? NO_ENTRIES);
  const rows = backgroundRows(previews, (taskId) => rowName(workers, taskId));
  // A row that starts reads the clock in the same render, never a stale one that would show 0s
  // until the next tick.
  const since = [...rows, ...cliTasks, ...workers.flatMap((worker) => (worker.background ?? []).map(({ task }) => ({ ...task, id: `${worker.id}:${task.id}` })))].map((thing) => `${thing.id}@${thing.startedAtMs}`).join();
  // eslint-disable-next-line react/purity
  const [clock, setClock] = useState(() => ({ since, now: Date.now() }));
  // eslint-disable-next-line react/purity
  if (clock.since !== since) setClock({ since, now: Date.now() });
  // Ended app tasks leave immediately; the native records remain in History.
  const activities = [...rows, ...appTaskRows(cliTasks, entries, clock.now), ...workerAppTaskRows(workers, (taskId) => rowName(workers, taskId), clock.now)]
    .toSorted((a, b) => b.startedAtMs - a.startedAtMs || b.id.localeCompare(a.id));
  const changed = JSON.stringify([previews.map((preview) => [preview.id, preview.startedAtMs]), cliTasks.map((task) => [task.id, task.startedAtMs]), workers.map((row) => [row.id, row.background?.map(({ task }) => [task.id, task.startedAtMs])])]);
  const historyCount = useBackgroundHistoryCount(conversationId, changed);
  const ticking = activities.length > 0;
  useEffect(() => {
    if (!ticking) return;
    const timer = setInterval(() => setClock((shown) => ({ ...shown, now: Date.now() })), 1000);
    return () => clearInterval(timer);
  }, [ticking]);
  if (activities.length === 0 && historyCount === 0) return null;
  return (
    <SummarySection data-activities foldKey="activities" title="Background" count={activities.length} unfoldOnIncrease={activities.length}>
      {activities.map((activity) => (
        <BackgroundRow key={`${activity.id}:${activity.startedAtMs}`} conversationId={conversationId} activity={activity} now={clock.now} />
      ))}
      <SummaryRowButton data-slot="background-history-link" icon={<Clock />} meta={historyCount === null ? undefined : String(historyCount)}
        onClick={() => openPanelTab(conversationId, { type: "backgroundHistory" }, "Background history")}>
        History
      </SummaryRowButton>
    </SummarySection>
  );
}

/**
 * Only the name truncates; a live dot needs no word, and every other state, and the worker whose
 * it is, keeps its room.
 */
function ActivityLabel({ name, state, worker, word = state }: { name: string; state: BackgroundState; worker?: string | null | undefined; word?: string }) {
  return (
    <span className="flex min-w-0 items-center gap-1">
      <span data-activity-name className="truncate">{name}</span>
      {worker && <span data-activity-worker className="text-muted-foreground shrink-0 whitespace-nowrap">{worker}</span>}
      {state !== "running" && (
        <span data-state={state} className={cn("text-muted-foreground shrink-0 whitespace-nowrap", live(state) && "shimmer")}>
          {word}
        </span>
      )}
    </span>
  );
}

function BackgroundRow({ conversationId, activity, now }: { conversationId: string; activity: Activity; now: number }) {
  const name = label(activity);
  // A task that ended shows how long it ran.
  const meta = elapsedWords((activity.type === "task" ? (activity.task.endedAtMs ?? now) : now) - activity.startedAtMs);
  if (activity.type === "task") return <TaskRow conversationId={conversationId} task={activity.task} worker={activity.worker} command={activity.command} name={name} meta={meta} state={activity.state} />;
  const tip = [
    `${previewWord(activity.preview)}: ${activity.preview.command}`,
    activity.worker && `From ${activity.worker}`,
    ...activity.with.map((copy: Preview) => `${title(copy.name)}: ${copy.command}`),
  ].filter(Boolean).join("\n");
  const stop = () => stopPreview(conversationId, activity.id);
  const output = () => openPanelTab(conversationId, { type: "output", source: "preview", id: activity.id, startedAtMs: activity.startedAtMs }, name);
  const url = activity.preview.kind === "web" && activity.preview.state.type === "running" ? activity.preview.url : undefined;
  return (
    <SummaryRowButton
      data-activity={activity.id}
      title={tip}
      icon={<StateIcon state={activity.state} />}
      meta={meta}
      action={
        <>
          {url && (
            <TooltipIconButton tooltip="Show output" aria-label={`Show output of ${name}`} size="icon-xs" onClick={output}>
              <Terminal />
            </TooltipIconButton>
          )}
          <StopButton name={name} stop={stop} />
        </>
      }
      onClick={url
        ? () => openLink(url, { agent: true, conversationId }).catch((reason: unknown) => toast(`Couldn't open it: ${String(reason)}`, { tone: "error" }))
        : output}
    >
      <ActivityLabel name={name} state={activity.state} />
    </SummaryRowButton>
  );
}

/**
 * A row for one of the CLI's background tasks, or a worker's CLI's: its last line on hover, Stop
 * while it runs, and a command's output in a tab on a click. Native shell/terminal output opens when its output file is available.
 */
function TaskRow({ conversationId, task, worker, command, name, meta, state }: {
  conversationId: string;
  task: BackgroundTask;
  worker: { id: string; name: string | null } | null;
  command?: string | null | undefined;
  name: string;
  meta: string;
  state: BackgroundState;
}) {
  const said = [command, task.lastLine ?? task.summary].filter(Boolean).join("\n") || task.label;
  const parts = {
    "data-activity": worker ? `task:${worker.id}:${task.id}` : `task:${task.id}`,
    title: worker?.name ? `${said}\nFrom ${worker.name}` : said,
    icon: <StateIcon state={state} />,
    meta,
    action: task.status === "running" ? <StopButton name={name} stop={() => stopBackgroundTask(conversationId, task.id, worker?.id, task.startedAtMs)} /> : undefined,
  };
  const end = taskEnd(task);
  const words = <ActivityLabel name={name} state={state} worker={worker?.name} {...(end && { word: endWords(end) })} />;
  if ((task.kind !== "shell" && task.kind !== "terminal") || !task.outputFile) return <SummaryRow {...parts}>{words}</SummaryRow>;
  return (
    <SummaryRowButton {...parts} onClick={() => openPanelTab(conversationId, { type: "output", source: "task", id: task.id, startedAtMs: task.startedAtMs, ...(worker && { worker: worker.id }) }, name)}>
      {words}
    </SummaryRowButton>
  );
}

/** A row's Stop. Its row leaves once it has stopped: the focus goes back to the card, not the page. */
function StopButton({ name, stop }: { name: string; stop: () => Promise<void> }) {
  const [stopping, setStopping] = useState(false);
  return (
    <Button variant="ghost" size="xs" disabled={stopping} aria-label={`Stop ${name}`}
      onClick={(event) => {
        const button = event.currentTarget;
        const card = button.closest<HTMLElement>('[data-slot="summary-card"]');
        setStopping(true);
        stop().then(
          () => requestAnimationFrame(() => {
            if (!button.isConnected && card?.isConnected && !card.contains(document.activeElement)) card.focus({ preventScroll: true });
          }),
          (reason: unknown) => {
            setStopping(false);
            toast(`Couldn't stop it: ${String(reason)}`, { tone: "error" });
          },
        );
      }}>
      Stop
    </Button>
  );
}
