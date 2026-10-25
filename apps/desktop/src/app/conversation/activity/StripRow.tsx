import { memo, useContext } from "react";

import { StopWorker } from "@/app/conversation/activity/StopWorker";
import { workerNow } from "@/app/conversation/activity/workerNow";
import { Counts } from "@/components/shared";
import { RowTime } from "@/app/conversation/RowTime";
import { AgentsPanelContext, useWorkerName } from "@/app/conversation/WorkerChip";
import { WorkerGlyph } from "@/components/conversation";
import { useNow } from "@/hooks/use-now";
import { cn } from "@/lib/utils";
import { useWorkerRows } from "@/state/cliThread";

/**
 * One worker: `[g] Name · what it does · 1m 2s · +12 −3`, from its row (its CLI's events and git):
 * while it works its call at work, shimmering, else its state. Its Stop while its CLI runs.
 * Opens its thread.
 */
export const StripRow = memo(function StripRow({ conversationId, taskId }: { conversationId: string; taskId: string }) {
  const name = useWorkerName(taskId);
  const { setPanel } = useContext(AgentsPanelContext);
  const row = useWorkerRows(conversationId).find((each) => each.id === taskId);
  const now = useNow(30_000);
  if (!row) return null;
  const state = workerNow(row, now);
  const stat = row.diff;
  return (
    <li>
      <div className="flex min-w-0 items-start">
      <button
        type="button"
        data-slot="workers-strip-row"
        data-worker-state={state.kind}
        onClick={() => setPanel(taskId)}
        className="hover:bg-foreground/5 rounded-control flex min-w-0 flex-1 flex-col items-stretch px-2 text-start text-sm"
      >
        <span className="min-h-row flex min-w-0 items-center gap-1.5">
          <WorkerGlyph taskId={taskId} className="size-4 shrink-0" />
          <span className="text-foreground/80 max-w-1/3 shrink-0 truncate">{name}</span>
          <span aria-hidden className="text-foreground/40">·</span>
          <span className={cn("min-w-0 flex-1 truncate", state.kind === "working" ? "shimmer" : "text-foreground/60")}>
            {state.word}
          </span>
          <RowTime row={row} />
          {stat && stat.insertions + stat.deletions > 0 && <Counts insertions={stat.insertions} deletions={stat.deletions} />}
        </span>
      </button>
      {/* Level with the first line, however many lines the row has. */}
      <span className="h-row flex shrink-0 items-center">
        <StopWorker row={row} name={name ?? "this worker"} />
      </span>
      </div>
    </li>
  );
});
