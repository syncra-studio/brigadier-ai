// Files dropped on the window and desktop notifications (src-tauri's drops.rs and notifications.rs).
//
// The page sees the drag here, but only the preload's world, out of the page's reach, tells main what the OS
// dropped: a page can't name a file into the list `read_dropped` is limited to.
import fs from "node:fs";

import { Notification, shell } from "electron";

import { onFromApp } from "./appPage";
import { addCommands } from "./commands";
import { droppedFile } from "./dropped";
import { note } from "./events";
import { identifier } from "./identity";
import { native } from "./native";
import { showMain, userCameToIt } from "./windows";

let lastDrop: { paths: string[]; shift: boolean } | null = null;

const isFolder = (path: string) => {
  try {
    return fs.statSync(path).isDirectory();
  } catch {
    return false;
  }
};

const ASK_WAIT_MS = 60_000;
const ASK_POLL_MS = 500;

/** Asks once, then waits for the answer, up to a minute; elsewhere there is nothing to ask. */
async function askAndWait(): Promise<string> {
  native().noticeAsk();
  const deadline = Date.now() + ASK_WAIT_MS;
  for (;;) {
    const now = await native().noticePermission();
    if (now !== "notAsked" || Date.now() >= deadline) return now;
    await new Promise((resolve) => setTimeout(resolve, ASK_POLL_MS));
  }
}

/** Kept until they're done: a collected notification no longer hears its click. */
const showing = new Set<Notification>();

/** A desktop notification, as the app itself. Best effort. Clicked, the app comes back as from its Dock icon. */
export function notify(title: string, body: string): void {
  const notification = new Notification({ title, body, silent: true });
  showing.add(notification);
  notification.on("show", () => note("notification-shown", { title }));
  notification.on("click", () => {
    showing.delete(notification);
    note("notification-clicked", { title });
    userCameToIt();
    showMain();
  });
  notification.on("close", () => showing.delete(notification));
  notification.on("failed", (_event, error) => {
    showing.delete(notification);
    note("notification-failed", { title, error });
  });
  notification.show();
}

export function installDrops(): void {
  // Before the page's own drop handler asks for `last_drop`: the preload's listener sends first, on one pipe.
  onFromApp("brigadier:dropped", (_event, paths: string[], shift: boolean) => {
    // The addon reads the key on macOS only; elsewhere the drop's own event says.
    lastDrop = { paths, shift: shift || native().shiftHeld() };
  });
  addCommands({
    drag_shift: () => native().shiftHeld(),
    last_drop: () =>
      lastDrop && {
        shift: lastDrop.shift,
        entries: lastDrop.paths.map((path) => ({ path, folder: isFolder(path) })),
      },
    read_dropped: ({ path }) => droppedFile(lastDrop?.paths ?? [], path as string),
    notification_permission: () => native().noticePermission(),
    ask_notification_permission: () => askAndWait(),
    open_notification_settings: () => {
      if (process.platform !== "darwin") throw new Error("Open your system's notification settings to turn them on for Brigadier.");
      return shell.openExternal(`x-apple.systempreferences:com.apple.Notifications-Settings.extension?id=${identifier}`);
    },
  });
}
