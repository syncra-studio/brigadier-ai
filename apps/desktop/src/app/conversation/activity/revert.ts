import type { OrchestratorStepKind } from "@/ipc/generated";

export type Merged = Extract<OrchestratorStepKind, { type: "merged" }>;

/**
 * The request Revert sends as the user's: what to take back, where, and how it went in (a pull
 * request's revert is pushed; a merge made on this Mac is taken back here).
 */
export function revertRequest(merge: Merged): string | null {
  if (!merge.mergeCommit) return null;
  const sha = merge.mergeCommit.slice(0, 7);
  return merge.pr === undefined
    ? `Revert merge ${sha} on ${merge.base} the way it went in`
    : `Revert merge ${sha} (PR #${merge.pr}) on ${merge.base} and push it the way it went in`;
}
