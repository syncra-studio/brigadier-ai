import type { RebirthRow } from "@/app/inspector/orchestratorLog";
import { RebirthRowView } from "@/app/inspector/RebirthRowView";

/** One row per orchestrator rebirth, newest first; each opens to its briefing. */
export function RebirthLog({ rebirths }: { rebirths: readonly RebirthRow[] }) {
  return (
    <div data-selectable className="min-h-0 flex-1 overflow-y-auto text-xs">
      {rebirths.length === 0 ? (
        <p className="text-muted-foreground p-4">
          The orchestrator has not been reborn yet. It is, between turns, once its context passes
          the swap line.
        </p>
      ) : (
        <ul>
          {rebirths.toReversed().map((row) => (
            <RebirthRowView key={row.streamSeq} row={row} />
          ))}
        </ul>
      )}
    </div>
  );
}