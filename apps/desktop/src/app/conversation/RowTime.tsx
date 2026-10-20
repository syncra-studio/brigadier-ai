import { useShallow } from "zustand/react/shallow";

import { workerDone } from "@/app/conversation/workerPresentation";
import { useNow } from "@/hooks/use-now";
import { formatAgo, formatDuration } from "@/lib/format";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";

/**
 * How long a worker has been at it, ticking each second. Once it is done: how long ago it
 * finished with `ago` (ticking each minute), else nothing. Sized by the caller.
 */
export function RowTime({ taskId, ago = false, className }: { taskId: string; ago?: boolean; className?: string }) {
  const [start, doneAt] = useBoard(
    useShallow((s) => {
      const task = s.board?.tasks[taskId];
      if (!task) return [null, null];
      return workerDone(task)
        ? [null, task.updatedAtMs]
        : [task.attempts.at(-1)?.startedAtMs ?? task.createdAtMs, null];
    }),
  );
  const shown = start !== null || (ago && doneAt !== null);
  const now = useNow(!shown ? null : start !== null ? 1000 : 60_000);
  if (!shown) return null;
  return (
    <span className={cn("text-foreground/50 shrink-0 tabular-nums", className)}>
      {start !== null ? formatDuration(Math.max(0, now - start)) : formatAgo(doneAt ?? now, now)}
    </span>
  );
}
