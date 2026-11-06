// What counts as the app's own page, the one thing that gets the shell's IPC. A guest has no preload to reach
// it with; these checks hold if that ever stops being so, or if a window of ours is sent somewhere else.
import { app, ipcMain, type IpcMainEvent, type IpcMainInvokeEvent, type WebContents } from "electron";

import { note } from "./events";
import { dev, rendererUrl } from "./identity";

const page = new URL(rendererUrl);

/** By scheme and host, since Node gives `app:` an origin of "null". */
export function isAppPage(url: string): boolean {
  try {
    const parsed = new URL(url);
    return parsed.protocol === page.protocol && parsed.host === page.host;
  } catch {
    return false;
  }
}

/** The top frame of one of the app's windows, showing the app's page. A popup is a window too, so the address counts. */
export function fromAppPage(event: IpcMainEvent | IpcMainInvokeEvent): boolean {
  const frame = event.senderFrame;
  return event.sender.getType() === "window" && frame !== null && frame.parent === null && isAppPage(frame.url);
}

const refused = (channel: string, event: IpcMainEvent | IpcMainInvokeEvent) =>
  note("ipc-refused", { channel, webContents: event.sender.id, url: event.senderFrame?.url ?? null });

export function handleFromApp(channel: string, handler: (event: IpcMainInvokeEvent, ...args: any[]) => unknown): void {
  ipcMain.handle(channel, (event, ...args) => {
    if (fromAppPage(event)) return handler(event, ...args);
    refused(channel, event);
    throw new Error(`${channel} is for the app's own page`);
  });
}

export function onFromApp(channel: string, listener: (event: IpcMainEvent, ...args: any[]) => void): void {
  ipcMain.on(channel, (event, ...args) => {
    if (fromAppPage(event)) listener(event, ...args);
    else refused(channel, event);
  });
}

/** A link or a dropped file must not carry the app's window, with its preload, off to another address. */
export function keepOnAppPage(contents: WebContents): void {
  const guard = (details: Electron.Event<{ url: string }>) => {
    if (isAppPage(details.url)) return;
    note("app-navigation-refused", { webContents: contents.id, url: details.url });
    details.preventDefault();
  };
  contents.on("will-navigate", guard);
  contents.on("will-redirect", (details) => {
    if (details.isMainFrame) guard(details);
  });
}

// `--inspect` and its kind are shut by a fuse (builder.config.cjs); these have none. A dev run needs the pipe (tools/browser-check).
const REFUSED_SWITCHES = ["remote-debugging-port", "remote-debugging-pipe", "no-sandbox", "disable-web-security"];

/** A packaged app won't run with a debug port or with a page's protections off, so a shell can't undo them on the user's signed-in pages. */
export function refuseUnsafeSwitches(): void {
  if (dev) return;
  const given = REFUSED_SWITCHES.find((name) => app.commandLine.hasSwitch(name));
  if (!given) return;
  console.error(`brigadier: refusing to start with --${given}`);
  app.exit(1);
}
