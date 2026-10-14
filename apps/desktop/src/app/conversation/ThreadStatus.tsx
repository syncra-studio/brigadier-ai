import { type FC, useMemo } from "react";

import { elapsedWords, type StatusHead, type StatusInput, type ThreadStatusView, threadStatus } from "@/app/conversation/liveStatus";
import { waitingWords } from "@/app/conversation/activity/peerWords";
import { useWorkerText } from "@/app/conversation/WorkerChip";
import type { BlockState } from "@/app/conversation/blocks";
import { useNow } from "@/hooks/use-now";
import type { QuotaWait } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";

const NONE: readonly string[] = [];

export type ThreadStatusProps = {
  requestIds: string[];
  state: BlockState;
  thinkingLive: boolean;
  compacting: boolean;
  quotaWait: QuotaWait | null;
  /** The sessions whose replies it waits for, by title. */
  waitingFor?: readonly string[];
};

/** What a live turn's last line says now, its workers by name; `null` when nothing. */
export function useThreadStatusHead(props: ThreadStatusProps | null): StatusHead | null {
  // A string, so unrelated board changes and token ticks do not repaint the line.
  const json = useBoard((s) => {
    const board = s.board;
    if (!board || !props) return null;
    const input: StatusInput = { board, ...props, waitingFor: props.waitingFor ?? NONE };
    return JSON.stringify(threadStatus(input));
  });
  const view = useMemo(() => (json ? (JSON.parse(json) as ThreadStatusView) : null), [json]);
  const words = useWorkerText(view?.head?.text ?? "");
  const head = view?.head;
  return useMemo(() => (head ? { ...head, text: words } : null), [head, words]);
}

/** The status line on its own, when no work row says it; a long run's says how long it has gone. */
export const StatusLine: FC<{ head: StatusHead }> = ({ head }) => {
  const now = useNow(head.sinceMs === undefined ? null : 5_000);
  const text = head.sinceMs === undefined ? head.text : `${head.text} · ${elapsedWords(now - head.sinceMs)}`;
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
        {text}
      </div>
    </div>
  );
};

/**
 * The last line of a live turn, one line: what happens right now ("Thinking", "Delegating to a
 * worker"), what waits on the user ("Waiting for your approval"), or how many workers the turn
 * waits for (each worker's progress is in the Workers strip on the composer). It is gone once
 * the turn is over. Token ticks never announce a live region.
 */
export const ThreadStatus: FC<ThreadStatusProps> = (props) => {
  const head = useThreadStatusHead(props);
  return head ? <StatusLine head={head} /> : null;
};

/**
 * The line under an answer whose request still waits: what waits on the user first, else the
 * sessions whose replies it waits for (also while the answer streams), else a long run the
 * request works on until its result comes, else what it waits on.
 */
export const AnsweredStatus: FC<ThreadStatusProps & { waiting: boolean }> = ({ waiting, ...props }) => {
  const head = useThreadStatusHead(props);
  const waitingFor = props.waitingFor ?? NONE;
  if (head?.tone === "needsYou") return <StatusLine head={head} />;
  if (waitingFor.length > 0) return <StatusLine head={{ text: waitingWords(waitingFor), tone: "still" }} />;
  if (head?.sinceMs !== undefined) return <StatusLine head={head} />;
  return waiting && head ? <StatusLine head={head} /> : null;
};
