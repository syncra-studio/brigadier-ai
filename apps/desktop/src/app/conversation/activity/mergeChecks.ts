import type { MergeRecord, ProviderKind } from "@/ipc/generated";

const short = (sha: string) => sha.slice(0, 7);
const findings = (open: number) => (open === 1 ? "one finding still open" : `${open} findings still open`);
/** The checks that failed on the merged code: one that failed on earlier code says nothing of it. */
const failedOnHead = (record: MergeRecord) => record.checks.filter((check) => check.onHead && check.status !== "exit 0");

/**
 * What is really wrong with a merged commit, one line each: a review's findings left open, a
 * check, CI or the verifier failing. A check that ran on an earlier commit or a missing review
 * is not wrong, and nothing else lists. The Progress tab's Merging step shows these lines;
 * `vendors` names each reviewer.
 */
export function mergeProblems(record: MergeRecord, vendors: Readonly<Record<ProviderKind, string>>): string[] {
  return [
    ...failedOnHead(record).map((check) => `${check.command}: ${check.status}`),
    ...record.reviews
      .filter((review) => review.open > 0)
      .map((review) => `${vendors[review.reviewer]} review of ${short(review.from)}..${short(review.to)}: ${findings(review.open)}`),
    ...(record.ci?.result === "failed" ? [`CI failed on ${short(record.ci.sha)}`] : []),
    ...(record.verifier?.result === "failed" ? [`Fresh verifier: failed on ${short(record.verifier.commit)}`] : []),
  ];
}

/**
 * The one plain line the thread shows under a merge when something is really wrong (the same
 * things as {@link mergeProblems}). Null when nothing is.
 */
export function mergeWarning(record: MergeRecord): string | null {
  const open = record.reviews.reduce((sum, review) => sum + review.open, 0);
  const failed = failedOnHead(record).map((check) => check.command);
  const parts = [
    ...(open > 0 ? [open === 1 ? "one review finding is still open" : `${open} review findings are still open`] : []),
    ...(failed.length === 1 ? [`${failed[0]} failed`] : failed.length > 1 ? [`${failed.length} checks failed`] : []),
    ...(record.ci?.result === "failed" ? ["CI failed"] : []),
    ...(record.verifier?.result === "failed" ? ["the verifier found a problem"] : []),
  ];
  if (parts.length === 0) return null;
  const text = parts.length === 1 ? parts[0]! : `${parts.slice(0, -1).join(", ")} and ${parts.at(-1)}`;
  return `${text.charAt(0).toUpperCase()}${text.slice(1)}.`;
}
