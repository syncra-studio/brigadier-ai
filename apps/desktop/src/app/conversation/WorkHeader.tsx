import type { FC } from "react";

import { useBlockError } from "@/app/conversation/BlockError";
import { isLive, turnTime, waitsOnCard } from "@/app/conversation/blocks";
import { headerLabel, useTicking } from "@/app/conversation/requestTime";
import type { BlockMeta } from "@/app/conversation/RequestBlock";
import { interruptedByQuit } from "@/app/conversation/turnError";
import { useAuiState } from "@/components/conversation";
import { ChevronRightAlt } from "@/components/icons";
import { cn } from "@/lib/utils";

/**
 * The row over a request's work, with a rule under it: "Working for 12s" while live, also while
 * what it started still runs (its live line says what it waits on), "Worked for 3m 4s ›" once
 * its work folds, "Waiting for quota · 12s" while its messages wait for a model that can take them.
 */
export const WorkHeader: FC<{
  meta: BlockMeta;
  open: boolean;
  foldable: boolean;
  /** Its messages wait for quota, and no worker of it runs meanwhile. */
  quota: boolean;
  /** Its turn is over, but workers, subagents or background commands it started still hold it open (D24). */
  held: boolean;
  /** When the last of the background subagents that held it ended, if after its turns (D53). */
  heldUntil?: number | null;
  onToggle: () => void;
  /** Its calls the user denied: "{n} denied actions" on the right, which shows the first. */
  denied: number;
  onDenied: () => void;
}> = ({ meta, open, foldable, quota, held, heldUntil = null, onToggle, denied, onDenied }) => {
  const now = useTicking(isLive(meta.state) || held);
  // Held open, it still works: its clock goes on from its start, never restarts; once what held
  // it ended, it worked until then, so the time never drops back (D53).
  const from = meta.worked[0]?.fromMs ?? meta.startedAtMs;
  const elapsed = held ? Math.max(0, now - from) : Math.max(turnTime(meta, now), meta.state === "done" && heldUntil !== null ? heldUntil - from : 0);
  // Waiting on its own question card, or held open by what it started, the turn still reads as working.
  const state = waitsOnCard(meta) || held ? "working" : meta.state;
  const failed = useAuiState((s) => {
    const status = s.message.status;
    return status?.type === "incomplete" && status.reason === "error" ? String(status.error ?? "") : null;
  });
  const interrupted = interruptedByQuit(useBlockError(meta, failed)?.error);
  const label = headerLabel(state, elapsed, quota, interrupted);
  const text = (
    <span
      data-slot="request-work-label"
      className={cn(
        "text-foreground/50 text-sm leading-(--spacing-activity-row) tabular-nums",
        foldable && "group-hover:text-foreground group-focus-visible:text-foreground",
      )}
    >
      {label}
    </span>
  );
  // The label, 8px, then a hairline rule across the column.
  const rule = "border-work-rule flex items-center border-b pb-2";
  const deniedButton = denied > 0 && (
    <button
      type="button"
      data-slot="request-denied"
      onClick={onDenied}
      className="text-foreground/50 hover:text-foreground focus-visible:text-foreground ms-auto shrink-0 text-sm leading-(--spacing-activity-row) tabular-nums transition-colors"
    >
      {denied === 1 ? "1 denied action" : `${denied} denied actions`}
    </button>
  );
  if (!foldable) {
    return (
      <div data-slot="request-work-header" className={rule}>
        {text}
        {deniedButton}
      </div>
    );
  }
  const toggle = (
    <button
      type="button"
      data-slot="request-work-header"
      aria-expanded={open}
      onClick={onToggle}
      className={cn(deniedButton ? "flex items-center" : rule, "group gap-1 text-start")}
    >
      {text}
      <ChevronRightAlt
        aria-hidden
        className={cn(
          "text-foreground/50 size-chevron group-hover:text-foreground group-focus-visible:text-foreground shrink-0 transition-[rotate,color] duration-150 ease-standard motion-reduce:transition-none",
          open && "rotate-90",
        )}
      />
    </button>
  );
  if (!deniedButton) return toggle;
  return (
    <div className={cn(rule, "gap-3")}>
      {toggle}
      {deniedButton}
    </div>
  );
};
