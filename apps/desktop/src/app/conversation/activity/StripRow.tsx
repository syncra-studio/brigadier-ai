import { memo, useContext } from "react";

import { WorkerControls } from "@/app/conversation/activity/StopWorker";
import { workerNow } from "@/app/conversation/activity/workerNow";
import { Counts } from "@/components/shared";
import { RowTime } from "@/app/conversation/RowTime";
import { AgentsPanelContext, useWorkerName } from "@/app/conversation/WorkerChip";
import { WorkerGlyph } from "@/components/conversation";
import { useNow } from "@/hooks/use-now";
import { cn } from "@/lib/utils";
import { useWorkerRows } from "@/state/cliThread";

/** A lone row's quiet hover: its words turn white under the pointer or the keyboard's focus. */
const BRIGHTENS = "group-hover/strip-row:text-foreground group-focus-visible/strip-row:text-foreground";

/**
 * One worker: `[g] Name · what it does · 1m 2s · +12 −3`, from its row (its CLI's events and git):
 * while it works its call at work, shimmering, else its state. Its pause/resume and End while
 * its CLI runs (D199).
 * Opens its thread. A `lone` row (the strip's only one) has no hover fill: under the pointer its
 * words brighten instead, as a thread row's do (#735).
 */
export const StripRow = memo(function StripRow({
  conversationId,
  taskId,
  lone = false,
}: {
  conversationId: string;
  taskId: string;
  lone?: boolean;
}) {
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
        className={cn("group/strip-row rounded-control flex min-w-0 flex-1 cursor-pointer flex-col items-stretch px-2 text-start text-sm", !lone && "hover:bg-foreground/5")}
      >
        <span className="min-h-row flex min-w-0 items-center gap-1.5">
          <WorkerGlyph taskId={taskId} className="size-4 shrink-0" />
          <span className={cn("text-foreground/80 max-w-1/3 shrink-0 truncate", lone && BRIGHTENS)}>{name}</span>
          <span aria-hidden className="text-foreground/40">·</span>
          <span className={cn("min-w-0 flex-1 truncate", state.kind === "working" ? "shimmer" : cn("text-foreground/60", lone && BRIGHTENS))}>
            {state.word}
          </span>
          <RowTime row={row} />
          {stat && stat.insertions + stat.deletions > 0 && <Counts insertions={stat.insertions} deletions={stat.deletions} />}
        </span>
      </button>
      {/* Level with the first line, however many lines the row has. */}
      <span className="h-row flex shrink-0 items-center">
        <WorkerControls row={row} name={name ?? "this worker"} />
      </span>
      </div>
    </li>
  );
});
