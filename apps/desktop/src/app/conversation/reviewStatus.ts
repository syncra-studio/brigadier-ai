import { useMemo } from "react";

import type { Finding, OrchestratorStep, ReviewRun } from "@/ipc/generated";
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

/** One finding as the review line's list shows it: what it is and what the thread recorded. */
export type FindingRow = {
  id: string;
  title: string;
  location: string | null;
  /** "fixed in task-7", "open", "dismissed: …", "new": the thread's words, or not recorded yet. */
  status: string;
  open: boolean;
};

/** The findings of these reviews, as the thread recorded them (SESSION-FIXES-PLAN.md Q7). */
export function findingRows(reviews: readonly ReviewRun[]): FindingRow[] {
  return reviews.flatMap((review) =>
    review.items.map((item) => ({
      id: item.id,
      title: item.title,
      location: item.location,
      status: findingStatus(item.status),
      open: item.status.type === "open",
    })),
  );
}

function findingStatus(status: Finding["status"]): string {
  switch (status.type) {
    case "new":
      return "new";
    case "fixed":
      return status.task ? `fixed in ${status.task}` : "fixed";
    case "open":
      return status.why ? `open: ${status.why}` : "open";
    case "dismissed":
      return `dismissed: ${status.why}`;
  }
}

/** How many findings the thread recorded as open, and how many it hasn't recorded yet. */
function counts(reviews: readonly ReviewRun[]): { open: number; unrecorded: number } {
  let open = 0;
  let unrecorded = 0;
  for (const review of reviews) {
    for (const item of review.items) {
      if (item.status.type === "open") open += 1;
      if (item.status.type === "new") unrecorded += 1;
    }
    // A review stored before findings were items: what it found was never recorded.
    if (review.items.length === 0 && review.state.type === "findings") unrecorded += review.state.count;
  }
  return { open, unrecorded };
}

/**
 * The reviews' outcome in a few words, from what the thread recorded of each finding (Q7):
 * "Review running…" while one runs (merging doesn't wait for it), then "Review: clean" or
 * "Review: 2 open", with the findings the thread hasn't recorded yet as "new". Never a total
 * of what the reviews found. Null when nothing was reviewed.
 */
export function reviewStatus(reviews: readonly ReviewRun[]): string | null {
  if (reviews.length === 0) return null;
  if (reviews.some((review) => review.state.type === "running")) return "Review running…";
  const { open, unrecorded } = counts(reviews);
  const parts = [...(open > 0 ? [`${open} open`] : []), ...(unrecorded > 0 ? [`${unrecorded} new`] : [])];
  if (parts.length > 0) return `Review: ${parts.join(" · ")}`;
  if (reviews.every((review) => review.state.type === "failed")) return "Review couldn’t run";
  return "Review: clean";
}

/**
 * The merged work's line: "Merged · review clean", or "Merged · 2 open (not fixed)" for what the
 * thread left open; a review that ends after the merge says so too.
 */
export function mergedStatus(reviews: readonly ReviewRun[]): string | null {
  if (reviews.length === 0) return null;
  if (reviews.some((review) => review.state.type === "running")) return "Merged · review running…";
  const { open, unrecorded } = counts(reviews);
  const parts = [...(open > 0 ? [`${open} open (not fixed)`] : []), ...(unrecorded > 0 ? [`${unrecorded} new`] : [])];
  if (parts.length > 0) return `Merged · ${parts.join(" · ")}`;
  if (reviews.every((review) => review.state.type === "failed")) return "Merged · review couldn’t run";
  return "Merged · review clean";
}

/** One review line of the context card, with the findings a click lists. */
export type ReviewLine = { text: string; findings: FindingRow[] };

/** The context card's review lines; null where there is nothing to say. */
export type ReviewLines = {
  /** The work the last merge took: its review may still run, and its outcome stays. */
  merged: ReviewLine | null;
  /** The work since the last merge (or since the session began). */
  current: ReviewLine | null;
};

function line(batch: readonly ReviewRun[], text: string | null): ReviewLine | null {
  return text === null ? null : { text, findings: findingRows(batch) };
}

/**
 * The review lines of the context card, from the session's "Merged …" rows (the user asks for a
 * merge in words; there is no card to carry them). The work the last merge took keeps its line
 * until the next merge, so a review still running when the user merged shows its outcome after.
 * Each line counts only its own batch: nothing adds up across merges.
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
  const now = batchReviews(reviews, last ?? 0, Number.POSITIVE_INFINITY);
  const current = line(now, reviewStatus(now));
  if (last === undefined) return { merged: null, current };
  const before = batchReviews(reviews, merges.at(-2) ?? 0, last);
  return { merged: line(before, mergedStatus(before)), current };
}

/** The open conversation's review lines, live. */
export function useReviewLines(conversationId: string): ReviewLines {
  const json = useBoard((s) => {
    const board = s.board;
    if (!board || board.conversationId !== conversationId) return null;
    return JSON.stringify(reviewLines(board.orchestratorSteps, Object.values(board.reviews)));
  });
  return useMemo(() => (json ? (JSON.parse(json) as ReviewLines) : { merged: null, current: null }), [json]);
}
