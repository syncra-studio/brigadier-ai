import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";

import { type AppUpdate, appUpdate, restartToUpdate, takeAppUpdate } from "@/ipc/client";

export type { AppUpdate };

/*
 * A nightly build's own update: the shell finds and offers it, downloads it only once the user
 * clicks it, then installs it when the app quits.
 */

export const useAppUpdate = create<{ update: AppUpdate | null }>(() => ({ update: null }));

/** The shell's event, with the update as it now stands, each time it changes. */
export const APP_UPDATE_EVENT = "app-update";

/** Follows the shell's update from now on, starting with one it already has. */
export async function startAppUpdate(): Promise<void> {
  // An event that comes while the shell is asked is newer than its answer.
  let heard = false;
  await listen<AppUpdate>(APP_UPDATE_EVENT, (event) => {
    heard = true;
    useAppUpdate.setState({ update: event.payload });
  });
  const update = await appUpdate();
  if (update && !heard) useAppUpdate.setState({ update });
}

/**
 * The pill's click: downloads an offered update (again, after a failure), or restarts into a
 * downloaded one. The pill turns busy at once, before the shell's word that it has.
 */
export function clickAppUpdate(): void {
  const { update } = useAppUpdate.getState();
  if (!update || update.state === "updating") return;
  if (update.state === "ready") {
    void restartToUpdate();
    return;
  }
  useAppUpdate.setState({ update: { ...update, state: "updating" } });
  takeAppUpdate().catch(() => {
    useAppUpdate.setState({ update: { ...update, state: "failed" } });
  });
}
