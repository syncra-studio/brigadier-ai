import type { OrchestratorStepKind } from "@/ipc/generated";

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
