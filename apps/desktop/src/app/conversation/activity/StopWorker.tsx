import { StopCircle } from "@openai/apps-sdk-ui/components/Icon";

import { useAction } from "@/app/conversation/useAction";
import { workerStoppable } from "@/app/conversation/workerPresentation";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { stopTask } from "@/state/actions";
import { useBoard } from "@/state/board";

/**
 * The user's Stop for one worker while it runs or waits to run (UI audit M4), on its strip row
 * and in its view's header. The thread hears that the user stopped it.
 */
export function StopWorker({ taskId, name }: { taskId: string; name: string }) {
  const stoppable = useBoard((s) => {
    const task = s.board?.tasks[taskId];
    return !!task && workerStoppable(task);
  });
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
          stop.run(() => stopTask(taskId));
        }}
        className="text-foreground/60 hover:text-foreground shrink-0"
      >
        <StopCircle />
      </TooltipIconButton>
      {stop.error && <span role="alert" className="text-destructive shrink-0 text-xs">{stop.error}</span>}
    </>
  );
}
