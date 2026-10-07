import type { Approval, ReviewRun } from "@/ipc/generated";
import { useBoard } from "@/state/board";

/**
 * The code reviews a merge card speaks for: every landing's since the session's previous merge,
 * up to the card's own answer. Each landed change gets one, by the other vendor, in the
 * background; one still running at the merge stays the card's until it ends. A worker's review of
 * its own work in progress is not one: the worker answered it before it reported. (When a landing
 * reuses a worker's review of the same commits, the daemon hands that review to the orchestrator.)
 */
export function mergeReviews(
  card: Approval,
  approvals: readonly Approval[],
  reviews: readonly ReviewRun[],
): ReviewRun[] {
  const since = approvals
    .filter(
      (other) =>
        other.id !== card.id &&
        other.subject.type === "finishSession" &&
        other.state.type === "allowed" &&
        other.createdAtMs < card.createdAtMs,
    )
    .reduce((latest, other) => Math.max(latest, other.resolvedAtMs ?? other.createdAtMs), 0);
  const until = card.resolvedAtMs ?? Number.POSITIVE_INFINITY;
  return reviews.filter(
    (review) =>
      review.kind === "code" &&
      review.notify.type === "orchestrator" &&
      review.startedAtMs >= since &&
      review.startedAtMs < until,
  );
}

/**
 * The reviews' outcome in a few words: "Review running…" while one runs (merging doesn't wait
 * for it), then "Review: clean" or how many findings. Null when nothing was reviewed.
 */
export function reviewStatus(reviews: readonly ReviewRun[]): string | null {
  if (reviews.length === 0) return null;
  if (reviews.some((review) => review.state.type === "running")) return "Review running…";
  const findings = reviews.reduce(
    (sum, review) => sum + (review.state.type === "findings" ? review.state.count : 0),
    0,
  );
  if (findings > 0) return `Review: ${findings} finding${findings === 1 ? "" : "s"}`;
  if (reviews.every((review) => review.state.type === "failed")) return "Review couldn’t run";
  return "Review: clean";
}

/** A merge card's review line, live; null for any other card. */
export function useMergeReviewStatus(card: Approval | undefined): string | null {
  return useBoard((s) => {
    const board = s.board;
    if (!board || card?.subject.type !== "finishSession") return null;
    return reviewStatus(
      mergeReviews(card, Object.values(board.approvals), Object.values(board.reviews)),
    );
  });
}
