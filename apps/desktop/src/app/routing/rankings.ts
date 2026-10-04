import type { RankingsRefresh, RankingsRefreshState, RatingChange } from "@/ipc/generated";
import { formatDateTime, formatTime } from "@/lib/format";

/*
 * The words of a rankings refresh: whether it is still running (and so polled), what it is
 * doing or how it ended, and whether its researched ratings are in use.
 */

/** A running refresh is asked about this often while the Routing page shows it. */
export const RANKINGS_POLL_MS = 2_000;

/** Whether a refresh in this state is still running: the page polls it and keeps the button off. */
export function refreshRunning(
  state: RankingsRefreshState,
): state is "checkingRegistry" | "researching" {
  return state === "checkingRegistry" || state === "researching";
}

/** The researched models whose effective rating changed; one with no fields was only confirmed. */
export function changedModels(changes: readonly RatingChange[]): RatingChange[] {
  return changes.filter((change) => change.fields.length > 0);
}

export type RefreshStatus = {
  tone: "running" | "done" | "warning" | "error";
  text: string;
};

/**
 * What the refresh is doing, or how the last one ended, in one line; `null` when none ran.
 * `researcher` names the model it researched with.
 */
export function refreshStatus(refresh: RankingsRefresh, researcher: string | null): RefreshStatus | null {
  switch (refresh.state) {
    case "idle":
      return null;
    case "checkingRegistry":
      return { tone: "running", text: "Checking for a newer published model list…" };
    case "researching": {
      const model = researcher ?? "one of your models";
      const since = refresh.startedAtMs !== null ? ` since ${formatTime(refresh.startedAtMs)}` : "";
      return { tone: "running", text: `Researching the models with ${model}${since}…` };
    }
    case "done": {
      const count = changedModels(refresh.changes).length;
      const when = refresh.finishedAtMs !== null ? ` ${formatDateTime(refresh.finishedAtMs)}` : "";
      return {
        tone: "done",
        text:
          count === 0
            ? `Rankings refreshed${when}. No ratings changed.`
            : `Rankings refreshed${when}. Updated ${count} ${count === 1 ? "model" : "models"}.`,
      };
    }
    case "failed":
      return {
        tone: "error",
        text: "The refresh failed, so the ratings didn't change. The reason is under Advanced.",
      };
    case "cancelled":
      return { tone: "warning", text: "The refresh was stopped. The ratings didn't change." };
    case "superseded":
      return {
        tone: "warning",
        text: "A newer published model list came in during the refresh, so its research isn't used.",
      };
  }
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
  done: "finished.",
  failed: "failed.",
  cancelled: "stopped.",
  superseded: "replaced by a newer published model list.",
};

/** Which ratings are in use: the researched ones, since when, or the published ones. */
export function overlayStatus(refresh: RankingsRefresh): string {
  const at = refresh.overlayAtMs;
  if (refresh.overlayApplied) {
    return at !== null ? `Researched ratings from ${formatDateTime(at)}.` : "Researched ratings.";
  }
  if (at !== null) {
    return `The published ratings. The research from ${formatDateTime(at)} isn't used: a newer published model list replaced it.`;
  }
  return "The published ratings.";
}

const EFFORT_PREFIX = "defaultEffort.";
const STRENGTH_PREFIX = "strengths.";
const AREA_PREFIX = "areaStrengths.";

/** A changed field in words: "tier", "review score", "frontend adjustment", "research effort". */
export function fieldWords(field: string): string {
  if (field.startsWith(STRENGTH_PREFIX)) return `${field.slice(STRENGTH_PREFIX.length)} score`;
  if (field.startsWith(AREA_PREFIX)) return `${field.slice(AREA_PREFIX.length)} adjustment`;
  if (field.startsWith(EFFORT_PREFIX)) return `${field.slice(EFFORT_PREFIX.length)} effort`;
  return field;
}
