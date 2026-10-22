import { rowFinished } from "@/app/conversation/activity/workerState";
import { useNow } from "@/hooks/use-now";
import type { WorkerRow } from "@/ipc/generated";
import { formatAgo, formatDuration } from "@/lib/format";
import { cn } from "@/lib/utils";

/**
 * How long a worker has been at it, ticking each second. Once it is finished: how long ago its
 * CLI last said anything with `ago` (ticking each minute), else nothing. Sized by the caller.
 */
export function RowTime({ row, ago = false, className }: { row: WorkerRow; ago?: boolean; className?: string }) {
  const clock = useNow(60_000);
  const finished = rowFinished(row, clock);
  const start = finished ? null : row.startedAtMs;
  const doneAt = finished ? row.lastEventAtMs : null;
  const shown = start !== null || (ago && doneAt !== null);
  const now = useNow(!shown ? null : start !== null ? 1000 : 60_000);
  if (!shown) return null;
  return (
    <span className={cn("text-foreground/50 shrink-0 tabular-nums", className)}>
      {start !== null ? formatDuration(Math.max(0, now - start)) : formatAgo(doneAt ?? now, now)}
    </span>
  );
}
