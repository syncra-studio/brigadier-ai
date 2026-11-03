import { request } from "@/ipc/client";
import type { EventEnvelope, RankingsRefresh } from "@/ipc/generated";
import { openSettings } from "@/state/actions";
import { loadModelRules, useModelRules } from "@/state/modelRules";
import { toast } from "@/state/toasts";

/** The research job last told of in a toast, so each one is told of once, across restarts. */
const TOLD_KEY = "brigadier.ratingsResearch.told";

/** The toast's words: "Ratings research found a better-rated model for Computer use". */
export function newsText(found: readonly string[]): string {
  const works = found.length <= 2 ? found.join(" and ") : `${found.slice(0, -1).join(", ")} and ${found.at(-1)}`;
  return `Ratings research found a better-rated model for ${works}`;
}

/** Whether a research's result is news: it finished, found something for an order of yours, and wasn't told of yet. */
export function isNews(refresh: RankingsRefresh, toldOf: string | null): boolean {
  return refresh.state === "done" && refresh.jobId !== null && refresh.jobId !== toldOf && refresh.found.length > 0;
}

function lastTold(): string | null {
  try {
    return localStorage.getItem(TOLD_KEY);
  } catch {
    return null;
  }
}

/** One toast with View (Settings › Routing) after a research that found something new. */
export function noteResearch(refresh: RankingsRefresh): void {
  if (!isNews(refresh, lastTold())) return;
  try {
    localStorage.setItem(TOLD_KEY, refresh.jobId ?? "");
  } catch {
    // Without storage it may be told again after a restart.
  }
  toast(newsText(refresh.found), { tone: "neutral", actions: [{ label: "View", run: () => openSettings("routing") }] });
}

/** Reads the latest research and tells of what it found, once. */
export async function checkResearchNews(): Promise<void> {
  const { refresh } = await request({ method: "getRankingsRefresh" });
  noteResearch(refresh);
}

/** The ratings changed: tell of a research's news, and read Routing's view again if it is loaded. */
export function applyRatingsEvents(batch: readonly EventEnvelope[]): void {
  if (!batch.some(({ event }) => event.type === "rankingsChanged")) return;
  void checkResearchNews().catch(() => {});
  if (useModelRules.getState().view !== null) void loadModelRules();
}
