import { rowFinished } from "@/app/conversation/activity/workerState";
import { useAction } from "@/app/conversation/useAction";
import { TooltipIconButton } from "@/components/controls";
import { StopCircle } from "@/components/icons";
import { useNow } from "@/hooks/use-now";
import type { WorkerRow } from "@/ipc/generated";
import { isNativeWorker } from "@/state/nativeWorkers";
import { stopBackgroundTask, stopTask } from "@/state/actions";

/**
 * The user's Stop for one worker while it is at work or waits (UI audit M4), on its strip row,
 * its Workers list row and in its view's header: by its row, while its CLI runs. The thread
 * hears that the user stopped it.
 */
export function StopWorker({ row, name }: { row: WorkerRow; name: string }) {
  const now = useNow(60_000);
  const stoppable = !row.gone && !rowFinished(row, now) && (!isNativeWorker(row) || row.native.backgroundId !== null);
  const stop = useAction();
  if (!stoppable) return null;
  return (
    <>
      <TooltipIconButton
        tooltip="Stop this worker"
        aria-label={`Stop ${name}`}
        data-slot="worker-stop"
        disabled={stop.busy}
        onClick={(event) => {
          event.stopPropagation();
          stop.run(() => isNativeWorker(row)
            ? stopBackgroundTask(row.native.conversationId, row.native.backgroundId!, undefined, row.startedAtMs)
            : stopTask(row.id));
        }}
        className="text-foreground/60 hover:text-foreground shrink-0"
      >
        <StopCircle />
      </TooltipIconButton>
      {stop.error && <span role="alert" className="text-destructive shrink-0 text-xs">{stop.error}</span>}
    </>
  );
}
