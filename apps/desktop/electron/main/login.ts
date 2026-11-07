// Open at login, System Settings' lists, and computer use's setup window: what src-tauri/src/login_item.rs
// and computer_setup.rs answer in the Tauri build. The setup window is a second window on the same page
// (`?window=computer-setup`), with computer use's status on its own channel.
import fs from "node:fs";
import path from "node:path";

import { app, BrowserWindow, shell } from "electron";

import { addCommands, channelId, channelSender, type Args } from "./commands";
import { bundlePath } from "./core";
import { note } from "./events";
import { dev, identifier, rendererUrl, unfocused } from "./identity";
import { autostartDir, desktopEntry, LOGIN_ITEMS_PANE, loginItemTook, macLoginItem, privacyPane, type LoginItem } from "./loginRules";
import { native } from "./native";
import { setFinishSetup } from "./tray";
import { preloadPath, windows } from "./windows";

const SETUP_WINDOW = "computer-setup";
/** The helper app's bundle name in the data folder, where it runs from. */
const HELPER_APP = "Brigadier Computer Use.app";
const SETUP_BACKGROUND = "#0f0f0e";

/** An error as the page gets the Tauri build's `IpcError`. */
const internal = (message: string) => ({ code: "internal", message });

/** Only the installed app can be a login item. A dev run never registers one on the user's Mac. */
const installedOnMac = () => !dev && bundlePath(process.execPath) !== null;

const autostartEntry = (): string | null => {
  const dir = autostartDir(process.env);
  return dir && path.join(dir, `${identifier}.desktop`);
};

function loginItem(): LoginItem {
  switch (process.platform) {
    case "darwin":
      return installedOnMac() ? macLoginItem(app.getLoginItemSettings({ type: "mainAppService" }).status) : "unsupported";
    case "win32":
      return app.getLoginItemSettings().openAtLogin ? "on" : "off";
    case "linux": {
      const entry = autostartEntry();
      if (!entry) return "unsupported";
      return fs.statSync(entry, { throwIfNoEntry: false })?.isFile() ? "on" : "off";
    }
    default:
      return "unsupported";
  }
}

function setLoginItem(on: boolean): void {
  switch (process.platform) {
    case "darwin":
      if (!installedOnMac()) throw new Error("Only the installed app can open at login.");
      // Unregistering what isn't registered is an error to SMAppService; nothing to do.
      if (!on && loginItem() === "off") return;
      app.setLoginItemSettings({ openAtLogin: on, type: "mainAppService" });
      break;
    case "win32":
      app.setLoginItemSettings({ openAtLogin: on });
      break;
    case "linux": {
      const entry = autostartEntry();
      if (!entry) throw new Error("No home folder to keep the autostart entry in.");
      if (!on) {
        fs.rmSync(entry, { force: true });
        return;
      }
      // An AppImage runs from a temporary mount: start the image itself.
      fs.mkdirSync(path.dirname(entry), { recursive: true });
      fs.writeFileSync(entry, desktopEntry(app.getName(), process.env.APPIMAGE ?? process.execPath));
      return;
    }
    default:
      throw new Error("Opening at login isn't available on this system.");
  }
  if (!loginItemTook(on, loginItem())) throw new Error("The login item could not be changed.");
}

async function openLoginItems(): Promise<void> {
  switch (process.platform) {
    case "darwin":
      return shell.openExternal(LOGIN_ITEMS_PANE);
    case "win32":
      throw new Error("Startup apps are in Settings › Apps › Startup.");
    case "linux":
      throw new Error("Startup applications are in your desktop's settings.");
    default:
      throw new Error("Opening at login isn't available on this system.");
  }
}

function bringForward(window: BrowserWindow, focus: boolean): void {
  if (window.isMinimized()) window.restore();
  // The driver places a dev window before it ever shows: never over the user's work.
  if (unfocused && !window.isVisible()) return;
  if (focus && !unfocused) {
    window.show();
    window.focus();
  } else {
    window.showInactive();
  }
}

