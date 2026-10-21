import { useEffect, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { type BackgroundRow, type BackgroundState, backgroundRows, rowEnded } from "@/app/conversation/backgroundRows";
import { commandWords, elapsedWords } from "@/app/conversation/liveStatus";
import { latestLine, previewWord } from "@/app/conversation/previewStatus";
import { workerName } from "@/app/conversation/rowWords";
import { StateIcon, SummaryRowButton, SummarySection } from "@/components/conversation";
import { Terminal, X } from "@/components/icons";
import { TooltipIconButton, Button } from "@/components/controls";
import { request } from "@/ipc/client";
import type { Preview } from "@/ipc/generated";
import { dismissBackground, previewLogTail, restartBackgroundRun, restartPreview, stopBackgroundRun, stopPreview } from "@/state/actions";
import { useBoard } from "@/state/board";
import { openLink } from "@/state/links";
import { openPanelTab } from "@/state/sessionTabs";
import { toast } from "@/state/toasts";
import { cn } from "@/lib/utils";

/** One thing running for the session: a preview (the thread's or a worker's) or a long command. */
type Activity = BackgroundRow;

/** A process at work: its word shimmers. Paused or ended, it rests. */
const live = (state: BackgroundState) => state !== "paused" && !rowEnded(state);

/** What a row is for, first letter up: "Dev app", "Core tests". A command stands as it is. */
const title = (words: string) => words.charAt(0).toUpperCase() + words.slice(1);

const label = (activity: Activity) =>
  activity.type === "preview"
    ? activity.preview.name === activity.preview.command ? commandWords(activity.preview.command) : title(activity.preview.name)
    : activity.run.name ? title(activity.run.name) : commandWords(activity.run.command);

/**
 * "Background": what still runs for the session after its turn, one row per process: the
 * thread's and the workers' previews and the thread's long commands, each with its state in a
 * word and an icon (a worker's copy of an app or a browser at it is a state of the app's row),
 * how long it has gone, and Stop on hover. A click on a web preview that is up opens its page in
 * the browser (Brigadier's, by Settings › Browser), with what it prints one button away; any
 * other row opens what it prints in a tab. One the user stopped leaves the card; one of the
 * thread's that failed stays red with its last line, Restart and × until dealt with, and one
 * that finished on its own stays muted until the next message (Q94); the thread's fold keeps
 * the history.
 */
export function ActivitiesSummary({ conversationId }: { conversationId: string }) {
  const previews = useBoard(useShallow((s) => s.board?.conversationId === conversationId ? Object.values(s.board.previews) : []));
  const runs = useBoard(useShallow((s) => s.board?.conversationId === conversationId ? Object.values(s.board.backgroundRuns) : []));
  const tasks = useBoard((s) => s.board?.conversationId === conversationId ? s.board.tasks : null);
  const cleared = useBoard((s) => s.board?.conversationId === conversationId ? s.board.backgroundCleared : null);
  // When the user's newest message was sent: a row that finished before it has left.
  const lastSentMs = useBoard((s) => s.board?.conversationId === conversationId
    ? Object.values(s.board.requests).reduce<number | null>((last, each) => (each.fromPeer || (last !== null && last >= each.startedAtMs) ? last : each.startedAtMs), null)
    : null);
  const activities = backgroundRows(previews, runs, tasks, (task) => (tasks ? workerName(tasks, task) : null), { cleared: cleared ?? [], lastSentMs });
  // A row that starts reads the clock in the same render, never a stale one that would show 0s
  // until the next tick.
  const since = activities.map((activity) => `${activity.id}@${activity.startedAtMs}`).join();
  // eslint-disable-next-line react/purity
  const [clock, setClock] = useState(() => ({ since, now: Date.now() }));
  // eslint-disable-next-line react/purity
  if (clock.since !== since) setClock({ since, now: Date.now() });
  const ticking = activities.some((activity) => !rowEnded(activity.state));
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
  const ended = rowEnded(activity.state);
  const failed = activity.state === "failed";
  const lastLine = useLastLine(conversationId, failed ? activity : null);
  const endedAtMs = activity.type === "preview" ? activity.preview.endedAtMs : activity.run.endedAtMs;
  const tip = activity.type === "preview"
    ? [
      `${activity.state === "finished" ? "Finished" : previewWord(activity.preview)}: ${activity.preview.command}`,
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
      meta={elapsedWords((ended ? (endedAtMs ?? now) : now) - activity.startedAtMs)}
      muted={activity.state === "finished"}
      actionVisible={failed}
      action={
        failed ? (
          <EndedActions conversationId={conversationId} activity={activity} name={name} />
        ) : ended ? undefined : (
          <>
            {url && (
              <TooltipIconButton tooltip="Show output" aria-label={`Show output of ${name}`} size="icon-xs" onClick={output}>
                <Terminal />
              </TooltipIconButton>
            )}
            <StopButton name={name} stop={stop} />
          </>
        )
      }
      onClick={url
        ? () => openLink(url, { agent: true, conversationId }).catch((reason: unknown) => toast(`Couldn't open it: ${String(reason)}`, { tone: "error" }))
        : output}
    >
      {name}
      <span data-state={activity.state} className={cn(failed ? "text-destructive" : "text-muted-foreground", live(activity.state) && "shimmer")}> {activity.state}</span>
      {lastLine && <span data-last-line className="text-muted-foreground"> · {lastLine}</span>}
    </SummaryRowButton>
  );
}

/** A failed row's last line of output: read once when it fails (its log, or a run's kept output). */
function useLastLine(conversationId: string, activity: Activity | null): string | null {
  const [found, setFound] = useState<{ key: string; line: string } | null>(null);
  const endedAtMs = activity && (activity.type === "preview" ? activity.preview.endedAtMs : activity.run.endedAtMs);
  const key = activity ? `${conversationId} ${activity.id} ${endedAtMs ?? ""}` : null;
  const source = activity?.type ?? null;
  const id = activity?.id ?? null;
  useEffect(() => {
    if (!key || !source || !id) return;
    let current = true;
    const tail = source === "preview"
      ? previewLogTail(conversationId, id).then(({ tail: text }) => text)
      : request({ method: "readBackgroundRun", conversationId, runId: id }).then(({ output }) => output);
    tail.then((text) => current && setFound({ key, line: latestLine(text) }), () => {});
    return () => {
      current = false;
    };
  }, [key, conversationId, source, id]);
  return found && found.key === key && found.line ? found.line : null;
}

/** A failed row's Restart (its own command again: a preview's from its record, a run's by the thread) and its ×, which takes it off the card. */
function EndedActions({ conversationId, activity, name }: { conversationId: string; activity: Activity; name: string }) {
  const [busy, setBusy] = useState(false);
  const act = (what: string, call: () => Promise<void>) => {
    setBusy(true);
    call().catch((reason: unknown) => {
      setBusy(false);
      toast(`Couldn't ${what} it: ${String(reason)}`, { tone: "error" });
    });
  };
  const restart = activity.type === "preview"
    ? () => restartPreview(conversationId, activity.id)
    : () => restartBackgroundRun(conversationId, activity.id);
  return (
    <>
      <Button variant="ghost" size="xs" disabled={busy} aria-label={`Restart ${name}`} onClick={() => act("restart", restart)}>
        Restart
      </Button>
      <TooltipIconButton tooltip="Dismiss" aria-label={`Dismiss ${name}`} size="icon-xs" disabled={busy}
        onClick={() => act("dismiss", () => dismissBackground(conversationId, activity.id))}>
        <X />
      </TooltipIconButton>
    </>
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
