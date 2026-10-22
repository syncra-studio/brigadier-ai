import type { PullRequest } from "@/ipc/generated";

/** How the pull request row reads: a state word, and the colour family its icon and word take. */
export type PullRequestLook = { word: string; tone: "good" | "failed" | "warning" | "quiet" };

/**
 * The pull request's state in a word or two, from `gh` (S14): failed checks first (red), then
 * what keeps an open one from merging (conflicts, changes asked for), its checks still running,
 * else its state. It has no actions.
 */
export function pullRequestLook(pr: PullRequest): PullRequestLook {
  if (pr.state === "merged") return { word: "Merged", tone: "quiet" };
  if (pr.state === "closed") return { word: "Closed", tone: "quiet" };
  if (pr.ci === "failing") return { word: "Checks failed", tone: "failed" };
  if (pr.conflicts) return { word: "Conflicts", tone: "warning" };
  if (pr.changesRequested) return { word: "Changes requested", tone: "warning" };
  if (pr.state === "draft") return { word: "Draft", tone: "quiet" };
  if (pr.ci === "pending") return { word: "Checks running", tone: "warning" };
  return { word: "Open", tone: "good" };
}

/** Each tone's colour, by the app's own tokens. */
export const PULL_REQUEST_TONES: Record<PullRequestLook["tone"], string> = {
  good: "text-success",
  failed: "text-destructive",
  warning: "text-warning",
  quiet: "text-muted-foreground",
};
