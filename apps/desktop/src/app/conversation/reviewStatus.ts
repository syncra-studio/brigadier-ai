import { useMemo } from "react";

import type { Finding, IntegratedCheck, MergedUpstream, OrchestratorStep, ReviewRun } from "@/ipc/generated";
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

/** What the thread recorded of a finding, as its chip says it. */
export type FindingLabel = "Open" | "New" | "Fixed" | "Unverified" | "Dismissed";

/** One finding as the review line's list shows it: what it is and what the thread recorded. */
export type FindingRow = {
  id: string;
  /** `P0` to `P3`; null when the review's text couldn't be split. */
  severity: string | null;
  title: string;
  location: string | null;
  status: FindingLabel;
  /** The thread's words on it, shown on hover: why it stays open or was dismissed, what fixed it. */
  note: string | null;
  open: boolean;
};

/** The findings of these reviews, as the thread recorded them (SESSION-FIXES-PLAN.md Q7). */
export function findingRows(reviews: readonly ReviewRun[]): FindingRow[] {
  return reviews.flatMap((review) =>
    review.items.map((item) => ({
      id: item.id,
      severity: item.severity,
      title: item.title,
      location: item.location,
      ...findingStatus(item.status),
      open: item.status.type === "open",
    })),
  );
}

function findingStatus(status: Finding["status"]): { status: FindingLabel; note: string | null } {
  switch (status.type) {
    case "new":
      return { status: "New", note: null };
    case "fixed": {
      const task = status.task ? `Fixed in ${status.task}` : null;
      const said = [task, status.why].filter(Boolean).join(": ");
      // A fix the thread took on a report's word, never checked in the merged code.
      if (!status.checked) {
        return { status: "Unverified", note: [said, "Not checked against the merged code"].filter(Boolean).join(". ") };
      }
      return { status: "Fixed", note: [said, `Checked: ${status.checked}`].filter(Boolean).join(". ") };
    }
    case "open":
      return { status: "Open", note: status.why };
    case "dismissed":
      return { status: "Dismissed", note: status.why };
  }
}

/**
 * How many findings the thread recorded as open, how many as fixed without a check in the merged
 * code, and how many it hasn't recorded yet; then how many it fixed and checked, and how many it
 * dismissed.
 */
function counts(reviews: readonly ReviewRun[]): { open: number; unverified: number; unrecorded: number; fixed: number; dismissed: number } {
  let open = 0;
  let unverified = 0;
  let unrecorded = 0;
  let fixed = 0;
  let dismissed = 0;
  for (const review of reviews) {
    for (const item of review.items) {
      if (item.status.type === "open") open += 1;
      if (item.status.type === "fixed" && !item.status.checked) unverified += 1;
      if (item.status.type === "fixed" && item.status.checked) fixed += 1;
      if (item.status.type === "dismissed") dismissed += 1;
      if (item.status.type === "new") unrecorded += 1;
    }
    // A review stored before findings were items: what it found was never recorded.
    if (review.items.length === 0 && review.state.type === "findings") unrecorded += review.state.count;
  }
  return { open, unverified, unrecorded, fixed, dismissed };
}

/** What the thread did with the findings it settled: "2 fixed · 1 dismissed", or nothing. */
function settled({ fixed, dismissed }: { fixed: number; dismissed: number }): string[] {
  return [...(fixed > 0 ? [`${fixed} fixed`] : []), ...(dismissed > 0 ? [`${dismissed} dismissed`] : [])];
}

/**
 * A batch's outcome in a word or two, from what the thread recorded of each finding (Q7):
 * "running…" while a review runs (merging doesn't wait for it), then "2 open · 1 unverified ·
 * 1 new", "2 fixed · 1 dismissed", "clean" or "couldn’t run". Clean is only a review that found
 * nothing: findings fixed or dismissed say so, and a fix nobody checked reads unverified.
 * Null when nothing was reviewed.
 */
