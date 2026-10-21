import { memo, useContext } from "react";

import { workerDone, workerWorking, workerState, workerPreview } from "@/app/conversation/workerPresentation";
import { StopWorker } from "@/app/conversation/activity/StopWorker";
import { AgentsPanelContext, useWorkerName } from "@/app/conversation/WorkerChip";
import { WorkerGlyph } from "@/components/conversation";
import { RowTime } from "@/app/conversation/RowTime";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";

/**
 * One worker on one line: its glyph, its name (a soft "working" while it works), and how long it
 * has worked or how long ago it finished; Stop in the time's place on hover. A click opens it.
 */
export const AgentRow = memo(function AgentRow({ taskId }: { taskId: string }) {
  const task = useBoard((s) => s.board?.tasks[taskId]);
  const name = useWorkerName(taskId);
  const { setPanel } = useContext(AgentsPanelContext);
  if (!task) return null;
  const working = workerWorking(task);
  const stoppable = !workerDone(task);
  return (
    <li className="group/row relative flex min-w-0">
      <button
        type="button"
        data-task={`task-${task.number}`}
        aria-label={`${name} ${workerState(task)}`}
        title={workerPreview(task) ?? undefined}
        onClick={() => setPanel(task.id)}
        className="hover:bg-foreground/5 rounded-lg flex h-row min-w-0 flex-1 items-center gap-3 px-2 text-start transition-colors"
      >
        <WorkerGlyph taskId={task.id} className="size-5" />
        <span className="min-w-0 flex-1 truncate text-label">
          {name}
          {working && <span className="text-muted-foreground shimmer"> working</span>}
        </span>
        <span className={cn(stoppable && "group-hover/row:invisible group-focus-within/row:invisible")}>
          <RowTime taskId={task.id} ago className="text-xs" />
        </span>
      </button>
      {/* Its Stop on the row, without opening the worker first. */}
      {stoppable && (
        <span className="absolute inset-y-0 end-1 hidden items-center group-focus-within/row:flex group-hover/row:flex">
          <StopWorker taskId={task.id} name={name ?? task.title} />
        </span>
      )}
    </li>
  );
});
