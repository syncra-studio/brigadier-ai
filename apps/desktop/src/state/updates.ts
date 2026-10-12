import { create } from "zustand";

import { openUrl, request } from "@/ipc/client";
import type { EventEnvelope, UpdateItem, UpdateTarget, UpdatesView } from "@/ipc/generated";

/**
 * Newer versions of Brigadier and the agent CLIs: the daemon checks and runs the updates,
 * publishing each change as `updatesChanged`; this holds the last view, which versions the user
 * skipped (kept across launches), the updates that finished and have left (until the app quits),
 * and why starting an update failed.
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
  /** Updates that finished and, after a moment showing so, left the list. */
  seen: string[];
  /** Why asking for an update failed, by what it was for, until it is asked again. */
  errors: Partial<Record<UpdateTarget, string>>;
}>(() => ({ view: null, skipped: readSkipped(), seen: [], errors: {} }));

/** How long a finished update shows its check before it leaves on its own. */
export const UPDATED_LINGER_MS = 1000;

/** The rows to show: not skipped, and not an update that finished and has left. */
export function shownUpdates(
  view: UpdatesView | null,
  skipped: readonly string[],
  seen: readonly string[],
): UpdateItem[] {
  return (view?.items ?? []).filter(
    (item) => !skipped.includes(keyOf(item)) && !seen.includes(keyOf(item)),
  );
}

/**
 * What a click on the updates pill does: when all there is is one agent's update that Brigadier
 * runs itself, it starts at once; anything else (several, Brigadier's own download, a command
 * to copy, a failure to read) opens the list.
 */
export function pillClick(items: readonly UpdateItem[]): UpdateItem | "menu" {
  const only = items.length === 1 ? items[0] : undefined;
  return only &&
    only.target !== "app" &&
    only.action.type === "run" &&
    only.progress.type === "available"
    ? only
    : "menu";
}

/** The updates in `view` that have finished but not yet left. */
export function finishedKeys(view: UpdatesView | null, seen: readonly string[]): string[] {
  return (view?.items ?? [])
    .filter((item) => item.progress.type === "updated")
    .map(keyOf)
    .filter((key) => !seen.includes(key));
}

const leaving = new Set<string>();

/** Takes in the daemon's view; an update that finished leaves the list a moment later. */
function setView(view: UpdatesView | null): void {
  useUpdates.setState({ view });
  for (const key of finishedKeys(view, useUpdates.getState().seen)) {
    if (leaving.has(key)) continue;
    leaving.add(key);
    setTimeout(() => {
      leaving.delete(key);
      const { seen } = useUpdates.getState();
      if (!seen.includes(key)) useUpdates.setState({ seen: [...seen, key] });
    }, UPDATED_LINGER_MS);
  }
}

/** Reads the daemon's view: on (re)connecting, after events may have been missed. */
export async function loadUpdates(): Promise<void> {
  const { updates } = await request({ method: "getUpdates" });
  setView(updates);
}

export function applyUpdateEvents(batch: readonly EventEnvelope[]): void {
  for (const { event } of batch) {
    if (event.type === "updatesChanged") setView(event.updates);
  }
}

/** Takes the update, keeping why asking failed for its row; false when it did. */
export async function startUpdate(item: UpdateItem): Promise<boolean> {
  useUpdates.setState(({ errors }) => {
    const rest = { ...errors };
    delete rest[item.target];
    return { errors: rest };
  });
  try {
    await takeUpdate(item);
    return true;
  } catch (cause) {
    const error = cause instanceof Error ? cause.message : String(cause);
    useUpdates.setState(({ errors }) => ({ errors: { ...errors, [item.target]: error } }));
    return false;
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
