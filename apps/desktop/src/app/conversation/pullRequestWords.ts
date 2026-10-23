import type { PullRequest, PullRequestState } from "@/ipc/generated";

/**
 * A pull request's word when only its state is known, as a session's sidebar row and tab show
 * it: a draft, merged or closed one in the card's own word; an open one, "PR open" (its checks
 * and review are the card's lookup only).
 */
export const PULL_REQUEST_STATE_WORDS: Record<PullRequestState, string> = {
  open: "PR open",
  draft: "Draft",
  merged: "Merged",
  closed: "Closed",
};

/** The grill's seven words and their colour families. */
export type PullRequestLook = { word: string; tone: "good" | "failed" | "yellow" | "purple" | "quiet" };

/** Terminal states win, then draft, checks, and readiness; every other open PR needs review. */
export function pullRequestLook(pr: PullRequest): PullRequestLook {
  if (pr.state === "merged") return { word: PULL_REQUEST_STATE_WORDS.merged, tone: "purple" };
  if (pr.state === "closed") return { word: PULL_REQUEST_STATE_WORDS.closed, tone: "quiet" };
  if (pr.state === "draft") return { word: PULL_REQUEST_STATE_WORDS.draft, tone: "quiet" };
  if (pr.ci === "failing") return { word: "Checks failed", tone: "failed" };
  if (pr.ci === "pending") return { word: "Checks running", tone: "yellow" };
  if (pr.approved && pr.ci === "passing" && pr.mergeable && !pr.conflicts && !pr.changesRequested) return { word: "Ready", tone: "good" };
  return { word: "Review needed", tone: "yellow" };
}

/** Each tone's colour, by the app's own tokens. */
export const PULL_REQUEST_TONES: Record<PullRequestLook["tone"], string> = {
  good: "text-success",
  failed: "text-destructive",
  yellow: "text-pr-yellow",
  purple: "text-pr-purple",
  quiet: "text-muted-foreground",
};
