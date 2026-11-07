// The first launch's intro, from the window's side (the page plays the rest: app/setup/onboarding), as
// src-tauri/src/intro.rs. On macOS the first launch holds the window clear until the page is ready, then
// stages it: no title-bar buttons, shadow or colour (only the page's orb is seen), above a black window that
// fades in to dim the desktop. When the burst has filled the window its chrome comes back; then the dim fades
// out. Hiding the window, a page load and the deadlines all end it, and every end restores the window.
import { BrowserWindow, screen } from "electron";

import { addCommands } from "./commands";
import { unfocused } from "./identity";
import { fadeValue } from "./introRules";
import { WINDOW_BACKGROUND, mainWindow, showMain } from "./windows";

const HOLD_DEADLINE_MS = 15_000;
const STAGED_DEADLINE_MS = 20_000;
const QUICK_FADE_MS = 300;
const DIM = 0.85;
const DIM_AFTER_MS = 950;
const DIM_IN_MS = 700;
const ENDED_EVENT = "intro-ended";

let held = false;
let staged = false;
let ends = 0;
let dim: BrowserWindow | null = null;
let fadeTimer: ReturnType<typeof setInterval> | null = null;

/** Its startup colour must not cover the page's orb while staged. */
export const isStaged = (): boolean => staged;

function fade(window: BrowserWindow, to: number, ms: number, done?: () => void): void {
  if (fadeTimer) clearInterval(fadeTimer);
  const from = window.getOpacity();
  const started = Date.now();
  fadeTimer = setInterval(() => {
    if (window.isDestroyed()) {
      if (fadeTimer) clearInterval(fadeTimer);
      return;
    }
    const progress = ms <= 0 ? 1 : (Date.now() - started) / ms;
    window.setOpacity(fadeValue(from, to, progress));
    if (progress >= 1) {
      if (fadeTimer) clearInterval(fadeTimer);
      fadeTimer = null;
      done?.();
    }
  }, 16);
}

/** The screen below the menu bar. A dev run keeps it to the parked window, off the user's screen. */
function dimBounds(window: BrowserWindow): Electron.Rectangle {
  if (unfocused) return window.getBounds();
  const { bounds, workArea } = screen.getPrimaryDisplay();
  const menuBar = Math.max(0, workArea.y - bounds.y);
  return { x: bounds.x, y: workArea.y, width: bounds.width, height: bounds.height - menuBar };
}

function dimWindow(window: BrowserWindow): BrowserWindow {
  const made = new BrowserWindow({
    ...dimBounds(window),
    show: false,
    frame: false,
    hasShadow: false,
    focusable: false,
    skipTaskbar: true,
    backgroundColor: "#000000",
    enableLargerThanScreen: true,
    webPreferences: { sandbox: true, contextIsolation: true },
  });
  made.setOpacity(0);
  made.setIgnoreMouseEvents(true);
  made.setAlwaysOnTop(true, "floating");
  made.setVisibleOnAllWorkspaces(true, { visibleOnFullScreen: true, skipTransformProcessType: true });
  return made;
}

function shape(window: BrowserWindow): void {
  window.setHasShadow(true);
  window.setWindowButtonVisibility(true);
  window.setBackgroundColor(WINDOW_BACKGROUND);
}

function restore(window: BrowserWindow, fadeMs: number): void {
  shape(window);
  const lower = () => {
    if (window.isDestroyed()) return;
    window.setAlwaysOnTop(false);
    // Another app may have become the active one through a click on the dimmed desktop.
    if (!unfocused) window.moveTop();
  };
  const going = dim;
  dim = null;
  if (!going || going.isDestroyed()) {
    lower();
    return;
  }
  const close = () => {
    if (!going.isDestroyed()) going.destroy();
    lower();
  };
  if (fadeMs <= 0) close();
  else fade(going, 0, fadeMs, close);
}

function end(fadeMs: number, tellPage: boolean): void {
  if (!staged) return;
  staged = false;
  ends += 1;
  const window = mainWindow();
  if (window) {
    restore(window, fadeMs);
    if (tellPage) window.webContents.send("brigadier:event", ENDED_EVENT);
  }
}

/** The window is hidden, closed, or its page loads again: the dim goes at once and the page is told. */
export function interruptIntro(): void {
  end(0, true);
}

function unhold(): void {
  const window = mainWindow();
  if (window) {
    window.setIgnoreMouseEvents(unfocused);
    if (!unfocused) window.setOpacity(1);
  }
  showMain();
}

/** Holds a new window clear: a hidden window's page doesn't paint, and startup waits for a paint. */
export function holdForIntro(window: BrowserWindow): void {
  held = true;
  // A new page has no intro playing, and the intro doesn't outlive its window being seen.
  window.webContents.on("did-start-navigation", (details) => {
    if (details.isMainFrame && !details.isSameDocument) interruptIntro();
  });
  window.on("hide", interruptIntro);
  if (!unfocused) {
    window.setOpacity(0);
    window.setIgnoreMouseEvents(true);
    showMain();
  }
  setTimeout(() => {
    if (held) {
      held = false;
      console.warn("the page didn't begin the intro; showing the window");
      unhold();
    }
  }, HOLD_DEADLINE_MS);
}

export function installIntroCommands(): void {
  addCommands({
    intro_begin: () => {
      const window = mainWindow();
      if (!held || !window) return false;
      held = false;
      staged = true;
      dim = dimWindow(window);
      window.setVibrancy(null);
      window.setBackgroundColor("#00000000");
      window.setHasShadow(false);
      window.setWindowButtonVisibility(false);
      window.setAlwaysOnTop(true, "floating");
      if (!unfocused) window.center();
      dim.showInactive();
      // Brought forward after the dim, so it stands above it on the same level.
      window.setIgnoreMouseEvents(unfocused);
      if (!unfocused) window.setOpacity(1);
      showMain();
      const going = dim;
      setTimeout(() => {
        if (dim === going && !going.isDestroyed()) fade(going, DIM, DIM_IN_MS);
      }, DIM_AFTER_MS);
      const armed = ends;
      setTimeout(() => {
        if (ends === armed && staged) {
          console.warn("the intro ran past its deadline; ending it");
          end(QUICK_FADE_MS, true);
        }
      }, STAGED_DEADLINE_MS);
      return true;
    },
    intro_shaped: () => {
      const window = mainWindow();
      if (staged && window) shape(window);
    },
    intro_finish: ({ fadeMs }) => end(Math.min(fadeMs as number, 5000), false),
    intro_release: () => {
      if (held) {
        held = false;
        unhold();
      }
    },
  });
}
