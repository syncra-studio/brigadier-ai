import type { BreachRow } from "@/app/inspector/orchestratorLog";
import { formatClock } from "@/lib/format";

/** Contract breaches (the CLI compacted its context): errors, above everything else. */
export function Breaches({ breaches }: { breaches: readonly BreachRow[] }) {
  if (breaches.length === 0) return null;
  return (
    <ul role="alert" className="border-destructive/40 bg-destructive/10 rounded-control flex flex-col gap-1 border px-2.5 py-2">
      {breaches.map((breach) => (
        <li key={breach.streamSeq} className="flex gap-2">
          <span className="text-destructive shrink-0 font-medium">Contract breach</span>
          <span className="min-w-0 flex-1 wrap-break-word">{breach.message}</span>
          <span className="text-muted-foreground shrink-0 tabular-nums">{formatClock(breach.atMs)}</span>
        </li>
      ))}
    </ul>
  );
}
