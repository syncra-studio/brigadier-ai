import { useContext, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { type BackgroundRow, type BackgroundState, backgroundRows, rowKey, taskRows, withHistory, workerTaskRows } from "@/app/conversation/backgroundRows";
import { endWords, taskEnd } from "@/app/conversation/backgroundTasks";
import { commandWords, elapsedWords } from "@/app/conversation/liveStatus";
import { previewWord } from "@/app/conversation/previewStatus";
import { rowName } from "@/app/conversation/activity/workerState";
import { useTicking } from "@/app/conversation/requestTime";
import { AgentsPanelContext } from "@/app/conversation/WorkerChip";
import { StateIcon, SummaryRow, SummaryRowButton, SummarySection } from "@/components/conversation";
import { Clock, Terminal } from "@/components/icons";
import { TooltipIconButton, Button } from "@/components/controls";
import type { BackgroundTask, FileEntry, Preview } from "@/ipc/generated";
import { stopBackgroundTask, stopPreview } from "@/state/actions";
import { useBoard } from "@/state/board";
import { useCliThread, useWorkerRows } from "@/state/cliThread";
import { openLink } from "@/state/links";
import { backgroundOf, subagentPanel } from "@/state/nativeWorkers";
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
      ? activity.task.kind === "terminal" ? commandWords(activity.task.label) : activity.task.kind === "agent" ? `Subagent: ${activity.task.label}` : activity.task.label
      : activity.preview.name === activity.preview.command ? commandWords(activity.preview.command) : title(activity.preview.name);

/** No background tasks. */
const NO_TASKS: readonly BackgroundTask[] = [];
const NO_ENTRIES: readonly FileEntry[] = [];
/** No previews. */
const NO_PREVIEWS: readonly Preview[] = [];

/** Previews and background commands and subagents, the session's and its workers', with their own output/Stop and durable History. */
export function ActivitiesSummary({ conversationId }: { conversationId: string }) {
  const previews = useBoard(useShallow((s) => (s.board?.conversationId === conversationId ? Object.values(s.board.previews) : NO_PREVIEWS)));
  const workers = useWorkerRows(conversationId);
  const cliTasks = useCliThread((s) => (s.views[conversationId] ? backgroundOf(s.views[conversationId]) : NO_TASKS));
  const entries = useCliThread((s) => s.views[conversationId]?.entries ?? NO_ENTRIES);
  const rows = backgroundRows(previews, (taskId) => rowName(workers, taskId));
  const switchedAtMs = useBoard((s) => s.board?.conversationId === conversationId
    ? Math.max(0, ...s.board.orchestratorSteps.flatMap((step) => step.kind.type === "providerTransition" && !step.kind.taskId ? [step.kind.startedAtMs] : []))
    : 0);
  // Every background command and subagent, the session's and its workers' (D51, D97); only
  // running ones are listed, ended ones (failed too) live in Background history (supersedes D98).
  const activities = withHistory(conversationId, [...rows, ...taskRows(cliTasks, entries), ...workerTaskRows(workers, (taskId) => rowName(workers, taskId))], switchedAtMs)
    .toSorted((a, b) => b.startedAtMs - a.startedAtMs || b.id.localeCompare(a.id));
  // Running ones count on the clock every live time shares (D94).
  const running = activities.filter((activity) => activity.type === "preview" || activity.task.status === "running");
  const now = useTicking(running.length > 0);
  // No section until something has run (D195); after that its heading keeps the History clock.
  if (!activities.length) return null;
  return (
    <SummarySection data-activities foldKey="activities" title="Background" count={running.length} unfoldOnIncrease={running.length} collapsible={running.length > 0}
      action={
        <TooltipIconButton data-slot="background-history-link" tooltip="Background history" aria-label="Background history" size="icon-xs"
          onClick={() => openPanelTab(conversationId, { type: "backgroundHistory" }, "Background history")}>
          <Clock />
        </TooltipIconButton>
      }>
      {running.map((activity) => (
        <BackgroundRow key={rowKey(activity)} conversationId={conversationId} activity={activity} now={now} />
      ))}
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
  const { setPanel } = useContext(AgentsPanelContext);
  const end = taskEnd(task);
  const words = <ActivityLabel name={name} state={state} worker={worker?.name} {...(end && { word: endWords(end) })} />;
  // A background subagent opens its transcript, running or over (D97).
  if (task.kind === "agent" && task.itemId !== null && !worker) return <SummaryRowButton {...parts} onClick={() => setPanel(subagentPanel(conversationId, task.itemId!))}>{words}</SummaryRowButton>;
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
