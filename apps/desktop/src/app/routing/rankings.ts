import type { RankingsRefresh, RankingsRefreshState, RatingChange } from "@/ipc/generated";
import { formatDateTime, formatTime } from "@/lib/format";

/*
 * The words of a rankings refresh: whether it is still running (and so polled), what it is
 * doing or how it ended, and whether its researched ratings are in use.
 */

/** A running refresh is asked about this often while the Routing page shows it. */
export const RANKINGS_POLL_MS = 2_000;

/** Whether a refresh in this state is still running: the page polls it and keeps the button off. */
export function refreshRunning(state: RankingsRefreshState): boolean {
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

/** What the refresh is doing, or how it ended; `null` when none ran. */
export function refreshStatus(refresh: RankingsRefresh): RefreshStatus | null {
  switch (refresh.state) {
    case "idle":
      return null;
    case "checkingRegistry":
      return { tone: "running", text: "Checking for a newer model list…" };
    case "researching": {
      const model = refresh.model ?? "one of your models";
      const since = refresh.startedAtMs !== null ? ` since ${formatTime(refresh.startedAtMs)}` : "";
      return { tone: "running", text: `Researching with ${model}${since}…` };
    }
    case "done": {
      const count = changedModels(refresh.changes).length;
      return {
        tone: "done",
        text:
          count === 0
            ? "No ratings changed."
            : `Updated ${count} ${count === 1 ? "model" : "models"}.`,
      };
    }
    case "failed":
      return { tone: "error", text: "The refresh failed. The ratings in use didn't change." };
    case "cancelled":
      return { tone: "warning", text: "The refresh was cancelled." };
    case "superseded":
      return {
        tone: "warning",
        text: "A newer curated model list was installed, so the research no longer applies.",
      };
  }
}

/** Which ratings are in use: the researched ones, since when, or the curated ones. */
export function overlayStatus(refresh: RankingsRefresh): string {
  const at = refresh.overlayAtMs;
  if (refresh.overlayApplied) {
    return at !== null ? `Using researched ratings from ${formatDateTime(at)}.` : "Using researched ratings.";
  }
  if (at !== null) {
    return `Using the curated ratings. The research from ${formatDateTime(at)} is no longer applied: a newer curated model list replaced it.`;
  }
  return "Using the curated ratings.";
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
