// The nightly update through electron-updater, in the Tauri build's order (src-tauri/src/updates.rs). A build
// checks only if electron-builder wrote it an app-update.yml, which only a build given a feed gets.
//
// macOS: the update is Squirrel's, which requires the running app and the update to carry the same code
// signature; it can only be proved on a signed build.
import fs from "node:fs";
import path from "node:path";

import { app } from "electron";

import { addCommands } from "./commands";
import { dev } from "./identity";
import { setBeforeExit, quit } from "./quit";
import { UpdateStages, unlessStalled, updateChannel, updatesEnabled, type Download, type Shown } from "./updateStages";
import { mainWindow } from "./windows";

const FIRST_CHECK_MS = 60_000;
const CHECK_EVERY_MS = 4 * 60 * 60_000;
const DOWNLOAD_STALL_MS = 60_000;
const FEED_LIMIT_MS = 30_000;
const EVENT = "app-update";

const stages = new UpdateStages();

/** Whether this build carries a feed. */
export function updateConfigured(): boolean {
  // A check package may switch it on, and only "Check now" looks: no timers in a dev run (updatesEnabled).
  if (dev && process.env.BRIGADIER_DEV_UPDATES !== "1") return false;
  return fs.existsSync(path.join(process.resourcesPath, "app-update.yml"));
}

/** The feed in words, for Settings › About. */
export const channelOfThisBuild = (): string | null => updateChannel(app.getVersion(), updateConfigured());

function emit(shown: Shown): void {
  mainWindow()?.webContents.send("brigadier:event", EVENT, shown);
}

// Loaded only when used: its import costs start-up time, and most launches have nothing to do with it.
const updater = async () => {
  const { autoUpdater } = (await import("electron-updater")).default;
  autoUpdater.autoDownload = false;
  autoUpdater.autoInstallOnAppQuit = false;
  return autoUpdater;
};

function within<T>(limitMs: number, work: Promise<T>): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("the update feed did not answer")), limitMs);
    work.then(resolve, reject).finally(() => clearTimeout(timer));
  });
}

async function check(): Promise<void> {
  const autoUpdater = await updater();
  const found = await within(FEED_LIMIT_MS, autoUpdater.checkForUpdates());
  if (!found?.isUpdateAvailable) return;
  const download: Download = async () => {
    // Read again: a nightly published since the offer deletes the offered build's archive.
    await within(FEED_LIMIT_MS, autoUpdater.checkForUpdates()).catch((error: unknown) => {
      console.info("the offered update stands; the feed could not be read again:", error);
    });
    const { CancellationToken } = (await import("electron-updater")).default;
    const cancel = new CancellationToken();
    await unlessStalled(
      DOWNLOAD_STALL_MS,
      autoUpdater.downloadUpdate(cancel),
      (tick) => {
        autoUpdater.on("download-progress", tick);
        return () => autoUpdater.off("download-progress", tick);
      },
      () => cancel.cancel(),
    );
    return (reopen) => autoUpdater.quitAndInstall(true, reopen);
  };
  const shown = stages.offer(found.updateInfo.version, download);
  if (shown) emit(shown);
}

const attempt = () => {
  check().catch((error: unknown) => console.info("no nightly update this time:", error));
};

export function installUpdates(smoke: boolean): void {
  const enabled = updatesEnabled(dev, updateConfigured(), smoke);
  addCommands({
    app_update: () => stages.shown(),
    check_for_updates: async () => {
      if (!updateConfigured()) throw new Error("this build has no update feed");
      await check();
      return stages.shown();
    },
    take_app_update: () => {
      void stages.take(emit);
    },
    restart_to_update: () => {
      stages.restart = true;
      quit(mainWindow);
    },
  });
  // Installing hands the quit over to the updater, which ends the process itself; else it may reopen the app.
  setBeforeExit((daemonStopped) => {
    const { reopen, installing } = stages.finish(daemonStopped);
    if (installing) return true;
    if (reopen) app.relaunch();
    return false;
  });
  if (!enabled) return;
  setTimeout(() => {
    attempt();
    setInterval(attempt, CHECK_EVERY_MS);
  }, FIRST_CHECK_MS);
}
