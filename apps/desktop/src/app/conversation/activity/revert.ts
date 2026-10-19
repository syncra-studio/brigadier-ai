import type { MergeUndo, OrchestratorStepKind } from "@/ipc/generated";

export type Merged = Extract<OrchestratorStepKind, { type: "merged" }>;

/**
 * The request Revert sends as the user's: what to take back, where, and how it went in (a pull
 * request's revert is pushed; a merge made on this Mac is taken back here).
 */
export function revertRequest(merge: Merged): string | null {
  if (!merge.mergeCommit) return null;
  return merge.pr === undefined
    ? `Revert merge ${merge.mergeCommit.slice(0, 7)} on ${merge.base} the way it went in`
    : revertPullRequest(merge.mergeCommit, merge.pr, merge.base);
}

/** Revert's request for a pull request merged on GitHub (the merge message's, the card's). */
export function revertPullRequest(mergeCommit: string, pr: number, base: string): string {
  return `Revert merge ${mergeCommit.slice(0, 7)} (PR #${pr}) on ${base} and push it the way it went in`;
}

/**
 * The request Redo sends as the user's on a reverted merge: the thread reverts the revert and
 * merges that the way the merge went in, under the same rules as any merge (it is the user's
 * message, seen in the thread). None while the merge isn't reverted.
 */
export function redoRequest(merge: Merged, undo: MergeUndo | null): string | null {
  if (!merge.mergeCommit || !undo || undo.redoneBy !== undefined) return null;
  const revert = undo.revertedBy.slice(0, 7);
  return merge.pr === undefined
    ? `Redo merge ${merge.mergeCommit.slice(0, 7)} on ${merge.base}: revert its revert ${revert} the way it went in`
    : `Redo merge ${merge.mergeCommit.slice(0, 7)} (PR #${merge.pr}) on ${merge.base}: revert its revert ${revert} and push it the way it went in`;
}
