import { useMemo } from "react";

import type { Merged } from "@/app/conversation/activity/revert";
import type { BlockOrchestratorStep } from "@/app/conversation/blocks";
import { useBoard } from "@/state/board";
import type { ReviewTarget } from "@/state/sessionTabs";

/**
 * What a finished turn shows of its merge, in its Changes card: which merge it is and where its
 * View changes opens. What was checked lives in the Progress tab.
 */

/** A turn's merge: `own` when this turn made it, else the later merge that took this turn's edits. */
export type TurnMerge = { merge: Merged; own: boolean };

const isMerge = (kind: BlockOrchestratorStep["kind"]): kind is Merged => kind.type === "merged" && !!(kind as Merged).mergeCommit;

/**
 * The merge a turn shows: the last one it made, else the last merge (any turn's) whose requests
 * include one of this turn's, so the turn that edited the files says they merged.
 */
export function useTurnMerge(steps: readonly BlockOrchestratorStep[], requestIds: readonly string[]): TurnMerge | null {
  const own = steps.flatMap((step) => (isMerge(step.kind) ? [step.kind] : [])).at(-1);
  const key = requestIds.join(" ");
  const later = useBoard((s) => {
    if (own || !key) return null;
    const ids = key.split(" ");
    const found = (s.board?.orchestratorSteps ?? [])
      .flatMap((step) => (isMerge(step.kind) && step.kind.requests?.some((id) => ids.includes(id)) ? [step.kind] : []))
      .at(-1);
    return found ? JSON.stringify(found) : null;
  });
  return useMemo(() => (own ? { merge: own, own: true } : later ? { merge: JSON.parse(later) as Merged, own: false } : null), [own, later]);
}

/** "Merged into main", "Merged into main on this Mac", "Merged into main (PR #7)". */
export function mergeWords(merge: Merged): string {
  if (merge.pr !== undefined) return `Merged into ${merge.base} (PR #${merge.pr})`;
  return merge.upstream ? `Merged into ${merge.base} on this Mac` : `Merged into ${merge.base}`;
}

/**
 * Where a merge's View changes opens: the merge commit's own change, in a diff tab of its own.
 * A fast-forward's commit is the branch's head: its change is all the merged commits (#411).
 */
export function mergeViewTarget(merge: Merged): Extract<ReviewTarget, { type: "commit" }> {
  const commits = merge.pr == null && merge.mergeCommit === merge.head && merge.commits > 1 ? merge.commits : undefined;
  return { type: "commit", commit: merge.mergeCommit!, ...(commits ? { commits } : {}), staged: false };
}
