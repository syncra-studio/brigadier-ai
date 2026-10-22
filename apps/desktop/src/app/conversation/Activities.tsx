import { type ReactNode, useEffect, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { type BackgroundRow, type BackgroundState, backgroundRows, taskRows } from "@/app/conversation/backgroundRows";
import { commandWords, elapsedWords } from "@/app/conversation/liveStatus";
import { previewWord } from "@/app/conversation/previewStatus";
import { rowName } from "@/app/conversation/activity/workerState";
import { StateIcon, SummaryRow, SummaryRowButton, SummarySection } from "@/components/conversation";
import { Terminal } from "@/components/icons";
import { TooltipIconButton, Button } from "@/components/controls";
import type { BackgroundTask, Preview } from "@/ipc/generated";
import { stopBackgroundTask, stopPreview } from "@/state/actions";
import { useBoard } from "@/state/board";
import { useCliThread, useWorkerRows } from "@/state/cliThread";
import { openLink } from "@/state/links";
import { openPanelTab } from "@/state/sessionTabs";
import { toast } from "@/state/toasts";
import { cn } from "@/lib/utils";

/**
 * One thing the session has going: a preview (the thread's or a worker's), or a background task
 * of its CLI's.
 */
type Activity = BackgroundRow;

/** A process at work: its word shimmers. Paused or done, it rests. */
const live = (state: BackgroundState) => state !== "paused" && state !== "done";

/** What a row is for, first letter up: "Dev app", "Core tests". A command stands as it is. */
const title = (words: string) => words.charAt(0).toUpperCase() + words.slice(1);

const label = (activity: Activity) =>
  activity.type === "task"
      ? activity.task.kind === "terminal" ? commandWords(activity.task.label) : activity.task.label
      : activity.preview.name === activity.preview.command ? commandWords(activity.preview.command) : title(activity.preview.name);

/** No background tasks. */
const NO_TASKS: readonly BackgroundTask[] = [];
/** No previews. */
const NO_PREVIEWS: readonly Preview[] = [];

/**
 * "Background": what the session has going in the background, one row per thing: the thread's
 * and the workers' previews (`brigadier preview`) and its CLI's own background tasks (by the CLI's
 * rules, `taskShown`), each with its state in a word and an icon (a worker's copy of an app or a
 * browser at it is a state of the app's row), how long it has gone, and Stop on hover. A click on
 * a web preview that is up opens its page in the browser (Brigadier's, by Settings › Browser),
 * with what it prints one button away; any other preview opens what it prints in a tab. A row
 * leaves as soon as its thing ends, finished, failed or stopped (the thread's fold keeps the
 * history).
 */
export function ActivitiesSummary({ conversationId }: { conversationId: string }) {
  const previews = useBoard(useShallow((s) => (s.board?.conversationId === conversationId ? Object.values(s.board.previews) : NO_PREVIEWS)));
  const workers = useWorkerRows(conversationId);
  const cliTasks = useCliThread((s) => s.views[conversationId]?.background ?? NO_TASKS);
  const rows = backgroundRows(previews, (taskId) => rowName(workers, taskId));
  // A row that starts reads the clock in the same render, never a stale one that would show 0s
  // until the next tick.
  const since = [...rows, ...cliTasks].map((thing) => `${thing.id}@${thing.startedAtMs}`).join();
  // eslint-disable-next-line react/purity
  const [clock, setClock] = useState(() => ({ since, now: Date.now() }));
  // eslint-disable-next-line react/purity
  if (clock.since !== since) setClock({ since, now: Date.now() });
  const activities = [...rows, ...taskRows(cliTasks, clock.now)].toSorted((a, b) => b.startedAtMs - a.startedAtMs || b.id.localeCompare(a.id));
  const ticking = activities.length > 0;
  useEffect(() => {
    if (!ticking) return;
    const timer = setInterval(() => setClock((shown) => ({ ...shown, now: Date.now() })), 1000);
    return () => clearInterval(timer);
  }, [ticking]);
  if (activities.length === 0) return null;
  return (
    <SummarySection data-activities foldKey="activities" title="Background" count={activities.length} unfoldOnIncrease={activities.length}>
      {activities.map((activity) => (
        <BackgroundRow key={activity.id} conversationId={conversationId} activity={activity} now={clock.now} />
      ))}
    </SummarySection>
  );
}

function BackgroundRow({ conversationId, activity, now }: { conversationId: string; activity: Activity; now: number }) {
  const name = label(activity);
  // A task that ended shows how long it ran.
  const meta = elapsedWords((activity.type === "task" ? (activity.task.endedAtMs ?? now) : now) - activity.startedAtMs);
  const state = (
    <span data-state={activity.state} className={cn("text-muted-foreground", live(activity.state) && "shimmer")}>
      {" "}{activity.state}
    </span>
  );
  if (activity.type === "task") return <TaskRow conversationId={conversationId} task={activity.task} name={name} meta={meta} state={activity.state} words={state} />;
  const tip = [
    `${previewWord(activity.preview)}: ${activity.preview.command}`,
    activity.worker && `From ${activity.worker}`,
    ...activity.with.map((copy: Preview) => `${title(copy.name)}: ${copy.command}`),
  ].filter(Boolean).join("\n");
  const stop = () => stopPreview(conversationId, activity.id);
  const output = () => openPanelTab(conversationId, { type: "output", source: "preview", id: activity.id }, name);
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
      {name}
      {state}
    </SummaryRowButton>
  );
}

/**
 * A row for one of the CLI's background tasks: its last line on hover, Stop while it runs, and a
 * command's output in a tab on a click. A subagent or a Codex terminal has no output to open.
 */
function TaskRow({ conversationId, task, name, meta, state, words }: {
  conversationId: string;
  task: BackgroundTask;
  name: string;
  meta: string;
  state: BackgroundState;
  words: ReactNode;
}) {
  const parts = {
    "data-activity": `task:${task.id}`,
    title: task.lastLine ?? task.summary ?? task.label,
    icon: <StateIcon state={state} />,
    meta,
    action: task.status === "running" ? <StopButton name={name} stop={() => stopBackgroundTask(conversationId, task.id)} /> : undefined,
  };
  if (task.kind !== "shell" || !task.outputFile) return <SummaryRow {...parts}>{name}{words}</SummaryRow>;
  return (
    <SummaryRowButton {...parts} onClick={() => openPanelTab(conversationId, { type: "output", source: "task", id: task.id }, name)}>
      {name}
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
