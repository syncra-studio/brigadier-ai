import { rowOnHand, workerStateOf } from "@/app/conversation/activity/workerState";
import { useAction } from "@/app/conversation/useAction";
import { TooltipIconButton } from "@/components/controls";
import { PauseCircle, PlayCircle, X } from "@/components/icons";
import { useNow } from "@/hooks/use-now";
import type { WorkerRow } from "@/ipc/generated";
import { isNativeWorker } from "@/state/nativeWorkers";
import { pauseTask, resumeTask, stopBackgroundTask, stopTask } from "@/state/actions";

const BUTTON = "text-foreground/60 hover:text-foreground shrink-0";

/**
 * The user's End for one worker while it is at work, waits or is paused (UI audit M4, D199), on
 * its strip row, its context card row and its Workers list row (not its view's header): no
 * confirm, its work stays on its branch. The thread says "You ended ‹worker›".
 */
export function StopWorker({ row, name }: { row: WorkerRow; name: string }) {
  const now = useNow(60_000);
  const stoppable = !row.gone && rowOnHand(row, now) && (!isNativeWorker(row) || row.native.backgroundId !== null);
  const stop = useAction();
  if (!stoppable) return null;
  return (
    <>
      <TooltipIconButton
        tooltip="End worker · its work stays on its branch"
        aria-label={`End ${name}`}
        data-slot="worker-stop"
        disabled={stop.busy}
        onClick={(event) => {
          event.stopPropagation();
          stop.run(() => isNativeWorker(row)
            ? stopBackgroundTask(row.native.conversationId, row.native.backgroundId!, undefined, row.startedAtMs)
            : stopTask(row.id));
        }}
        className={BUTTON}
      >
        <X />
      </TooltipIconButton>
      {stop.error && <span role="alert" className="text-destructive shrink-0 text-xs">{stop.error}</span>}
    </>
  );
}

/**
 * The user's pause/resume toggle for one worker (D199): the same engine pause and resume as the
 * thread's "pause it" and "resume it", one "Paused …" or "Resumed …" line. Not for a CLI's own
 * subagent, which has no pause.
 */
export function PauseWorker({ row, name }: { row: WorkerRow; name: string }) {
  const now = useNow(60_000);
  const toggle = useAction();
  if (row.gone || isNativeWorker(row) || !rowOnHand(row, now)) return null;
  const paused = workerStateOf(row, now).kind === "paused";
  return (
    <>
      <TooltipIconButton
        tooltip={paused ? "Resume worker" : "Pause worker"}
        aria-label={`${paused ? "Resume" : "Pause"} ${name}`}
        data-slot="worker-pause"
        data-state={paused ? "paused" : "running"}
        disabled={toggle.busy}
        onClick={(event) => {
          event.stopPropagation();
          toggle.run(() => (paused ? resumeTask(row.id) : pauseTask(row.id)));
        }}
        className={BUTTON}
      >
        {paused ? <PlayCircle /> : <PauseCircle />}
      </TooltipIconButton>
      {toggle.error && <span role="alert" className="text-destructive shrink-0 text-xs">{toggle.error}</span>}
    </>
  );
}

/** A worker's pause/resume toggle and its End, side by side (D199). */
export function WorkerControls({ row, name }: { row: WorkerRow; name: string }) {
  return (
    <span data-slot="worker-controls" className="flex shrink-0 items-center">
      <PauseWorker row={row} name={name} />
      <StopWorker row={row} name={name} />
    </span>
  );
}
