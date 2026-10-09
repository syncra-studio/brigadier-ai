import { ChevronLeft, ChevronRight, Cursor, Pause, Play } from "@openai/apps-sdk-ui/components/Icon";
import { useEffect, useState } from "react";

import {
  actionWords,
  type Batch,
  batchesOf,
  outcomeWords,
  PLAY_MS,
  type Playback,
  playbackKey,
  playStep,
  playToggle,
  routeWords,
  summaryWords,
} from "@/app/conversation/computerSteps";
import { CHEVRON, OPENS, ROW } from "@/components/assistant-ui/elements/activity-row";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import type { ComputerAction } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { loadEarlierComputerActions, openComputerLog, readStoredImage } from "@/state/actions";
import { useBoard } from "@/state/board";

const NO_ACTIONS: ComputerAction[] = [];

/** A stored screenshot as an object URL while it is shown; null until it is read. */
function useStoredImage(hash: string | null): string | null {
  const [read, setRead] = useState<{ hash: string; url: string } | null>(null);
  useEffect(() => {
    if (!hash) return;
    let live = true;
    let made: string | null = null;
    readStoredImage(hash)
      .then((blob) => {
        if (!live) return;
        made = URL.createObjectURL(blob);
        setRead({ hash, url: made });
      })
      .catch((error: unknown) => console.error("could not read a computer screenshot", error));
    return () => {
      live = false;
      if (made) URL.revokeObjectURL(made);
    };
  }, [hash]);
  return read && read.hash === hash ? read.url : null;
}

/** One action of the timeline: what it did and what came of it; the shown batch's also say how. */
function StepRow({ action, shown, onShow }: { action: ComputerAction; shown: boolean; onShow: () => void }) {
  const outcome = outcomeWords(action);
  return (
    <li data-slot="computer-step">
      <button
        type="button"
        aria-current={shown ? "step" : undefined}
        onClick={onShow}
        className={cn("rounded-control hover:text-foreground flex w-full min-w-0 flex-col px-1.5 py-0.5 text-start", shown && "bg-foreground/5 text-foreground/80")}
      >
        <span className="flex min-w-0 items-baseline gap-1.5">
          <span className="min-w-0 truncate">{actionWords(action)}</span>
          <span aria-hidden className="text-foreground/40">·</span>
          <span className={cn("shrink-0", outcome.failed ? "text-destructive" : "text-foreground/50")}>{outcome.text}</span>
        </span>
        {shown && <span data-slot="computer-step-detail" className="text-foreground/45 truncate text-xs">{routeWords(action)}</span>}
      </button>
    </li>
  );
}

/** The shown batch's screenshot, with where each action aimed marked; opens full size. */
function Shot({ batch, url, onOpen }: { batch: Batch | undefined; url: string | null; onOpen: () => void }) {
  if (!batch?.image) {
    return <div className="text-foreground/45 bg-foreground/3 rounded-control grid h-24 place-items-center text-xs">No screenshot for this step</div>;
  }
  return (
    <button
      type="button"
      data-slot="computer-shot"
      aria-label="Open the screenshot full size"
      onClick={onOpen}
      className="rounded-control border-border bg-foreground/3 block w-full overflow-hidden border"
    >
      {url ? (
        <img src={url} alt="Where these steps aimed, marked on the window" className="max-h-96 w-full object-contain" />
      ) : (
        <div className="h-48" aria-hidden />
      )}
    </button>
  );
}

/** The player's controls: previous, play or pause, next, and where it is. */
function Controls({ playback, count, onChange }: { playback: Playback; count: number; onChange: (next: Playback) => void }) {
  const { index, playing } = playback;
  return (
    <div
      role="toolbar"
      tabIndex={-1}
      aria-label="Computer timeline"
      data-slot="computer-controls"
      className="flex items-center gap-1"
      onKeyDown={(event) => {
        const next = playbackKey(playback, event.key, count);
        if (!next) return;
        event.preventDefault();
        onChange(next);
      }}
    >
      <Button size="xs" variant="ghost" aria-label="Previous step" disabled={index === 0} onClick={() => onChange({ index: index - 1, playing: false })}>
        <ChevronLeft aria-hidden />
      </Button>
      <Button size="xs" variant="ghost" aria-label={playing ? "Pause" : "Play"} disabled={count < 2} onClick={() => onChange(playToggle(playback, count))}>
        {playing ? <Pause aria-hidden /> : <Play aria-hidden />}
      </Button>
      <Button size="xs" variant="ghost" aria-label="Next step" disabled={index >= count - 1} onClick={() => onChange({ index: index + 1, playing: false })}>
        <ChevronRight aria-hidden />
      </Button>
      <span className="text-foreground/50 ps-1 text-xs tabular-nums" aria-live="polite">
        Step {Math.min(index + 1, count)} of {count}
      </span>
    </div>
  );
}

