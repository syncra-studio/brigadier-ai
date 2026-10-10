import { type FC, useMemo } from "react";

import { type StatusInput, type ThreadStatusView, threadStatus } from "@/app/conversation/liveStatus";
import { useWorkerText } from "@/app/conversation/WorkerChip";
import type { BlockState } from "@/app/conversation/blocks";
import type { QuotaWait } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";

/**
 * The last line of a live turn, one line: what happens right now ("Thinking", "Delegating to a
 * worker"), what waits on the user ("Waiting for your approval"), or how many workers the turn
 * waits for (each worker's progress is in the Workers strip on the composer). It is gone once
 * the turn is over. Token ticks never announce a live region.
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
  const head = view?.head;
  if (!head) return null;
  return (
    <div data-slot="thread-status" data-tone={head.tone} className="flex min-w-0">
      {/* As wide as its words, so the sweep crosses them rather than the whole row. */}
      <div
        data-slot="request-activity"
        className={cn(
          "w-fit max-w-full truncate text-sm",
          // Waiting on the user is live too: it shimmers as quietly as the work, the card itself asks.
          (head.tone === "busy" || head.tone === "needsYou") && "shimmer",
          head.tone === "still" && "text-muted-foreground",
        )}
      >
        {words}
      </div>
    </div>
  );
};
