import { useEffect, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { commandWords, elapsedWords } from "@/app/conversation/liveStatus";
import { PreviewList } from "@/app/conversation/PreviewList";
import { previewActive, previewWord } from "@/app/conversation/previewStatus";
import { workerName } from "@/app/conversation/rowWords";
import { SummarySection } from "@/components/assistant-ui/elements/summary-section";
import { TaskCard, type TaskCardState } from "@/components/assistant-ui/elements/task-card";
import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import type { BackgroundRun, Preview, Task } from "@/ipc/generated";
import { stopBackgroundRun, stopPreview } from "@/state/actions";
import { useBoard } from "@/state/board";
import { openLink } from "@/state/links";

/** One thing running for the session: a preview (the thread's or a worker's) or a long command. */
type Activity =
  | { type: "preview"; id: string; startedAtMs: number; endedAtMs: number | null; preview: Preview; worker: string | null }
  | { type: "run"; id: string; startedAtMs: number; endedAtMs: null; run: BackgroundRun };

function previewCardState(preview: Preview): TaskCardState {
  const { state } = preview;
  if (state.type === "paused") return "waiting";
  if (state.type === "starting" || state.type === "running") return "working";
  return state.type === "exited" && state.code !== 0 ? "failed" : "cancelled";
}

/** Held (paused) still counts as there, but not as running. */
const active = (activity: Activity) => activity.type === "run" || previewActive(activity.preview.state);
const live = (activity: Activity) => activity.type === "run" || activity.preview.state.type === "starting" || activity.preview.state.type === "running";

/** What a row is for, first letter up: "Dev app", "Core tests". A command stands as it is. */
const title = (words: string) => words.charAt(0).toUpperCase() + words.slice(1);

/**
 * Everything running for the session, a compact row each: the thread's and the workers' previews
 * and the thread's long commands. A row says what it is for, its state and how long it has gone;
 * opened, it shows its command on one line, with Stop, and Open for a web page that is up.
 * Folded, the header counts what runs.
 */
export function ActivitiesSummary({ conversationId }: { conversationId: string }) {
  const previews = useBoard(useShallow((s) => s.board?.conversationId === conversationId ? Object.values(s.board.previews) : []));
  const runs = useBoard(useShallow((s) => s.board?.conversationId === conversationId
    ? Object.values(s.board.backgroundRuns).filter((run) => run.state.type === "running")
    : []));
  const tasks = useBoard((s) => s.board?.conversationId === conversationId ? s.board.tasks : null);
  const activities: Activity[] = [
    ...previews.map((preview): Activity => {
      const task: Task | undefined = preview.taskId ? tasks?.[preview.taskId] : undefined;
      return { type: "preview", id: preview.id, startedAtMs: preview.startedAtMs, endedAtMs: preview.endedAtMs, preview,
        worker: task && tasks ? workerName(tasks, task) : null };
    }),
    ...runs.map((run): Activity => ({ type: "run", id: run.id, startedAtMs: run.startedAtMs, endedAtMs: null, run })),
  ].toSorted((a, b) => Number(active(b)) - Number(active(a)) || b.startedAtMs - a.startedAtMs || b.id.localeCompare(a.id));
  const running = activities.filter(live).length;
  // A row that starts reads the clock in the same render, never a stale one that would show 0s
  // until the next tick.
  const since = activities.filter(active).map((activity) => `${activity.id}@${activity.startedAtMs}`).join();
  // eslint-disable-next-line react/purity
  const [clock, setClock] = useState(() => ({ since, now: Date.now() }));
  // eslint-disable-next-line react/purity
  if (clock.since !== since) setClock({ since, now: Date.now() });
  useEffect(() => {
    if (!running) return;
    const timer = setInterval(() => setClock((shown) => ({ ...shown, now: Date.now() })), 1000);
    return () => clearInterval(timer);
  }, [running]);
  if (!activities.length) return null;
  return (
    <SummarySection data-activities foldKey="activities" title="Activities" count={activities.length} unfoldOnIncrease={activities.length}
      summary={running ? `${running} running` : undefined}>
      <div className="flex flex-col gap-0.5">
        {activities.map((activity) => activity.type === "preview"
          ? <PreviewActivity key={activity.id} preview={activity.preview} worker={activity.worker} now={clock.now} />
          : <RunActivity key={activity.id} run={activity.run} now={clock.now} />)}
      </div>
    </SummarySection>
  );
}

function PreviewActivity({ preview, worker, now }: { preview: Preview; worker: string | null; now: number }) {
  const [log, setLog] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const trigger = useRef<HTMLButtonElement | null>(null);
  const card = useRef<HTMLElement | null>(null);
  const stoppable = previewActive(preview.state);
  const url = preview.kind === "web" && preview.state.type === "running" ? preview.url : null;
  return (
    <TaskCard data-activity={preview.id} label={preview.name === preview.command ? commandWords(preview.command) : title(preview.name)} meta={previewWord(preview)} state={previewCardState(preview)}
      elapsed={elapsedWords((preview.endedAtMs ?? now) - preview.startedAtMs)} result={error}
      actions={<>
        {url && <Button variant="ghost" size="xs" onClick={() => void openLink(url, { agent: true, conversationId: preview.conversationId })}>Open</Button>}
        {stoppable && <StopButton label={preview.name} stop={() => stopPreview(preview.conversationId, preview.id)} onError={setError} />}
      </>}>
      {/* The command opens the session's previews, each with its log. */}
      <Popover open={log} onOpenChange={setLog}>
        <PopoverTrigger asChild>
          <button ref={trigger} type="button" title={`${preview.command} (in ${preview.workdir})`} aria-label={`Show previews: ${preview.name}`}
            className="text-muted-foreground hover:text-foreground flex min-w-0 flex-1 gap-1 text-start text-xs">
            {worker && <span className="shrink-0">{worker} ·</span>}
            <code className="min-w-0 flex-1 truncate font-mono">{preview.command}</code>
          </button>
        </PopoverTrigger>
        <PopoverContent
          onOpenAutoFocus={() => {
            card.current = trigger.current?.closest<HTMLElement>('[data-slot="summary-card"]') ?? null;
          }}
          onCloseAutoFocus={(event) => {
            const board = useBoard.getState().board;
            if (board?.conversationId === preview.conversationId && !board.previews[preview.id] && card.current?.isConnected) {
              event.preventDefault();
              card.current.focus({ preventScroll: true });
            }
          }}
          align="end" aria-label="Previews" className="flex max-h-(--radix-popover-content-available-height) w-96 max-w-(--radix-popover-content-available-width) flex-col overflow-y-auto rounded-2xl border-0 p-0">
          <PreviewList conversationId={preview.conversationId} onOpenLink={() => setLog(false)} />
        </PopoverContent>
      </Popover>
    </TaskCard>
  );
}

function RunActivity({ run, now }: { run: BackgroundRun; now: number }) {
  const [error, setError] = useState<string | null>(null);
  return (
    <TaskCard data-activity={run.id} label={run.name ? title(run.name) : commandWords(run.command)} meta="Running" state="working"
      elapsed={elapsedWords(now - run.startedAtMs)} result={error}
      actions={<StopButton label={run.name ?? run.command} stop={() => stopBackgroundRun(run.conversationId, run.id)} onError={setError} />}>
      <code title={run.command} className="text-muted-foreground min-w-0 flex-1 truncate font-mono text-xs">{run.command}</code>
    </TaskCard>
  );
}

function StopButton({ label, stop, onError }: { label: string; stop: () => Promise<void>; onError: (error: string | null) => void }) {
  const [stopping, setStopping] = useState(false);
  return (
    <Button variant="ghost" size="xs" disabled={stopping} aria-label={`Stop ${label}`}
      onClick={() => {
        setStopping(true);
        onError(null);
        stop().catch((reason: unknown) => {
          setStopping(false);
          onError(`Couldn't stop it: ${String(reason)}`);
        });
      }}>
      Stop
    </Button>
  );
}
