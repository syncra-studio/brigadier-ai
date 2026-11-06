// The quit order, as in src-tauri/src/shell.rs. An orderly quit has the window save its pages' pending
// edits, while it still runs and the daemon still takes saves; then the daemon drains and acknowledges;
// then the app exits. Electron reports the Dock's Quit, logout and Cmd+Q alike as `before-quit`, so all of
// them come here, including the system quits the Tauri build could not flush for.
import { app, type BrowserWindow } from "electron";

import { note } from "./events";
import { native } from "./native";

const FLUSH_TIMEOUT_MS = 1500;
/** Covers the daemon's bounded ending of CLI sessions; it finishes quitting on its own if this runs out. */
const QUIT_TIMEOUT_MS = 10_000;
const HANDOVER_LIMIT_MS = 15_000;

let quitting = false;
let daemonStarted = false;
let daemonStopped = false;
let exitCode = 0;
let nextFlush = 1;
let beforeExit: (daemonStopped: boolean) => boolean = () => false;
let handedOver = false;
let waiting: { token: number; answered: () => void } | null = null;
/** A window close and a quit each finish their save request before the next replaces it. */
let flushGate: Promise<void> = Promise.resolve();

export const isQuitting = (): boolean => quitting;
export const didDaemonStop = (): boolean => daemonStopped;
export const markDaemonStarted = (): void => {
  daemonStarted = true;
};

/** Asks the window to save its pages' pending edits and waits for its answer, at most the timeout. */
export function flushPages(window: BrowserWindow | null): Promise<void> {
  if (!window || window.isDestroyed()) return Promise.resolve();
  const run = async () => {
    const token = nextFlush++;
    const answered = new Promise<boolean>((resolve) => {
      waiting = { token, answered: () => resolve(true) };
      setTimeout(() => resolve(false), FLUSH_TIMEOUT_MS);
    });
    window.webContents.send("brigadier:event", "flush-pages", token);
    const saved = await answered;
    waiting = null;
    note("flush-pages", { token, saved });
    if (!saved) console.warn("the window did not say its pages were saved; quitting anyway");
  };
  const done = flushGate.then(run);
  flushGate = done;
  return done;
}

/** The window's answer to `flush-pages`. */
export function pagesFlushed(token: number): void {
  if (waiting?.token === token) waiting.answered();
}

/** The app's last step before exiting, once the daemon's fate is known; true when something else now ends the process. */
export function setBeforeExit(step: (daemonStopped: boolean) => boolean): void {
  beforeExit = step;
}

/** Orderly quit: pages saved, the daemon drained and acknowledged, then exit with `code`. */
export function quit(window: () => BrowserWindow | null, code = 0): void {
  if (quitting) return;
  quitting = true;
  exitCode = code;
  void (async () => {
    await flushPages(window());
    const shown = window();
    if (shown && !shown.isDestroyed()) shown.hide();
    if (daemonStarted) {
      native().setQuitting(true);
      daemonStopped = await native().shutdownDaemon(QUIT_TIMEOUT_MS);
      note("daemon-quit", { acknowledged: daemonStopped });
      if (!daemonStopped) console.warn("brigadierd did not acknowledge the quit");
    }
    if (beforeExit(daemonStopped)) {
      // The updater ends the process itself, through app.quit; should it not, leave anyway.
      handedOver = true;
      setTimeout(() => app.exit(exitCode), HANDOVER_LIMIT_MS);
      return;
    }
    app.exit(exitCode);
  })();
}

/** Every way to quit that Electron knows of comes through the orderly path. */
export function holdQuits(window: () => BrowserWindow | null): void {
  app.on("before-quit", (event) => {
    if (handedOver) return;
    event.preventDefault();
    quit(window);
  });
}
