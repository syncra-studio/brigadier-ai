import { type FC, useMemo } from "react";

import { elapsedWords, liveHead, type StatusHead, type StatusInput, type ThreadStatusView, threadStatus } from "@/app/conversation/liveStatus";
import { useWorkerText } from "@/app/conversation/WorkerChip";
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
  /** The turn is over, and this many background commands it started still hold it (D21). */
  commands?: number;
  /** The turn is over, and this many subagents it started still work (D24). */
  subagents?: number;
  /** What holds it over, named when it is one thing (`heldWait`, D106). */
  named?: StatusHead | null;
  /** What its CLI is blocked on in its open turn, named (`liveWait`, D106). */
  wait?: StatusHead | null;
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
  const commands = props?.commands ?? 0;
  const subagents = props?.subagents ?? 0;
  const named = props?.named ?? null;
  return useMemo(() => liveHead(head ? { ...head, text: words } : null, workers, commands, subagents, named), [head, words, workers, commands, subagents, named]);
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

