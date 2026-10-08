import { useShallow } from "zustand/react/shallow";

import type { OrchestratorStep, ReviewRun } from "@/ipc/generated";
import { useBoard } from "@/state/board";

/**
 * The code reviews of one batch of the session's work, started from `since` up to `until`:
 * those the orchestrator hears. Each landed change gets one, by the other vendor, in the
 * background, and so does each range of commits the thread made itself (a review with no task,
 * started before the merge that takes them). A worker's review of its own work in progress is
 * not one: the worker answered it before it reported. (When a landing reuses a worker's review of
 * the same commits, the daemon hands that review to the orchestrator.)
 */
export function batchReviews(
  reviews: readonly ReviewRun[],
  since: number,
  until: number,
): ReviewRun[] {
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

/** The context card's review lines; null where there is nothing to say. */
export type ReviewLines = {
  /** The work the last merge took: its review may still run, and its outcome stays. */
  merged: string | null;
  /** The work since the last merge (or since the session began). */
  current: string | null;
};

/**
 * The review lines of the context card, from the session's "Merged …" rows (the user asks for a
 * merge in words; there is no card to carry them). The work the last merge took keeps its line
 * until the next merge, so a review still running when the user merged shows its outcome after.
 */
export function reviewLines(
  steps: readonly OrchestratorStep[],
  reviews: readonly ReviewRun[],
): ReviewLines {
  const merges = steps
    .filter((step) => step.kind.type === "merged")
    .map((step) => step.atMs)
    .toSorted((a, b) => a - b);
  const last = merges.at(-1);
  const current = reviewStatus(batchReviews(reviews, last ?? 0, Number.POSITIVE_INFINITY));
  if (last === undefined) return { merged: null, current };
  return { merged: reviewStatus(batchReviews(reviews, merges.at(-2) ?? 0, last)), current };
}

/** The open conversation's review lines, live. */
export function useReviewLines(conversationId: string): ReviewLines {
  const [merged, current] = useBoard(
    useShallow((s) => {
      const board = s.board;
      if (!board || board.conversationId !== conversationId) return [null, null];
      const lines = reviewLines(board.orchestratorSteps, Object.values(board.reviews));
      return [lines.merged, lines.current];
    }),
  );
  return { merged, current };
}