function outcome(reviews: readonly ReviewRun[]): string | null {
  if (reviews.length === 0) return null;
  if (reviews.some((review) => review.state.type === "running")) return "running…";
  const count = counts(reviews);
  const parts = [
    ...(count.open > 0 ? [`${count.open} open`] : []),
    ...(count.unverified > 0 ? [`${count.unverified} unverified`] : []),
    ...(count.unrecorded > 0 ? [`${count.unrecorded} new`] : []),
  ];
  if (parts.length > 0) return parts.join(" · ");
  if (settled(count).length > 0) return settled(count).join(" · ");
  if (reviews.every((review) => review.state.type === "failed")) return "couldn’t run";
  return "clean";
}

/**
 * The reviews' outcome in a few words: "Review running…", then "Review: clean" or
 * "Review: 2 open", with the findings the thread hasn't recorded yet as "new". Never a total
 * of what the reviews found. Null when nothing was reviewed.
 */
export function reviewStatus(reviews: readonly ReviewRun[]): string | null {
  const said = outcome(reviews);
  if (said === null) return null;
  if (said === "running…") return "Review running…";
  if (said === "couldn’t run") return "Review couldn’t run";
  return `Review: ${said}`;
}

/**
 * Where a merge landed (#224): "Merged" with no upstream to the base, "Merged on this Mac" when
 * it wasn't pushed to one, and "Merged on this Mac · origin/main moved on" when the upstream had
 * commits the merged base lacked.
 */
export function mergedWhere(upstream: MergedUpstream | undefined): string {
  if (upstream === undefined) return "Merged";
  if (upstream.behind > 0) return `Merged on this Mac · ${upstream.name} moved on`;
  return "Merged on this Mac";
}

/**
 * The merged work's line: "Merged · review clean", or "Merged · 2 open (not fixed)" for what the
 * thread left open, "Merged · 1 fixed, unverified" for a fix it never checked in the merged
 * code and "Merged · 2 fixed" for the fixes it checked; a review that ends after the merge says
 * so too. "Merged" says where the merge landed (`mergedWhere`).
 */
export function mergedStatus(reviews: readonly ReviewRun[], upstream?: MergedUpstream): string | null {
  if (reviews.length === 0) return null;
  const merged = mergedWhere(upstream);
  if (reviews.some((review) => review.state.type === "running")) return `${merged} · review running…`;
  const count = counts(reviews);
  const parts = [
    ...(count.open > 0 ? [`${count.open} open (not fixed)`] : []),
    ...(count.unverified > 0 ? [`${count.unverified} fixed, unverified`] : []),
    ...(count.unrecorded > 0 ? [`${count.unrecorded} new`] : []),
  ];
  if (parts.length > 0) return `${merged} · ${parts.join(" · ")}`;
  if (settled(count).length > 0) return `${merged} · ${settled(count).join(" · ")}`;
  if (reviews.every((review) => review.state.type === "failed")) return `${merged} · review couldn’t run`;
  return `${merged} · review clean`;
}

/** One review line of the context card, with the findings a click lists. */
export type ReviewLine = {
  text: string;
  /** Its batch's outcome alone: "clean", "1 open", "running…". */
  outcome: string;
  findings: FindingRow[];
};

/** The context card's review lines; null where there is nothing to say. */
export type ReviewLines = {
  /** The work the last merge took: its review may still run, and its outcome stays. */
  merged: ReviewLine | null;
  /** The work since the last merge (or since the session began). */
  current: ReviewLine | null;
};

function line(batch: readonly ReviewRun[], text: string | null): ReviewLine | null {
  const said = outcome(batch);
  return text === null || said === null ? null : { text, outcome: said, findings: findingRows(batch) };
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
    .flatMap((step) => (step.kind.type === "merged" ? [{ atMs: step.atMs, upstream: step.kind.upstream }] : []))
    .toSorted((a, b) => a.atMs - b.atMs);
  const last = merges.at(-1);
  const now = batchReviews(reviews, last?.atMs ?? 0, Number.POSITIVE_INFINITY);
  const current = line(now, reviewStatus(now));
  if (last === undefined) return { merged: null, current };
  const before = batchReviews(reviews, merges.at(-2)?.atMs ?? 0, last.atMs);
  return { merged: line(before, mergedStatus(before, last.upstream)), current };
}

