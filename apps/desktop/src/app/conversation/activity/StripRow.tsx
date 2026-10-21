import { memo, useContext } from "react";

import { StopWorker } from "@/app/conversation/activity/StopWorker";
import { Counts } from "@/app/conversation/Counts";
import { RowTime } from "@/app/conversation/RowTime";
import { useTaskActivityLine } from "@/app/conversation/WorkerActivity";
import { AgentsPanelContext, useWorkerName, WorkerGlyph } from "@/app/conversation/WorkerChip";
import { workerDone, workerState } from "@/app/conversation/workerPresentation";
import { workerStat } from "@/app/side-panel/WorkerSummary";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";

/** One worker: `[g] Name · what it is at · 1m 2s · +12 −3`, and its Stop while it runs. Opens its thread. */
export const StripRow = memo(function StripRow({ taskId }: { taskId: string }) {
  const name = useWorkerName(taskId);
  const { setPanel } = useContext(AgentsPanelContext);
  const { ref, first, firstWorking } = useTaskActivityLine(taskId, false);
  const done = useBoard((s) => {
    const task = s.board?.tasks[taskId];
    return task && workerDone(task) ? workerState(task) : null;
  });
  const stat = useBoard((s) => {
    const task = s.board?.tasks[taskId];
    return task ? workerStat(task, s.board?.diffs) : undefined;
  });
  return (
    <li>
      <div ref={ref} className="flex min-w-0 items-center">
      <button
        type="button"
        data-slot="workers-strip-row"
        onClick={() => setPanel(taskId)}
        className="hover:bg-foreground/5 rounded-control min-h-row flex min-w-0 flex-1 items-center gap-1.5 px-2 text-start text-sm"
      >
        <WorkerGlyph taskId={taskId} className="size-4 shrink-0" />
        <span className="text-foreground/80 max-w-1/3 shrink-0 truncate">{name}</span>
        <span aria-hidden className="text-foreground/40">·</span>
        <span className={cn("min-w-0 flex-1 truncate", done ? "text-foreground/50" : firstWorking ? "shimmer" : "text-foreground/60")}>
          {done ?? first}
        </span>
        <RowTime taskId={taskId} />
        {stat && stat.insertions + stat.deletions > 0 && <Counts insertions={stat.insertions} deletions={stat.deletions} />}
      </button>
      <StopWorker taskId={taskId} name={name ?? "this worker"} />
      </div>
    </li>
  );
});
