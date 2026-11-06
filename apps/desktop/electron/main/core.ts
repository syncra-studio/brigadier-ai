// The commands the app can't start without: the daemon's bridge, the app's own facts, quitting.
import { app, BrowserWindow } from "electron";

import { addCommands, channelId, channelSender, RawJson, type Args } from "./commands";
import { identifier } from "./identity";
import { native, type Started } from "./native";
import { isQuitting, pagesFlushed, quit } from "./quit";
import { clearBackdrop, mainWindow } from "./windows";

export type AppFacts = { started: Started; smoke: boolean; budgetTolerance: number };

const openedFolders: string[] = [];

/** Folders opened with the app, for the page to take. Before it's connected they wait. */
export function folderOpened(folders: string[]): void {
  openedFolders.push(...folders);
}

/** The bundle a macOS executable runs from. */
export function bundlePath(executable: string): string | null {
  const at = executable.indexOf(".app/");
  return at < 0 ? null : executable.slice(0, at + 4);
}

export function installCoreCommands(facts: AppFacts): void {
  let coldStartMs: number | null = null;
  addCommands({
    ipc_request: async ({ request }: Args) => {
      const outcome = await native().request(JSON.stringify(request));
      if (outcome.error !== undefined && outcome.error !== null) throw JSON.parse(outcome.error);
      return new RawJson(outcome.response ?? "null");
    },
    ipc_subscribe: ({ channel }: Args, { sender }) => {
      native().attachUi(channelSender(sender, channelId(channel)));
    },
    app_info: () => ({
      version: app.getVersion(),
      platform: facts.started.platform,
      processStartMs: facts.started.processStartMs,
      smoke: facts.smoke,
      firstLaunch: facts.started.firstLaunch,
      budgetTolerance: facts.budgetTolerance,
    }),
    app_ready: ({ readyMs }: Args) => {
      coldStartMs ??= (readyMs as number) - facts.started.processStartMs;
      return coldStartMs;
    },
    startup_finished: (_args: Args, { sender }) => {
      const window = BrowserWindow.fromWebContents(sender);
      if (window && !isQuitting()) clearBackdrop(window);
    },
    quit_app: () => quit(mainWindow),
    pages_flushed: ({ token }: Args) => pagesFlushed(token as number),
    take_opened_folders: () => openedFolders.splice(0),
    system_name: () => native().systemName(),
    uninstall_app: () => ({
      identifier,
      bundlePath: process.platform === "darwin" ? bundlePath(process.execPath) : null,
      pid: process.pid,
    }),
    get_continuity_report: async () => new RawJson(await native().continuityReport()),
    open_simulator: ({ device }: Args) => native().openSimulator(device as string | null | undefined),
    is_fullscreen: (_args: Args, { sender }) => BrowserWindow.fromWebContents(sender)?.isFullScreen() ?? false,
    close_window: (_args: Args, { sender }) => BrowserWindow.fromWebContents(sender)?.close(),
  });
}
