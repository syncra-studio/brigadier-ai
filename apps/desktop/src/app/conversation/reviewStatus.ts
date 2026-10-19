import type { Finding, ReviewRun } from "@/ipc/generated";

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
