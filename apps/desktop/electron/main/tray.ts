// The menu-bar items and the app menu, as in src-tauri/src/shell.rs and computer_menu.rs. The Brigadier item
// is there from the start, which is what lets closing the window keep the app running; the computer-use item
// shows only while an agent is using apps.
import { execFile } from "node:child_process";

import { app, Menu, nativeImage, screen, Tray, type MenuItem, type MenuItemConstructorOptions } from "electron";

import type { ComputerSession, ComputerUseStatus, RunningChat } from "@/ipc/generated";

import computerIcon from "../../src-tauri/icons/computer-use-tray-active.png?inline";
import windowsIcon from "../../src-tauri/icons/icon.ico?inline";
import brigadierIcon from "../../src-tauri/icons/tray.png?inline";
import { addCommands, type Args } from "./commands";
import { dev } from "./identity";
import { native } from "./native";
import { quit } from "./quit";
import {
  appMenuTemplate,
  CLOSE_WINDOW_ITEM,
  computerTemplate,
  FEEDBACK_ITEM,
  icoEntry,
  OPEN_FOLDER_ITEM,
  plural,
  QUIT_ITEM,
  SETTINGS_ITEM,
  showing,
  trayTemplate,
  UNINSTALL_ITEM,
} from "./trayMenus";
import { mainWindow, setInMenuBar, showMain } from "./windows";

/** A menu-bar icon's height on macOS, in points. */
const MAC_ICON_POINTS = 18;
/** A menu's items answer only while it lives, and one still open can be older than the latest. */
const MENUS_KEPT = 4;
/** Longest a recording holds the menu's own items, should the page never say it ended. */
const KEY_CAPTURE_LIMIT_MS = 60_000;

/** While Settings records a key press, the menu's own items reach the page as these key presses. */
const CAPTURED_KEYS: Record<string, string> = {
  [CLOSE_WINDOW_ITEM]: "close-window",
  [SETTINGS_ITEM]: "settings",
  [OPEN_FOLDER_ITEM]: "open-folder",
};
const MENU_EVENTS: Record<string, string> = {
  [OPEN_FOLDER_ITEM]: "openFolderMenu",
  [UNINSTALL_ITEM]: "uninstallMenu",
  [SETTINGS_ITEM]: "settingsMenu",
  [FEEDBACK_ITEM]: "feedbackMenu",
};

// Held here: a tray nobody holds is collected, and leaves the menu bar.
let brigadierTray: Tray | null = null;
let computerTray: Tray | null = null;
let brigadierMenu: Menu | null = null;
let computerMenu: Menu | null = null;
const menusKept: Menu[] = [];
let captureUntil = 0;
let computerShown = false;
let computerStatus: ComputerUseStatus | null = null;
let whenDaemon: () => Promise<void> = () => Promise.resolve();
let finishSetup: (() => void) | null = null;

/** Sends the page a bridge event; one chosen before the bridge is up waits for it. */
function emit(event: Record<string, unknown>): void {
  whenDaemon()
    .then(() => native().emit(JSON.stringify(event)))
    .catch((error: unknown) => console.warn("couldn't tell the page about a menu item", error));
}

function sendKey(key: string): void {
  mainWindow()?.webContents.send("brigadier:event", "pane-shortcut", key);
}

function appMenuChosen(id: string): void {
  if (id.startsWith("pane:")) {
    sendKey(id.slice("pane:".length));
    return;
  }
  // While Settings records a key press, the menu's own items are key presses too.
  const captured = CAPTURED_KEYS[id];
  if (captured && Date.now() < captureUntil) {
    sendKey(captured);
    return;
  }
  if (id === QUIT_ITEM) {
    quit(mainWindow);
  } else if (id === CLOSE_WINDOW_ITEM) {
    mainWindow()?.close();
  } else if (MENU_EVENTS[id]) {
    showMain();
    emit({ type: MENU_EVENTS[id] });
  }
}

function openConversation(conversationId: string): void {
  showMain();
  emit({ type: "openConversation", conversationId });
}

function made(template: MenuItemConstructorOptions[]): Menu {
  const menu = Menu.buildFromTemplate(template);
  menusKept.push(menu);
  if (menusKept.length > MENUS_KEPT) menusKept.shift();
  return menu;
}

function setBrigadierMenu(running: readonly RunningChat[]): Menu {
  brigadierMenu = made(trayTemplate(running, { open: showMain, quit: () => quit(mainWindow), chat: openConversation }));
  brigadierTray?.setContextMenu(brigadierMenu);
  return brigadierMenu;
}

/** Brings the app the agent drives to the front (the user asked for it). */
function showApp(session: ComputerSession): void {
  const args = session.bundleId ? ["-b", session.bundleId] : ["-a", session.app];
  execFile("/usr/bin/open", args, (error) => {
    if (error) console.warn(`couldn't show ${session.app}`, error);
  });
}

async function stopSession(session: ComputerSession): Promise<void> {
  const outcome = await native().request(
    JSON.stringify({ method: "stopComputerSession", session: session.session, bundleId: session.bundleId || null, app: session.app }),
  );
  if (outcome.error) console.warn("couldn't stop the agent using the app", outcome.error);
}

function openComputerSettings(): void {
  showMain();
  emit({ type: "openComputerUseSettings" });
}

function makeComputerMenu(status: ComputerUseStatus, sessions: ComputerSession[]): Menu {
  return made(computerTemplate(status, sessions, {
    open: ({ conversationId }) => {
      if (conversationId) openConversation(conversationId);
    },
    show: showApp,
    stop: (session) => void stopSession(session),
    settings: openComputerSettings,
    setup: () => (finishSetup ?? openComputerSettings)(),
  }));
}

