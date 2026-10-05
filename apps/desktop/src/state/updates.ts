import { create } from "zustand";

import { openUrl, request } from "@/ipc/client";
import type { EventEnvelope, UpdateItem, UpdatesView } from "@/ipc/generated";

/**
 * Newer versions of Brigadier and the agent CLIs: the daemon checks and runs the updates,
 * publishing each change as `updatesChanged`; this holds the last view, and which versions the
 * user skipped (kept across launches) or has seen updated (until the app quits).
 */

const SKIPPED_KEY = "brigadier.skippedUpdates";

/** One version of one target: what skipping and having seen an update are about. */
const keyOf = (item: UpdateItem) => `${item.target}@${item.latest}`;

function readSkipped(): string[] {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(SKIPPED_KEY) ?? "[]");
    return Array.isArray(parsed) ? parsed.filter((key) => typeof key === "string") : [];
  } catch {
    return [];
  }
}

export const useUpdates = create<{
  view: UpdatesView | null;
  skipped: string[];
  /** Updated rows the user has seen; they leave the list once its panel closes. */
  seen: string[];
}>(() => ({ view: null, skipped: readSkipped(), seen: [] }));

/** The rows to show: not skipped, and not an update the user already saw finish. */
export function shownUpdates(
  view: UpdatesView | null,
  skipped: readonly string[],
  seen: readonly string[],
): UpdateItem[] {
  return (view?.items ?? []).filter(
    (item) => !skipped.includes(keyOf(item)) && !seen.includes(keyOf(item)),
  );
}

/** Reads the daemon's view: on (re)connecting, after events may have been missed. */
export async function loadUpdates(): Promise<void> {
  const { updates } = await request({ method: "getUpdates" });
  useUpdates.setState({ view: updates });
}

export function applyUpdateEvents(batch: readonly EventEnvelope[]): void {
  for (const { event } of batch) {
    if (event.type === "updatesChanged") useUpdates.setState({ view: event.updates });
  }
}

/** Takes the update: opens Brigadier's download page, or asks the daemon to update the CLI. */
export async function takeUpdate(item: UpdateItem): Promise<void> {
  if (item.action.type === "download") {
    await openUrl(item.action.url);
  } else if (item.action.type === "run") {
    await request({ method: "runUpdate", target: item.target });
  }
}

/** Hides this version until a newer one comes out. */
export function skipUpdate(item: UpdateItem): void {
  const skipped = [...new Set([...useUpdates.getState().skipped, keyOf(item)])];
  useUpdates.setState({ skipped });
  try {
    localStorage.setItem(SKIPPED_KEY, JSON.stringify(skipped));
  } catch {
    // Storage can be unavailable; the skip then lasts until the app quits.
  }
}

/** The panel closed: rows that finished updating have been seen. */
export function noteUpdatesSeen(): void {
  const { view, seen } = useUpdates.getState();
  const done = (view?.items ?? []).filter((item) => item.progress.type === "updated").map(keyOf);
  if (done.some((key) => !seen.includes(key))) {
    useUpdates.setState({ seen: [...new Set([...seen, ...done])] });
  }
}
