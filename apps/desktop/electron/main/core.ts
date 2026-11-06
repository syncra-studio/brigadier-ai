// The commands the app can't start without: the daemon's bridge, the app's own facts, quitting.
import { app, BrowserWindow } from "electron";

import { addCommands, channelId, channelSender, RawJson, type Args } from "./commands";
import { identifier } from "./identity";
import { isStaged } from "./intro";
import { native, type Started } from "./native";
import { isQuitting, pagesFlushed, quit } from "./quit";
import { clearBackdrop, mainWindow } from "./windows";

export type AppFacts = { started: Started; smoke: boolean; budgetTolerance: number; updateChannel: string | null };

const openedFolders: string[] = [];
let coldStart: number | null = null;

/** Process start to the app usable, once the page has said so. */
export const coldStartMs = (): number | null => coldStart;

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
      ...(facts.updateChannel ? { updateChannel: facts.updateChannel } : {}),
    }),
    app_ready: ({ readyMs }: Args) => {
      coldStart ??= (readyMs as number) - facts.started.processStartMs;
      return coldStart;
    },
    startup_finished: (_args: Args, { sender }) => {
      const window = BrowserWindow.fromWebContents(sender);
      // A staged intro keeps the window clear until its burst has opened it.
      if (window && !isQuitting() && !isStaged()) clearBackdrop(window);
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
