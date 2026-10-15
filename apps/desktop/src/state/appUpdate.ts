import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";

import { appUpdateReady } from "@/ipc/client";

/**
 * A nightly build's own update: the shell downloads it in the background and installs it when
 * the app quits (src-tauri/src/updates.rs). This holds its version once it is waiting.
 */
export const useAppUpdate = create<{ ready: string | null }>(() => ({ ready: null }));

/** The shell's event, with the version, when an update has been downloaded. */
export const APP_UPDATE_READY_EVENT = "app-update-ready";

/** Follows the shell's update from now on, starting with one that is already waiting. */
export async function startAppUpdate(): Promise<void> {
  await listen<string>(APP_UPDATE_READY_EVENT, (event) => {
    useAppUpdate.setState({ ready: event.payload });
  });
  const ready = await appUpdateReady();
  if (ready) useAppUpdate.setState({ ready });
}
