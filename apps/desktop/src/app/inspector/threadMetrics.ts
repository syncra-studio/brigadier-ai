import type { CacheCauses, ProviderTokens, RequestContext, RequestSummary, ThreadEdits } from "@/ipc/generated";
import { formatDuration, formatTokens } from "@/lib/format";

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
      // One report can cover several calls (a Claude turn): its context is read once.
      span:
        request.calls === 1 || request.firstTokens === request.lastTokens
          ? formatTokens(request.firstTokens)
          : `${formatTokens(request.firstTokens)} → ${formatTokens(request.lastTokens)}`,
      growth: request.calls === 1 ? "" : formatGrowth(request.growthTokens),
    }));
}

/** A time after the request was sent: tenths of a second under a minute, else "6m 40s". */
export function formatSince(ms: number): string {
  return ms < 60_000 ? `${(ms / 1000).toFixed(1)} s` : formatDuration(ms);
}

function providerName(provider: ProviderTokens["provider"]): string {
  return provider.charAt(0).toUpperCase() + provider.slice(1);
}

export type SummaryRow = {
  key: string;
  /** The start of the user's message. */
  label: string;
  /** "first 1.2 s · answer 6m 40s · landed 6m 2s · settled 7m 1s"; "working" while it works. */
  times: string;
  /** Per provider: "Claude 1.3M raw, 210K without cache reads, $0.75". */
  providers: string[];
  /** "thread 1.3K · worker 2.2K (3 calls)". */
  steps: string;
  /** Where its cache writes came from: "cache writes 217K (1h 217K) · note fork wrote 217K, missed ≈252K"; empty when it wrote and missed nothing. */
  cache: string;
};

/** A request's cache writes, by lifetime and by the known cause of each session start, with the estimated missed prefix (BUILD-PLAN B0). Missed figures are estimates, marked ≈. */
export function cacheLine(providers: readonly ProviderTokens[], cache: CacheCauses): string {
  const written = providers.reduce((sum, tokens) => sum + tokens.cacheWrite, 0);
  const hour = providers.reduce((sum, tokens) => sum + tokens.cacheWrite1h, 0);
  const five = providers.reduce((sum, tokens) => sum + tokens.cacheWrite5m, 0);
  if (written === 0 && cache.missed === 0) return "";
  const lifetimes = [hour > 0 && `1h ${formatTokens(hour)}`, five > 0 && `5m ${formatTokens(five)}`].filter(Boolean);
  const parts = [`cache writes ${formatTokens(written)}${lifetimes.length > 0 ? ` (${lifetimes.join(", ")})` : ""}`];
  for (const cause of cache.causes) {
    const times = cause.sessions > 1 ? ` ×${cause.sessions}` : "";
    const missed = cause.missed > 0 ? `, missed ≈${formatTokens(cause.missed)}` : "";
    parts.push(`${cause.cause}${times} wrote ${formatTokens(cause.written)}${missed}`);
  }
  if (cache.unexplained > 0) parts.push(`≈${formatTokens(cache.unexplained)} missed, no known cause`);
  if (cache.bigMisses > 0) parts.push(`${cache.bigMisses} ${cache.bigMisses === 1 ? "call" : "calls"} missed over 20K`);
  return parts.join(" · ");
}

/** Each request's time and tokens, newest first, at most `limit` of them. */
export function summaryRows(summaries: readonly RequestSummary[], limit: number): SummaryRow[] {
  return summaries
    .slice(-limit)
    .toReversed()
    .map((summary) => {
      const times = [
        summary.firstEventMs !== null && `first ${formatSince(summary.firstEventMs)}`,
        // A stop, a failure or a quit mid-turn never reads as an answer (UI audit H1).
        summary.answerMs !== null ? `${summary.ended ?? "answer"} ${formatSince(summary.answerMs)}` : "working",
        summary.landedMs !== null && `landed ${formatSince(summary.landedMs)}`,
        summary.settledMs !== null && `settled ${formatSince(summary.settledMs)}`,
      ].filter(Boolean);
      return {
        key: summary.requestId,
        label: summary.preview.trim() || "An earlier request",
        times: times.join(" · "),
        providers: summary.providers.map((tokens) => {
          const cost = tokens.costUsd !== null ? `, $${tokens.costUsd.toFixed(2)}` : "";
          // Codex's cached share of its input, which its comparisons rely on (BUILD-PLAN B0).
          const input = tokens.input + tokens.cachedInput;
          const share =
            tokens.provider === "codex" && input > 0 ? `, ${Math.round((tokens.cachedInput / input) * 100)}% of input cached` : "";
          return `${providerName(tokens.provider)} ${formatTokens(tokens.raw)} raw, ${formatTokens(tokens.rawWithoutCacheReads)} without cache reads${cost}${share}`;
        }),
        steps: summary.steps
          .map((step) => {
            // Its turns where they are counted, and the model calls behind them.
            const counts = [
              step.turns > 0 && `${step.turns} ${step.turns === 1 ? "turn" : "turns"}`,
              (step.turns > 0 || step.calls !== 1) && `${step.calls} ${step.calls === 1 ? "call" : "calls"}`,
            ].filter(Boolean);
            return `${step.step} ${formatTokens(step.raw)}${counts.length > 0 ? ` (${counts.join(", ")})` : ""}`;
          })
          .join(" · "),
        cache: cacheLine(summary.providers, summary.cache),
      };
    });
}
