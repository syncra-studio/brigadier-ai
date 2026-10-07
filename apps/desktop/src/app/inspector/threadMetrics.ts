import type { RequestContext, ThreadEdits } from "@/ipc/generated";
import { formatTokens } from "@/lib/format";

/** The thread's own commits in a line: how many, and the lines they changed. */
export function editsLine(edits: ThreadEdits | null): string {
  if (edits === null) return "Its branch hasn't been looked at yet.";
  const kept = edits.kept ? " (branch merged; last count)" : "";
  if (edits.commits === 0) return `No commits of its own on ${edits.branch}${kept}.`;
  const commits = edits.commits === 1 ? "1 commit" : `${edits.commits} commits`;
  return `${commits} on ${edits.branch}: +${edits.added.toLocaleString()} −${edits.removed.toLocaleString()} lines${kept}`;
}

/** A change in tokens with its sign: "+5.7K", "−1.2K", "no change". */
export function formatGrowth(tokens: number): string {
  if (tokens === 0) return "no change";
  return `${tokens > 0 ? "+" : "−"}${formatTokens(Math.abs(tokens))}`;
}

export type GrowthRow = {
  key: string;
  /** The start of the user's message. */
  label: string;
  calls: string;
  /** The context at its first and last call: "12.4K → 18.1K". */
  span: string;
  growth: string;
};

/** The requests' context growth, newest first, at most `limit` of them. */
export function growthRows(requests: readonly RequestContext[], limit: number): GrowthRow[] {
  return requests
    .slice(-limit)
    .toReversed()
    .map((request) => ({
      key: request.requestId,
      label: request.preview.trim() || "An earlier request",
      calls: request.calls === 1 ? "1 call" : `${request.calls} calls`,
      span:
        request.calls === 1
          ? formatTokens(request.firstTokens)
          : `${formatTokens(request.firstTokens)} → ${formatTokens(request.lastTokens)}`,
      growth: request.calls === 1 ? "" : formatGrowth(request.growthTokens),
    }));
}
