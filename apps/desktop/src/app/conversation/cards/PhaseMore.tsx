import { DoneWhen } from "@/app/conversation/cards/DoneWhen";
import { PhaseWork } from "@/app/conversation/cards/PhaseWork";
import { disclosureRow } from "@/components/conversation";
import type { OvernightPhase } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/** A phase's workers and what it covers; open while the phase is at work. */
export function PhaseMore({
  phase,
  open,
  taskIds,
}: {
  phase: OvernightPhase | undefined;
  open: boolean;
  taskIds: readonly string[];
}) {
  const covers = phase && (phase.scope || phase.doneWhen.length > 0) && (
    <details>
      <summary className={cn(disclosureRow, "text-xs")}>What it covers</summary>
      <div className="text-muted-foreground flex flex-col gap-1.5 pt-1 text-xs">
        {phase.scope && <p>{phase.scope}</p>}
        <DoneWhen lines={phase.doneWhen} />
      </div>
    </details>
  );
  if (taskIds.length === 0) return covers || null;
  const work = <PhaseWork taskIds={taskIds} />;
  return (
    <div className="text-muted-foreground flex flex-col gap-1 pt-1 text-xs">
      {open ? (
        work
      ) : (
        <details>
          <summary className={disclosureRow}>Workers</summary>
          <div className="pt-1">{work}</div>
        </details>
      )}
      {covers}
    </div>
  );
}
