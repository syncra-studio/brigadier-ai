import type { RankingsRefresh, RankingsRefreshState } from "@/ipc/generated";
import { formatDateTime } from "@/lib/format";

/*
 * The words of a rankings refresh: whether it is still running (and so polled), and how the
 * last one ended.
 */

/** A running refresh is asked about this often while Settings › Providers shows it. */
export const RANKINGS_POLL_MS = 2_000;

/** Whether a refresh in this state is still running: the page polls it and keeps the button off. */
export function refreshRunning(
  state: RankingsRefreshState,
): state is "checkingRegistry" | "researching" {
  return state === "checkingRegistry" || state === "researching";
}

/** When the last refresh ran, with which model and how it ended; `null` when none ran. */
export function lastRefresh(refresh: RankingsRefresh, researcher: string | null): string | null {
  if (refresh.state === "idle" || refreshRunning(refresh.state)) return null;
  const at = refresh.finishedAtMs ?? refresh.startedAtMs;
  const parts = [
    at !== null ? formatDateTime(at) : null,
    researcher !== null ? `with ${researcher}` : null,
  ].filter(Boolean);
  const ended = OUTCOME[refresh.state];
  return parts.length > 0 ? `${parts.join(" ")}: ${ended}` : ended;
}

const OUTCOME: Record<"done" | "failed" | "cancelled" | "superseded", string> = {
  done: "finished",
  failed: "failed",
  cancelled: "stopped",
  superseded: "replaced by a newer published model list",
};