/** A PNG (given as a data URL) at its size in the menu bar: macOS draws it, in its text colour, by height. */
function menuBarIcon(dataUrl: string): Electron.NativeImage {
  const png = Buffer.from(dataUrl.slice(dataUrl.indexOf(",") + 1), "base64");
  const { height } = nativeImage.createFromBuffer(png).getSize();
  const image = nativeImage.createFromBuffer(png, { scaleFactor: height / MAC_ICON_POINTS });
  image.setTemplateImage(true);
  return image;
}

/** Windows shows the app's icon, drawn for the notification area's size: 16 px at 100%, 20 at 125%… */
function windowsTrayIcon(dataUrl: string): Electron.NativeImage {
  const ico = Buffer.from(dataUrl.slice(dataUrl.indexOf(",") + 1), "base64");
  const scaleFactor = screen.getPrimaryDisplay().scaleFactor;
  return nativeImage.createFromBuffer(Buffer.from(icoEntry(ico, Math.round(16 * scaleFactor))), { scaleFactor });
}

/** A menu-bar item whose menu opens on a left click too, which only Windows needs to be told. */
function makeTray(image: Electron.NativeImage, tooltip: string, menu: Menu): Tray {
  const tray = new Tray(image);
  tray.setToolTip(tooltip);
  tray.setContextMenu(menu);
  if (process.platform === "win32") tray.on("click", () => tray.popUpContextMenu());
  return tray;
}

/** Shows, updates or removes the computer-use item. */
function refreshComputer(): void {
  const sessions = showing(computerShown, computerStatus);
  if (!sessions || !computerStatus) {
    computerTray?.destroy();
    computerTray = null;
    computerMenu = null;
    return;
  }
  const menu = makeComputerMenu(computerStatus, sessions);
  computerMenu = menu;
  const tooltip = `Computer use: ${plural(sessions.length)} in use`;
  if (computerTray) {
    computerTray.setContextMenu(menu);
    computerTray.setToolTip(tooltip);
  } else {
    computerTray = makeTray(menuBarIcon(computerIcon), tooltip, menu);
  }
}

/** The daemon's computer-use status changed. */
export function computerStatusChanged(status: ComputerUseStatus): void {
  computerStatus = status;
  refreshComputer();
}

/** What "Finish setup…" opens, from the computer-setup area; until then, computer use's settings. */
export function setFinishSetup(open: () => void): void {
  finishSetup = open;
}

/** Dev only: a menu as the checks read it. */
function readMenu(menu: Menu | null): unknown[] {
  return (menu?.items ?? []).map((item) => item.type === "separator" ? "-" : {
    label: item.label,
    enabled: item.enabled,
    ...(item.accelerator ? { accelerator: item.accelerator } : {}),
    ...(item.submenu ? { submenu: readMenu(item.submenu) } : {}),
  });
}

function findItem(menu: Menu | null, path: string[]): MenuItem {
  const [first, ...rest] = path;
  const item = menu?.items.find((each) => each.label === first);
  if (!item) throw new Error(`no menu item ${first}`);
  return rest.length > 0 ? findItem(item.submenu ?? null, rest) : item;
}

const devMenus = (): Record<string, Menu | null> => ({ app: Menu.getApplicationMenu(), brigadier: brigadierMenu, computer: computerMenu });

/** Dev only: reads the menus and chooses their items the way a click does, for scripted checks. */
function devCommand(): (args: Args) => unknown {
  return ({ op, menu, path, status, opacity }: Args) => {
    if (op === "read") {
      return {
        inMenuBar: brigadierTray !== null,
        computerItem: computerTray !== null,
        bounds: { brigadier: brigadierTray?.getBounds() ?? null, computer: computerTray?.getBounds() ?? null },
        capturing: Date.now() < captureUntil,
        app: readMenu(Menu.getApplicationMenu()),
        brigadier: readMenu(brigadierMenu),
        computer: readMenu(computerMenu),
      };
    }
    if (op === "click") {
      (findItem(devMenus()[menu as string] ?? null, path as string[]).click as () => void)();
      return true;
    }
    if (op === "status") {
      computerStatusChanged(status as ComputerUseStatus);
      return true;
    }
    // macOS puts a window shown again back on the screen: the check brings it back unseen, then parks it.
    if (op === "opacity") {
      mainWindow()?.setOpacity(opacity as number);
      return true;
    }
    throw new Error(`unknown op ${String(op)}`);
  };
}

/** The menus and their commands, once the app is ready. A desktop with no menu-bar host just has no item. */
export function installTray(daemon: () => Promise<void>): void {
  whenDaemon = daemon;
  addCommands({
    set_running_chats: ({ chats }: Args) => {
      setBrigadierMenu(chats as RunningChat[]);
    },
    set_key_capture: ({ on }: Args) => {
      captureUntil = on ? Date.now() + KEY_CAPTURE_LIMIT_MS : 0;
    },
    set_computer_menu: ({ shown }: Args) => {
      computerShown = shown as boolean;
      refreshComputer();
    },
    ...(dev ? { dev_tray: devCommand() } : {}),
  });
  // The standard Quit is left out for ours. Windows and Linux have no app menu, as in the Tauri build.
  Menu.setApplicationMenu(process.platform === "darwin" ? Menu.buildFromTemplate(appMenuTemplate(app.getName(), appMenuChosen)) : null);
  try {
    const image = process.platform === "win32" ? windowsTrayIcon(windowsIcon) : menuBarIcon(brigadierIcon);
    brigadierTray = makeTray(image, app.getName(), setBrigadierMenu([]));
    setInMenuBar(true);
  } catch (error) {
    console.warn("menu-bar item unavailable", error);
  }
}
