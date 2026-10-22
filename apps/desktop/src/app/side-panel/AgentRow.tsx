import { memo, useContext } from "react";

import { lowerFirst, rowFinished, workerStateOf } from "@/app/conversation/activity/workerState";
import { StopWorker } from "@/app/conversation/activity/StopWorker";
import { AgentsPanelContext, useWorkerName } from "@/app/conversation/WorkerChip";
import { WorkerGlyph } from "@/components/conversation";
import { RowTime } from "@/app/conversation/RowTime";
import { useNow } from "@/hooks/use-now";
import type { WorkerRow } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/**
 * One worker on one line, from its row: its glyph, its name (a soft "working" while it works),
 * and how long it has worked or how long ago it finished; Stop in the time's place on hover. Its
 * state in words is its tooltip. A click opens it.
 */
export const AgentRow = memo(function AgentRow({ row }: { row: WorkerRow }) {
  const name = useWorkerName(row.id);
  const { setPanel } = useContext(AgentsPanelContext);
  const now = useNow(30_000);
  const state = workerStateOf(row, now);
  const stoppable = !row.gone && !rowFinished(row, now);
  return (
    <li className="group/row relative flex min-w-0">
      <button
        type="button"
        data-task={`task-${row.number}`}
        data-worker-state={state.kind}
        aria-label={`${name} ${lowerFirst(state.word)}`}
        title={state.word}
        onClick={() => setPanel(row.id)}
        className="hover:bg-foreground/5 rounded-lg flex h-row min-w-0 flex-1 items-center gap-3 px-2 text-start transition-colors"
      >
        <WorkerGlyph taskId={row.id} className="size-5" />
        <span className="min-w-0 flex-1 truncate text-label">
          {name}
          {state.kind === "working" && <span className="text-muted-foreground shimmer"> working</span>}
        </span>
        <span className={cn(stoppable && "group-hover/row:invisible group-focus-within/row:invisible")}>
          <RowTime row={row} ago className="text-xs" />
        </span>
      </button>
      {/* Its Stop on the row, without opening the worker first. */}
      {stoppable && (
        <span className="absolute inset-y-0 end-1 hidden items-center group-focus-within/row:flex group-hover/row:flex">
          <StopWorker row={row} name={name ?? row.title} />
        </span>
      )}
    </li>
  );
});
