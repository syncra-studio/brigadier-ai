// The app's windows. The real one talks to the daemon; in a dev run a named stand-in window still runs the
// stage 1 browser checks on their own in-page sessions.
import path from "node:path";
import { fileURLToPath } from "node:url";

import { app, BrowserWindow, shell } from "electron";

import { dev, rendererUrl, unfocused } from "./identity";
import { note } from "./events";
import { flushPages, isQuitting, quit } from "./quit";

const here = path.dirname(fileURLToPath(import.meta.url));
export const preloadPath = path.join(here, "preload.cjs");
export const guestPasskeyPath = path.join(here, "guestPasskey.cjs");

/** The window's colour once the app covers it (the Tauri build's backgroundColor). */
export const WINDOW_BACKGROUND = "#0f0f0e";

export const windows = new Map<string, BrowserWindow>();
let main: BrowserWindow | null = null;
/** Whether the menu-bar item can bring a closed window back; else closing quits. */
let inMenuBar = false;
/** A build a session launched as an app preview never takes the focus, until the user comes to it (#142). */
let background = process.env.BRIGADIER_BACKGROUND_LAUNCH === "1" || unfocused;
let onVisibility: (visible: boolean) => void = () => {};

export const mainWindow = (): BrowserWindow | null => (main && !main.isDestroyed() ? main : null);
export const setInMenuBar = (on: boolean): void => {
  inMenuBar = on;
};
export const setOnVisibility = (listener: (visible: boolean) => void): void => {
  onVisibility = listener;
};
/** The user came to the app (its Dock icon): from now on it comes forward as usual. */
export const userCameToIt = (): void => {
  background = unfocused;
};

function setDockShown(shown: boolean): void {
  if (process.platform === "darwin" && !unfocused) app.setActivationPolicy(shown ? "regular" : "accessory");
}

export function showMain(): void {
  const window = mainWindow();
  setDockShown(true);
  if (window) {
    if (window.isMinimized()) window.restore();
    if (background) {
      window.showInactive();
    } else {
      window.show();
      window.focus();
    }
  }
  onVisibility(true);
}

export function hideMain(): void {
  mainWindow()?.hide();
  // A menu-bar app has no Dock icon while its window is closed.
  setDockShown(false);
  onVisibility(false);
}

/** The startup screen's blur behind the window (macOS). */
function applyBackdrop(window: BrowserWindow): void {
  if (process.platform !== "darwin") return;
  window.setBackgroundColor("#00000000");
  window.setVibrancy("menu", { animationDuration: 0 });
}

/** The startup screen has gone and the app covers the window: its backdrop goes too. */
export function clearBackdrop(window: BrowserWindow): void {
  if (process.platform === "darwin") window.setVibrancy(null);
  window.setBackgroundColor(WINDOW_BACKGROUND);
}

type WindowOptions = { name?: string; bounds?: Partial<Electron.Rectangle>; query?: string; real?: boolean; transparent?: boolean };

/** Makes a window of the app's page. A real one reaches the daemon through the preload's native door. */
export function createWindow({ name = "main", bounds = {}, query = "", real = !dev, transparent = false }: WindowOptions = {}): BrowserWindow {
  const window = new BrowserWindow({
    width: 1280,
    height: 820,
    minWidth: real ? 900 : 0,
    minHeight: real ? 560 : 0,
    ...bounds,
    show: false,
    // Only a first launch, whose intro shows nothing but the page's orb: a held window needs it from birth.
    ...(transparent ? { transparent: true } : {}),
    title: dev ? "Brigadier Dev" : "Brigadier",
    backgroundColor: real && process.platform === "darwin" ? "#00000000" : WINDOW_BACKGROUND,
    titleBarStyle: process.platform === "darwin" ? "hidden" : "default",
    // Their 14-point row centred on the 44-point title bar (tokens.css: --spacing-titlebar-toggle), level with
    // Back. Electron's y is the distance to their top; Tauri's y: 24 was a title-bar height, not the same thing.
    trafficLightPosition: { x: 16, y: 15 },
    ...(real && process.platform === "darwin" ? { vibrancy: "menu" as const, visualEffectState: "active" as const } : {}),
    focusable: !unfocused,
    minimizable: true,
    webPreferences: {
      preload: preloadPath,
      sandbox: true,
      contextIsolation: true,
      nodeIntegration: false,
      webviewTag: true,
      // The page keeps running while unseen, or onboarding would wait for the window to come forward (#134).
      backgroundThrottling: false,
      // The preload runs in every subframe too, only to lock it: a visual's child frames (preload.ts).
      ...(real ? { nodeIntegrationInSubFrames: true } : {
        // One localStorage per named window: the checks run a session in each, as separate users would.
        partition: `persist:shell-${name}`,
        additionalArguments: ["--brigadier-stand-in"],
      }),
    },
  });
  windows.set(name, window);
  for (const event of ["show", "hide", "minimize", "restore", "focus", "blur", "enter-full-screen", "leave-full-screen"] as const) {
    window.on(event as "show", () => note(`window-${event}`, { window: name }));
  }
  window.on("closed", () => windows.delete(name));
  window.webContents.setWindowOpenHandler(({ url }) => {
    if (/^https?:/.test(url)) void shell.openExternal(url);
    return { action: "deny" };
  });
  const resized = () => {
    if (!window.isDestroyed()) window.webContents.send("brigadier:resized");
  };
  for (const event of ["resize", "enter-full-screen", "leave-full-screen"] as const) window.on(event as "resize", resized);
  if (unfocused) window.setIgnoreMouseEvents(true);
  if (real) {
    main = window;
    window.on("close", (event) => {
      if (isQuitting()) return;
      // Closing the window keeps Brigadier running in the menu bar, with its pages saved first.
      event.preventDefault();
      if (inMenuBar) void flushPages(window).then(hideMain);
      else quit(mainWindow);
    });
    // A new page starts on the startup screen again, which needs the backdrop behind it.
    window.webContents.on("did-start-navigation", (details) => {
      if (details.isMainFrame && !details.isSameDocument) applyBackdrop(window);
    });
  }
  void window.loadURL(`${rendererUrl}${query ? `?${query}` : ""}`);
  return window;
}
