import type { MergeRecord } from "@/ipc/generated";

/**
 * The one plain line the thread shows under a merge when something is really wrong: a review
 * finding left open, a check, CI or the verifier failing. A check that ran on an earlier commit
 * or a missing review is not wrong: it stays in the Reviews tab. Null when nothing is.
 */
export function mergeWarning(record: MergeRecord): string | null {
  const open = record.reviews.reduce((sum, review) => sum + review.open, 0);
  const failed = record.checks.filter((check) => check.status !== "exit 0").map((check) => check.command);
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
