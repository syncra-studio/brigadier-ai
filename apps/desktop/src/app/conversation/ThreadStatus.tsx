import { type FC, memo, useMemo } from "react";

import { WorkerLink } from "@/app/conversation/TaskRow";
import { type StatusInput, type ThreadStatusView, threadStatus } from "@/app/conversation/liveStatus";
import { useTaskActivityLine } from "@/app/conversation/WorkerActivity";
import { useWorkerText, WorkerGlyph } from "@/app/conversation/WorkerChip";
import type { BlockState } from "@/app/conversation/blocks";
import type { QuotaWait } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";

/** A worker the turn waits for: its name, then what it is at, shimmering only while it works. */
const WorkerStatus = memo(function WorkerStatus({ taskId }: { taskId: string }) {
  const { ref, first, firstWorking } = useTaskActivityLine(taskId);
  return (
    <div ref={ref} data-slot="thread-status-worker" className="text-foreground/65 flex min-w-0 items-center gap-1.5 text-sm leading-5">
      <WorkerGlyph taskId={taskId} className="size-4 shrink-0" />
      <span className="shrink-0"><WorkerLink taskId={taskId} /></span>
      {first && (
        <span className={cn("min-w-0 truncate", firstWorking ? "shimmer" : "text-muted-foreground")}>{first}</span>
      )}
    </div>
  );
});

/**
 * The last line of a live turn: what happens right now ("Thinking", "Delegating to a worker"),
 * what waits on the user ("Waiting for your approval"), or the workers the turn waits for, each
 * with what it is at. It is gone once the turn is over. Token ticks never announce a live region.
 */
export const ThreadStatus: FC<{
  requestIds: string[];
  state: BlockState;
  thinkingLive: boolean;
  compacting: boolean;
  quotaWait: QuotaWait | null;
}> = ({ requestIds, state, thinkingLive, compacting, quotaWait }) => {
  // A string, so unrelated board changes and token ticks do not repaint the line.
  const json = useBoard((s) => {
    const board = s.board;
    if (!board) return null;
    const input: StatusInput = { board, requestIds, state, thinkingLive, compacting, quotaWait };
    return JSON.stringify(threadStatus(input));
  });
  const view = useMemo(() => (json ? (JSON.parse(json) as ThreadStatusView) : null), [json]);
  const words = useWorkerText(view?.head?.text ?? "");
  if (!view || (!view.head && view.workers.length === 0)) return null;
  const { head, workers, more } = view;
  return (
    <div data-slot="thread-status" data-tone={head?.tone} className="flex min-w-0 flex-col gap-1.5">
      {head && (
        // As wide as its words, so the sweep crosses them rather than the whole row.
        <div
          data-slot="request-activity"
          className={cn(
            "w-fit max-w-full truncate text-sm",
            head.tone === "busy" && "shimmer",
            head.tone === "still" && "text-muted-foreground",
            head.tone === "needsYou" && "text-foreground/80",
          )}
        >
          {words}
        </div>
      )}
      {workers.map((taskId) => <WorkerStatus key={taskId} taskId={taskId} />)}
      {more > 0 && <div className="text-muted-foreground text-sm">and {more} more</div>}
    </div>
  );
};
