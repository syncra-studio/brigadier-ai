import { useEffect, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { type BackgroundRow, type BackgroundState, backgroundRows } from "@/app/conversation/backgroundRows";
import { commandWords, elapsedWords } from "@/app/conversation/liveStatus";
import { previewWord } from "@/app/conversation/previewStatus";
import { workerName } from "@/app/conversation/rowWords";
import { StateIcon, SummaryRowButton, SummarySection } from "@/components/conversation";
import { Terminal } from "@/components/icons";
import { TooltipIconButton, Button } from "@/components/controls";
import { openUrl } from "@/ipc/client";
import type { Preview } from "@/ipc/generated";
import { cancelMerge, mergeAnyway, stopBackgroundRun, stopPreview } from "@/state/actions";
import { useBoard } from "@/state/board";
import { openLink } from "@/state/links";
import { openPanelTab } from "@/state/sessionTabs";
import { toast } from "@/state/toasts";
import { cn } from "@/lib/utils";

/** One thing the session has going: a preview (the thread's or a worker's), a long command, or an event (checks, a review, CI, a merge). */
type Activity = BackgroundRow;

/** A process or an event at work: its word shimmers. Paused, it rests. */
const live = (state: BackgroundState) => state !== "paused";

/** What a row is for, first letter up: "Dev app", "Core tests". A command stands as it is. */
const title = (words: string) => words.charAt(0).toUpperCase() + words.slice(1);

const label = (activity: Activity) =>
  activity.type === "event"
    ? activity.name
    : activity.type === "preview"
      ? activity.preview.name === activity.preview.command ? commandWords(activity.preview.command) : title(activity.preview.name)
      : activity.run.name ? title(activity.run.name) : commandWords(activity.run.command);

/**
 * "Background": what the session has going in the background, one row per thing: the thread's
 * and the workers' previews, the thread's long commands, and its own events while they run (the
 * full checks, a review, the CI watch, a held merge or one at work), each with its state in a
 * word and an icon (a worker's copy of an app or a browser at it is a state of the app's row),
 * how long it has gone, and its button on hover: Stop on a process, Open CI on the CI watch,
 * Cancel on a held merge, and Merge anyway, always shown, on a merge refused for want of checks. A click on a web preview that is up opens its page in the browser
 * (Brigadier's, by Settings › Browser), with what it prints one button away; an event opens the
 * Progress tab; any other row opens what it prints in a tab. The rows come from the live board:
 * one leaves as soon as its thing ends, finished, failed or stopped (the thread's fold keeps the
 * history).
 */
export function ActivitiesSummary({ conversationId }: { conversationId: string }) {
  const board = useBoard(useShallow((s) => {
    if (s.board?.conversationId !== conversationId) return null;
    const { previews, backgroundRuns, tasks, questions, reviews, integratedChecks, orchestratorSteps, requests, mergeRefused } = s.board;
    return { previews, backgroundRuns, tasks, questions, reviews, integratedChecks, orchestratorSteps, requests, mergeRefused };
  }));
  const activities = board
    ? backgroundRows(Object.values(board.previews), Object.values(board.backgroundRuns), board.tasks, (task) => workerName(board.tasks, task), board)
    : [];
  // A row that starts reads the clock in the same render, never a stale one that would show 0s
  // until the next tick.
  const since = activities.map((activity) => `${activity.id}@${activity.startedAtMs}`).join();
  // eslint-disable-next-line react/purity
  const [clock, setClock] = useState(() => ({ since, now: Date.now() }));
  // eslint-disable-next-line react/purity
  if (clock.since !== since) setClock({ since, now: Date.now() });
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
  const meta = elapsedWords(now - activity.startedAtMs);
  const state = (
    <span data-state={activity.state} className={cn("text-muted-foreground", live(activity.state) && "shimmer")}>
      {" "}{activity.state}{activity.type === "event" && activity.words ? ` · ${activity.words}` : ""}
    </span>
  );
  if (activity.type === "event") {
    // What waits on the user's Merge anyway keeps no clock, and its name keeps the room.
    const asks = activity.action?.type === "mergeAnyway";
    return (
      <SummaryRowButton
        data-activity={activity.id}
        title={activity.tip}
        icon={<StateIcon state={activity.state} />}
        meta={asks ? undefined : meta}
        action={activity.action ? <EventButton conversationId={conversationId} action={activity.action} /> : undefined}
        // A refused merge waits on the user: its Merge anyway shows all the time, not only on hover.
        actionVisible={asks}
        onClick={() => openPanelTab(conversationId, { type: "reviews" }, "Progress")}
      >
        {name}
        {state}
      </SummaryRowButton>
    );
  }
  const tip = activity.type === "preview"
    ? [
      `${previewWord(activity.preview)}: ${activity.preview.command}`,
      activity.worker && `From ${activity.worker}`,
      ...activity.with.map((copy: Preview) => `${title(copy.name)}: ${copy.command}`),
    ].filter(Boolean).join("\n")
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
 * An event row's button: Open CI opens the CI run in the browser, Cancel takes back the held
 * merge, Merge anyway merges without the checks.
 */
function EventButton({ conversationId, action }: { conversationId: string; action: Extract<Activity, { type: "event" }>["action"] }) {
  const [busy, setBusy] = useState(false);
  if (action?.type === "openCi") {
    return (
      <Button variant="ghost" size="xs" aria-label="Open CI" onClick={() => void openUrl(action.url).catch((reason: unknown) => toast(`Couldn't open it: ${String(reason)}`, { tone: "error" }))}>
        Open CI
      </Button>
    );
  }
  if (!action) return null;
  const [send, words, failed] = action.type === "cancelMerge"
    ? [cancelMerge, "Cancel", "Couldn't cancel it"]
    : [mergeAnyway, "Merge anyway", "Couldn't merge it"];
  return (
    <Button variant="ghost" size="xs" disabled={busy} aria-label={action.type === "cancelMerge" ? "Cancel the merge" : "Merge anyway"}
      onClick={() => {
        setBusy(true);
        send(conversationId).then(() => setBusy(false), (reason: unknown) => {
          setBusy(false);
          toast(`${failed}: ${String(reason)}`, { tone: "error" });
        });
      }}>
      {words}
    </Button>
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