/** One batch's findings in the review row's list: unlabelled when the row speaks for one batch. */
export type ReviewSection = { label: string | null; outcome: string; findings: FindingRow[] };

/** The context card's one review row, and the findings its click lists. */
export type ReviewRow = { text: string; sections: ReviewSection[] };

/**
 * The context card's one review row. With work since the last merge, the row speaks for both
 * batches: "Review: clean" when both found nothing, else each batch's own outcome, labelled ("Merged:
 * 1 open · Since the merge: running…"), so a newer review never hides what the merge left open
 * and no count adds across merges (Q7). Its list keeps the batches apart the same way.
 */
export function reviewRow({ merged, current }: ReviewLines): ReviewRow | null {
  const only = merged ?? current;
  if (only === null) return null;
  if (merged === null || current === null) {
    return { text: only.text, sections: [{ label: null, outcome: only.outcome, findings: only.findings }] };
  }
  const clean = merged.outcome === "clean" && current.outcome === "clean";
  return {
    text: clean ? "Review: clean" : `Merged: ${merged.outcome} · Since the merge: ${current.outcome}`,
    sections: [
      { label: "Merged work", outcome: merged.outcome, findings: merged.findings },
      { label: "Since the merge", outcome: current.outcome, findings: current.findings },
    ],
  };
}

/** The open conversation's review row, live. */
export function useReviewRow(conversationId: string): ReviewRow | null {
  const json = useBoard((s) => {
    const board = s.board;
    if (!board || board.conversationId !== conversationId) return null;
    return JSON.stringify(reviewRow(reviewLines(board.orchestratorSteps, Object.values(board.reviews))));
  });
  return useMemo(() => (json ? (JSON.parse(json) as ReviewRow | null) : null), [json]);
}

/** The context card's line for the session's full checks on its landed work: running, passed or failed. */
export type ChecksLine = {
  text: string;
  /** The line's hover text: what ran, or why it failed. */
  title: string;
  failed: boolean;
  /** While they run: since when, said as the time they have gone. */
  sinceMs?: number;
};

/** A check's number, from its id `check-<n>`. */
const checkNumber = (check: IntegratedCheck) => Number(check.id.replace(/^check-/, "")) || 0;

/**
 * Where the session's newest run of its full checks stands (SESSION-FIXES-PLAN.md §11): they run
 * once on everyone's landed work, in the background. Null before the first landing.
 */
export function checksLine(checks: readonly IntegratedCheck[]): ChecksLine | null {
  const latest = checks.reduce<IntegratedCheck | null>(
    (newest, check) => (newest && checkNumber(newest) >= checkNumber(check) ? newest : check),
    null,
  );
  if (!latest) return null;
  const at = latest.tip.slice(0, 10);
  switch (latest.state.type) {
    case "due":
      return { text: "Checks running…", title: `Due at ${at}`, failed: false, sinceMs: latest.dueAtMs };
    case "running":
      return { text: "Checks running…", title: latest.state.command ?? `At ${at}`, failed: false, sinceMs: latest.startedAtMs ?? latest.dueAtMs };
    case "passed":
      return { text: "Checks passed", title: `${latest.state.commands.join(", ") || "Nothing to run"} at ${at}`, failed: false };
    case "failed":
      return { text: `Checks failed: ${latest.state.command}`, title: `${latest.state.status} at ${at}`, failed: true };
    case "interrupted":
      return { text: "Checks couldn't run", title: latest.state.reason, failed: true };
  }
}

/** The checks line of the board's session. */
export function useChecksLine(conversationId: string): ChecksLine | null {
  const json = useBoard((s) => {
    const board = s.board;
    if (!board || board.conversationId !== conversationId) return null;
    return JSON.stringify(checksLine(Object.values(board.integratedChecks)));
  });
  return useMemo(() => (json ? (JSON.parse(json) as ChecksLine | null) : null), [json]);
}
