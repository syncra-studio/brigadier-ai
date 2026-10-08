/**
 * The startup screen: Brigadier's mark on the window's own backdrop (the native blur on macOS,
 * the window colour elsewhere), drawn by `index.html` before any script runs. It stays until
 * the app is ready, then the app fades in under it and it goes. `#root` renders underneath but
 * stays invisible until then, and the splash takes every click.
 *
 * If startup hangs, the app is shown anyway once React has mounted (it shows its own
 * not-connected state); before that the splash says it is still starting and offers Try again.
 */
import { useSyncExternalStore } from "react";

import { startupFinished } from "@/ipc/client";
import { markStartup } from "@/lib/startup";

/** How long startup may take before the splash gives up waiting (also `--splash-late-after`). */
const STALL_MS = 10_000;

let mounted = false;
let revealed = false;
let reveal: Promise<void> | null = null;
const listeners = new Set<() => void>();
let finishStartup = () => {};
const startupDone = new Promise<void>((resolve) => {
  finishStartup = resolve;
});

function splash(): HTMLElement | null {
  return document.getElementById("splash");
}

/** Starts the stall timer. Call first thing, before startup awaits anything. */
export function watchStartup(): void {
  setTimeout(() => {
    if (revealed) return;
    if (mounted) void revealApp();
    else showRetry("Still starting…");
  }, STALL_MS);
}

/** Startup failed before the app could draw: say so and offer Try again. */
export function showStartupError(): void {
  showRetry("Brigadier couldn't start.");
}

function showRetry(message: string): void {
  const note = document.getElementById("splash-note");
  const retry = document.getElementById("splash-retry");
  if (!note || !retry) return;
  note.textContent = message;
  note.dataset.shown = "";
  if (!retry.hidden) return;
  retry.hidden = false;
  retry.addEventListener("click", () => location.reload());
}

/** Says under the mark what startup is waiting for. */
export function setSplashStatus(text: string): void {
  const status = document.getElementById("splash-status");
  if (!status) return;
  status.textContent = text;
  status.dataset.shown = "";
}

/** React has mounted the app, so revealing it shows something. */
export function markMounted(): void {
  mounted = true;
}

/** Fades the app in and the splash out; resolves once the splash is gone. Safe to call twice. */
export function revealApp(): Promise<void> {
  reveal ??= fadeOut();
  return reveal;
}

async function fadeOut(): Promise<void> {
  const element = splash();
  const root = document.documentElement;
  if (element) {
    root.dataset.splash = "leaving";
    markStartup("fading");
    // The transitions start at the next style pass; with reduced motion there are none.
    void getComputedStyle(element).opacity;
    await Promise.all(element.getAnimations().map((animation) => animation.finished)).catch(
      () => {},
    );
    element.remove();
  }
  delete root.dataset.splash;
  revealed = true;
  for (const listener of listeners) listener();
  // The window's startup blur is covered from here on, so the app is shown without waiting for
  // it to be cleared; work that waits for startup to be over (whenRevealed) waits for that too.
  void startupFinished()
    .catch((error: unknown) => {
      console.error("clearing the startup backdrop failed", error);
    })
    .then(finishStartup);
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/**
 * Resolves once startup is over (the splash gone, the window's startup backdrop cleared): work
 * the app can show without waits for it. A page without a splash (the pane fixtures) has nothing
 * to wait for.
 */
export function whenRevealed(): Promise<void> {
  return document.documentElement.hasAttribute("data-splash") ? startupDone : Promise.resolve();
}

/** Whether the splash has gone: dialogs that open on their own wait for it. */
export function useRevealed(): boolean {
  return useSyncExternalStore(subscribe, () => revealed);
}