function makeSetupWindow(): BrowserWindow {
  const window = new BrowserWindow({
    width: 560,
    height: 600,
    useContentSize: true,
    resizable: false,
    maximizable: false,
    fullscreenable: false,
    show: false,
    title: "Set up computer use",
    backgroundColor: SETUP_BACKGROUND,
    // A non-activating panel: a background app's plain window can't show over another app's full-screen
    // Space. Electron also puts a panel on every Space, where the Tauri build moves it to the active one.
    ...(process.platform === "darwin" ? { type: "panel" } : {}),
    focusable: !unfocused,
    webPreferences: {
      preload: preloadPath,
      sandbox: true,
      contextIsolation: true,
      nodeIntegration: false,
    },
  });
  // Electron floats a panel over every app; the Tauri build's is an ordinary window to the others.
  window.setAlwaysOnTop(false);
  windows.set(SETUP_WINDOW, window);
  for (const event of ["show", "hide", "minimize", "restore", "focus", "blur"] as const) {
    window.on(event as "show", () => note(`window-${event}`, { window: SETUP_WINDOW }));
  }
  // The page's own title would replace the window's.
  window.on("page-title-updated", (event) => event.preventDefault());
  // Closing really closes it, where the Tauri build only hides its webview. Hidden, a page here can go on
  // reading as visible, so SetupWindow would keep polling and not check afresh when it's shown again.
  window.on("closed", () => windows.delete(SETUP_WINDOW));
  // Nothing is dropped in, and a file dropped on a page would navigate it away from the setup.
  window.webContents.on("will-navigate", (event) => event.preventDefault());
  window.webContents.setWindowOpenHandler(() => ({ action: "deny" }));
  if (unfocused) window.setIgnoreMouseEvents(true);
  // `bcw` is how the dev driver finds a window's page.
  void window.loadURL(`${rendererUrl}?window=${SETUP_WINDOW}${dev ? `&bcw=${SETUP_WINDOW}` : ""}`);
  return window;
}

/** Opens the setup window, or brings it forward. `focus`: the user asked; opened for an agent, it leaves the keyboard alone. */
export function openComputerSetup(focus: boolean): void {
  note("computer-setup-open", { focus });
  bringForward(windows.get(SETUP_WINDOW) ?? makeSetupWindow(), focus);
}

export function installLoginCommands(dataDir: string): void {
  setFinishSetup(() => openComputerSetup(true));
  /** The placed helper app, which System Settings lists. */
  const helperApp = (): string => {
    if (process.platform !== "darwin") throw internal("computer use needs macOS");
    const placed = path.join(dataDir, "computer", HELPER_APP);
    if (!fs.existsSync(placed)) throw internal("the computer-use helper isn't ready yet; try again in a moment");
    return placed;
  };
  addCommands({
    login_item: () => loginItem(),
    set_login_item: ({ on }: Args) => {
      setLoginItem(on === true);
      return loginItem();
    },
    open_login_items: () => openLoginItems(),
    open_privacy_settings: async ({ permission }: Args) => {
      const pane = privacyPane(permission);
      await shell.openExternal(pane).catch((error: unknown) => {
        throw internal(`couldn't open System Settings: ${String(error)}`);
      });
    },
    open_computer_setup: () => openComputerSetup(true),
    computer_setup_subscribe: ({ channel }: Args, { sender }) => {
      native().attachSetupUi(channelSender(sender, channelId(channel)));
    },
    // A page can't drag a file out of its window; this runs while the button is still held.
    computer_setup_drag: async () => {
      const file = helperApp();
      const window = windows.get(SETUP_WINDOW);
      if (!window) throw internal("the setup window is closed");
      const icon = await app.getFileIcon(file, { size: "large" });
      window.webContents.startDrag({ file, icon: icon.resize({ width: 48, height: 48 }) });
    },
    reveal_computer_helper: () => shell.showItemInFolder(helperApp()),
  });
}