/**
 * The worker's computer use as one disclosure: its line says what it did (live: what it does
 * now); open, it replays the timeline batch by batch, each with its marked screenshot.
 */
export function ComputerTimelineView({
  actions,
  live,
  looks,
  earlier,
  onEarlier,
  defaultOpen = false,
}: {
  actions: readonly ComputerAction[];
  live: boolean;
  /** How many times it looked (apps, observe, zoom) without acting: those aren't in the log. */
  looks: number;
  earlier: boolean;
  onEarlier: () => void;
  defaultOpen?: boolean;
}) {
  const batches = batchesOf(actions);
  const [open, setOpen] = useState(defaultOpen);
  const [playback, setPlayback] = useState<Playback>({ index: Math.max(0, batches.length - 1), playing: false });
  const [full, setFull] = useState(false);
  const count = batches.length;
  // A new batch while it is closed or at the end: show the newest.
  const [seen, setSeen] = useState(count);
  if (seen !== count) {
    setSeen(count);
    if (!playback.playing && (playback.index >= seen - 1 || !open)) setPlayback({ index: Math.max(0, count - 1), playing: false });
  }
  const index = Math.min(playback.index, Math.max(0, count - 1));
  const batch = batches[index];
  const url = useStoredImage(open ? (batch?.image ?? null) : null);
  useEffect(() => {
    if (!playback.playing) return;
    const timer = setTimeout(() => setPlayback((state) => playStep(state, count)), PLAY_MS);
    return () => clearTimeout(timer);
  }, [playback, count]);

  const label = actions.length === 0 && looks > 0 && !live
    ? `Used the computer · looked ${looks} ${looks === 1 ? "time" : "times"}`
    : summaryWords(actions, live);
  return (
    <Collapsible data-slot="computer-timeline" open={open} onOpenChange={setOpen}>
      <CollapsibleTrigger className={cn(ROW, "group hover:text-foreground rounded-control w-full text-start")}>
        <Cursor aria-hidden className="size-4 shrink-0" />
        <span className={cn("min-w-0 truncate", live && "shimmer")}>{label}</span>
        {count > 0 && <ChevronRight aria-hidden className={CHEVRON} />}
      </CollapsibleTrigger>
      {count > 0 && (
        <CollapsibleContent className={OPENS}>
          <div className="flex min-w-0 flex-col gap-2 pt-2 ps-5.5">
            <Shot batch={batch} url={url} onOpen={() => setFull(true)} />
            <Controls playback={{ index, playing: playback.playing }} count={count} onChange={setPlayback} />
            {earlier && (
              <Button size="xs" variant="ghost" className="self-start" onClick={onEarlier}>
                Load earlier
              </Button>
            )}
            <ol data-slot="computer-steps" className="text-foreground/60 flex max-h-72 min-w-0 flex-col overflow-y-auto text-sm">
              {batches.map((b, i) =>
                b.actions.map((action) => (
                  <StepRow
                    key={`${b.key}:${action.index}:${action.atMs}`}
                    action={action}
                    shown={i === index}
                    onShow={() => setPlayback({ index: i, playing: false })}
                  />
                )),
              )}
            </ol>
          </div>
          <Dialog open={full && url !== null} onOpenChange={setFull}>
            <DialogContent className="max-w-thread flex max-h-full flex-col p-2">
              <DialogTitle className="sr-only">Screenshot of step {index + 1}</DialogTitle>
              {url && <img src={url} alt="Where these steps aimed, marked on the window" className="min-h-0 w-full flex-1 object-contain" />}
            </DialogContent>
          </Dialog>
        </CollapsibleContent>
      )}
    </Collapsible>
  );
}

/** A worker's computer timeline, read from its action log and kept up live. */
export function ComputerTimeline({ conversationId, taskId, live, looks }: { conversationId: string; taskId: string; live: boolean; looks: number }) {
  const log = useBoard((s) => s.board?.computer[taskId]);
  useEffect(() => {
    void openComputerLog(conversationId, taskId).catch((error: unknown) => console.error("could not read the computer timeline", error));
  }, [conversationId, taskId]);
  return (
    <ComputerTimelineView
      actions={log?.actions ?? NO_ACTIONS}
      live={live}
      looks={looks}
      earlier={log?.earlier != null}
      onEarlier={() => void loadEarlierComputerActions(conversationId, taskId)}
    />
  );
}
