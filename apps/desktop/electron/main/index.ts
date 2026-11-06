// Brigadier's Electron main process. Tools/browser-check keeps one dev instance alive and drives it.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { app, BrowserWindow, protocol } from "electron";

import type { ComputerUseStatus } from "@/ipc/generated";

import { installBrowserHost } from "./browserHost";
import { installCommands, waitForCommands } from "./commands";
import { folderOpened, installCoreCommands } from "./core";
import { note } from "./events";
import { daemonDataDir, dev, identifier, setIdentity, unfocused } from "./identity";
import { native } from "./native";
import { folderArgs } from "./paths";
import { holdQuits, markDaemonStarted, quit } from "./quit";
import { computerStatusChanged, installTray } from "./tray";
import { createWindow, mainWindow, setOnVisibility, showMain, userCameToIt, windows } from "./windows";

const here = path.dirname(fileURLToPath(import.meta.url));

setIdentity();
app.enableSandbox();
if (!app.requestSingleInstanceLock()) app.exit(0);

// The production page's origin: a real scheme, so localStorage and fetch work as on the web.
protocol.registerSchemesAsPrivileged([
  { scheme: "app", privileges: { standard: true, secure: true, supportFetchAPI: true, stream: true } },
]);

installBrowserHost();
installCommands();

const smoke = process.argv.includes("--smoke");
const TOLERANCE_ENV = "BRIGADIER_BUDGET_TOLERANCE";
const budgetTolerance = (() => {
  const value = Number(process.env[TOLERANCE_ENV]);
  return Number.isFinite(value) && value >= 1 ? value : 1;
})();

let daemonReady: Promise<void> | null = null;

/** Tells the bridge whether a window is in front, which it can't ask from its own threads. */
const front = () => native().setInFront(BrowserWindow.getAllWindows().some((window) => window.isVisible() && window.isFocused()));

/** The bridge to the daemon, once. Before the first real window, since the page's first call needs it. */
function startDaemon(): Promise<void> {
  daemonReady ??= (async () => {
    const started = await native().start(
      {
        identifier,
        ...(daemonDataDir() ? { dataDir: daemonDataDir() as string } : {}),
        ...(process.env.BRIGADIER_DEV_SIDECARS ? { sidecarDir: process.env.BRIGADIER_DEV_SIDECARS } : {}),
        daemonEnv: [
          ...(process.env[TOLERANCE_ENV] ? [[TOLERANCE_ENV, process.env[TOLERANCE_ENV] as string]] : []),
          // A dev run's daemon can be kept off the user's real CLI sign-ins and settings.
          ...(dev && process.env.BRIGADIER_DEV_DAEMON_HOME ? [["HOME", process.env.BRIGADIER_DEV_DAEMON_HOME]] : []),
        ],
      },
      (json) => hearDaemon(JSON.parse(json) as ShellAsk),
    );
    markDaemonStarted();
    note("daemon-bridge-started", { dataDir: started.dataDir, firstLaunch: started.firstLaunch });
    installCoreCommands({ started, smoke, budgetTolerance });
    setOnVisibility((visible) => {
      native().emit(JSON.stringify({ type: "windowVisibility", visible }));
    });
    for (const name of ["browser-window-focus", "browser-window-blur"] as const) app.on(name as "browser-window-focus", front);
  })();
  waitForCommands(daemonReady);
  return daemonReady;
}

type ShellAsk = { kind: "notify"; title: string; body: string } | { kind: "openSetup" } | { kind: "computerUse"; status: ComputerUseStatus };

/** What the bridge asks of the shell; each is filled in by its own area. */
function hearDaemon(ask: ShellAsk): void {
  note("daemon-asks", { kind: ask.kind });
  if (ask.kind === "computerUse") computerStatusChanged(ask.status);
}

async function openMain(): Promise<void> {
  await startDaemon();
  const window = createWindow({ real: true });
  window.once("ready-to-show", showMain);
}

app.on("second-instance", (_event, argv, cwd) => {
  showMain();
  folderOpened(folderArgs(argv.slice(1), cwd, isFolder));
});
const isFolder = (candidate: string) => {
  try {
    return fs.statSync(candidate).isDirectory();
  } catch {
    return false;
  }
};
folderOpened(folderArgs(process.argv.slice(1), process.cwd(), isFolder));

holdQuits(mainWindow);
// Closing every window never quits by itself: the close handler decides.
app.on("window-all-closed", () => {});
app.on("activate", () => {
  if (unfocused) return;
  userCameToIt();
  showMain();
});

if (dev) {
  const { installDevControl } = await import("./devControl");
  installDevControl({
    windows,
    note,
    createWindow: (options) => {
      if (!options.real) return createWindow(options);
      // Waits on nothing: the page's first call waits on the bridge through the commands it needs.
      void startDaemon();
      return createWindow(options);
    },
  });
}

// Not awaited at the top level: Electron won't reach "ready" until this module has finished loading.
void app.whenReady().then(async () => {
  installTray(startDaemon);
  if (dev) {
    note("ready", { pid: process.pid, electron: process.versions.electron, chrome: process.versions.chrome, sandboxForced: true });
    // The first window waits for the driver: the checks place each window before it shows.
    const control = new BrowserWindow({
      show: false,
      webPreferences: { preload: path.join(here, "preload.cjs"), sandbox: true, contextIsolation: true },
    });
    windows.set("control", control);
    void control.loadFile(process.env.BRIGADIER_CONTROL_HTML as string);
    return;
  }
  try {
    await openMain();
  } catch (error) {
    console.error("brigadier: failed to start", error);
    quit(mainWindow, 1);
  }
});
