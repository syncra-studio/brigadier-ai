import { type FC, useMemo } from "react";

import { answeredHead, elapsedWords, type StatusHead, type StatusInput, type ThreadStatusView, threadStatus } from "@/app/conversation/liveStatus";
import { useWorkerText } from "@/app/conversation/WorkerChip";
import { workersWaitWords } from "@/app/conversation/requestTime";
import type { BlockState } from "@/app/conversation/blocks";
import { Tasks } from "@/components/icons";
import { useNow } from "@/hooks/use-now";
import type { QuotaWait } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";

export type ThreadStatusProps = {
  requestIds: string[];
  state: BlockState;
  thinkingLive: boolean;
  compacting: boolean;
  quotaWait: QuotaWait | null;
  /** The turn is over, and this many of the workers it started still work. */
  workers?: number;
};

/** What a live turn's last line says now, its workers by name; `null` when nothing. */
export function useThreadStatusHead(props: ThreadStatusProps | null): StatusHead | null {
  // A string, so unrelated board changes and token ticks do not repaint the line.
  const json = useBoard((s) => {
    const board = s.board;
    if (!board || !props) return null;
    const input: StatusInput = { board, ...props };
    return JSON.stringify(threadStatus(input));
  });
  const view = useMemo(() => (json ? (JSON.parse(json) as ThreadStatusView) : null), [json]);
  const words = useWorkerText(view?.head?.text ?? "");
  const head = view?.head;
  const workers = props?.workers ?? 0;
  return useMemo(() => {
    if (workers > 0) return { text: workersWaitWords(workers), tone: "busy" as const };
    return head ? { ...head, text: words } : null;
  }, [head, words, workers]);
}

/** The status line on its own, when no work row says it; a long run's says how long it has gone. */
export const StatusLine: FC<{ head: StatusHead }> = ({ head }) => {
  const now = useNow(head.sinceMs === undefined ? null : 5_000);
  const text = head.sinceMs === undefined ? head.text : `${head.text} · ${elapsedWords(now - head.sinceMs)}`;
  return (
    <div data-slot="thread-status" data-tone={head.tone} data-waits-on={head.waitsOn} className="flex min-w-0 items-center gap-2">
      {/* A question waiting for its answer: a checklist before the words. */}
      {head.waitsOn === "answer" && <Tasks aria-hidden data-icon="tasks" className="size-4 shrink-0 text-muted-foreground" />}
      {/* As wide as its words, so the sweep crosses them rather than the whole row. */}
      <div
        data-slot="request-activity"
        className={cn(
          "w-fit max-w-full truncate text-sm",
          // Waiting on the user is live too: it shimmers as quietly as the work, the card itself asks.
          (head.tone === "busy" || head.tone === "needsYou") && "shimmer",
          head.waitsOn === "approval" && "shimmer-quiet",
          head.tone === "still" && "text-muted-foreground",
        )}
      >
        {text}
      </div>
    </div>
  );
};

/** The line under an answer whose request still waits (see `answeredHead`). */
export const AnsweredStatus: FC<ThreadStatusProps & { waiting: boolean; reopened: boolean }> = ({ waiting, reopened, ...props }) => {
  const head = answeredHead(useThreadStatusHead(props), { waiting, reopened });
  return head ? <StatusLine head={head} /> : null;
};
