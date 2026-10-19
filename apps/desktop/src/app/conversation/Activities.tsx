import { Pause, Terminal } from "@openai/apps-sdk-ui/components/Icon";
import { useEffect, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { commandWords, elapsedWords } from "@/app/conversation/liveStatus";
import { previewActive, previewWord } from "@/app/conversation/previewStatus";
import { workerName } from "@/app/conversation/rowWords";
import { SummaryRowButton, SummarySection } from "@/components/assistant-ui/elements/summary-section";
import { Spinner } from "@/components/glyphs/spinner";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { Button } from "@/components/ui/button";
import type { BackgroundRun, Preview, Task } from "@/ipc/generated";
import { stopBackgroundRun, stopPreview } from "@/state/actions";
import { useBoard } from "@/state/board";
import { openLink } from "@/state/links";
import { openPanelTab } from "@/state/sessionTabs";
import { toast } from "@/state/toasts";

/** One thing running for the session: a preview (the thread's or a worker's) or a long command. */
type Activity =
  | { type: "preview"; id: string; startedAtMs: number; preview: Preview; worker: string | null }
  | { type: "run"; id: string; startedAtMs: number; run: BackgroundRun };

/** What a row is for, first letter up: "Dev app", "Core tests". A command stands as it is. */
const title = (words: string) => words.charAt(0).toUpperCase() + words.slice(1);

const label = (activity: Activity) =>
  activity.type === "preview"
    ? activity.preview.name === activity.preview.command ? commandWords(activity.preview.command) : title(activity.preview.name)
    : activity.run.name ? title(activity.run.name) : commandWords(activity.run.command);

/**
 * "Background": what still runs for the session after its turn, one line each: the thread's and
 * the workers' previews and the thread's long commands, with a spinner, how long it has gone, and
 * Stop on hover. A click on a web preview that is up opens its page in the browser (Brigadier's,
 * by Settings › Browser), with what it prints one button away; any other row opens what it prints
 * in a tab. A thing that ends leaves the card; the thread's fold keeps its history.
 */
export function ActivitiesSummary({ conversationId }: { conversationId: string }) {
  const previews = useBoard(useShallow((s) => s.board?.conversationId === conversationId
    ? Object.values(s.board.previews).filter((preview) => previewActive(preview.state))
    : []));
  const runs = useBoard(useShallow((s) => s.board?.conversationId === conversationId
    ? Object.values(s.board.backgroundRuns).filter((run) => run.state.type === "running")
    : []));
  const tasks = useBoard((s) => s.board?.conversationId === conversationId ? s.board.tasks : null);
  const activities: Activity[] = [
    ...previews.map((preview): Activity => {
      const task: Task | undefined = preview.taskId ? tasks?.[preview.taskId] : undefined;
      return { type: "preview", id: preview.id, startedAtMs: preview.startedAtMs, preview, worker: task && tasks ? workerName(tasks, task) : null };
    }),
    ...runs.map((run): Activity => ({ type: "run", id: run.id, startedAtMs: run.startedAtMs, run })),
  ].toSorted((a, b) => b.startedAtMs - a.startedAtMs || b.id.localeCompare(a.id));
  // A row that starts reads the clock in the same render, never a stale one that would show 0s
  // until the next tick.
  const since = activities.map((activity) => `${activity.id}@${activity.startedAtMs}`).join();
  // eslint-disable-next-line react/purity
  const [clock, setClock] = useState(() => ({ since, now: Date.now() }));
  // eslint-disable-next-line react/purity
  if (clock.since !== since) setClock({ since, now: Date.now() });
  const any = activities.length > 0;
  useEffect(() => {
    if (!any) return;
    const timer = setInterval(() => setClock((shown) => ({ ...shown, now: Date.now() })), 1000);
    return () => clearInterval(timer);
  }, [any]);
  if (!any) return null;
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
  const paused = activity.type === "preview" && activity.preview.state.type === "paused";
  const tip = activity.type === "preview"
    ? [`${previewWord(activity.preview)}: ${activity.preview.command}`, activity.worker && `From ${activity.worker}`].filter(Boolean).join("\n")
    : activity.run.command;
  const stop = activity.type === "preview"
    ? () => stopPreview(conversationId, activity.id)
    : () => stopBackgroundRun(conversationId, activity.id);
  const output = () => openPanelTab(conversationId, { type: "output", source: activity.type, id: activity.id }, name);
  const url = activity.type === "preview" && activity.preview.kind === "web" && activity.preview.state.type === "running" ? activity.preview.url : undefined;
  return (
    <SummaryRowButton
      data-activity={activity.id}
      title={tip}
      icon={paused ? <Pause /> : <Spinner />}
      meta={elapsedWords(now - activity.startedAtMs)}
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
