import type { PullRequest } from "@/ipc/generated";

/** The grill's seven words and their colour families. */
export type PullRequestLook = { word: string; tone: "good" | "failed" | "yellow" | "purple" | "quiet" };

/** Terminal states win, then draft, checks, and readiness; every other open PR needs review. */
export function pullRequestLook(pr: PullRequest): PullRequestLook {
  if (pr.state === "merged") return { word: "Merged", tone: "purple" };
  if (pr.state === "closed") return { word: "Closed", tone: "quiet" };
  if (pr.state === "draft") return { word: "Draft", tone: "quiet" };
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
