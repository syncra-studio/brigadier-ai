import { useEffect, useSyncExternalStore } from "react";

import type { ComputerAccess, ComputerGrant } from "@/ipc/generated";
import { request } from "@/ipc/client";
import { useApp } from "@/state/store";

/*
 * Computer use's system permissions, as last read. Only the helper knows them, so a read starts
 * it: only what is about computer use reads them (its Settings page, a session's permission row).
 */

let access: ComputerAccess | null = null;
const listeners = new Set<() => void>();

function set(next: ComputerAccess): void {
  access = next;
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** The permissions as last read; null until the first read answers. */
export function useComputerAccess(): ComputerAccess | null {
  return useSyncExternalStore(subscribe, () => access);
}

/**
 * Whether this system has computer use (macOS), from the app's own info: Settings lists its page
 * without a read, so showing Settings (the launch smoke check opens its Inspector) never starts
 * the helper.
 */
export function computerUseAvailable(): boolean {
  return useApp.getState().info?.platform === "macos";
}

let reading = false;

/** Reads the permissions again (the user may have changed them in System Settings). One read at a time. */
export function readComputerAccess(): void {
  if (reading) return;
  reading = true;
  request({ method: "getComputerAccess" })
    .then((response) => set(response.access))
    .catch((error: unknown) => console.error("computer access read failed", error))
    .finally(() => {
      reading = false;
    });
}

/** How often a shown permission row reads again while one is missing or the helper restarts. */
export const WATCH_MS = 1500;
/** How often it reads again while both are allowed: the user may still take one away. */
export const WATCH_ALLOWED_MS = 3000;

/**
 * How often a shown permission row reads again: often while one is missing (the user may be
 * turning it on in System Settings right now) or the helper is restarting to use a grant, less
 * often once both are allowed. Never where the system has no computer use.
 */
export function watchEvery(read: ComputerAccess | null): number | null {
  if (read?.available === false) return null;
  return read === null || read.restarting || !read.accessibility || !read.screenRecording ? WATCH_MS : WATCH_ALLOWED_MS;
}

/**
 * The permissions, kept current while shown: read when shown, when the window comes back, and
 * every {@link watchEvery}, so a change in System Settings shows without a click here.
 */
export function useLiveComputerAccess(): ComputerAccess | null {
  const current = useComputerAccess();
  const every = watchEvery(current);
  useEffect(() => {
    readComputerAccess();
    window.addEventListener("focus", readComputerAccess);
    return () => window.removeEventListener("focus", readComputerAccess);
  }, []);
  useEffect(() => {
    if (every === null) return;
    const timer = window.setInterval(readComputerAccess, every);
    return () => window.clearInterval(timer);
  }, [every]);
  return current;
}

/**
 * Asks the system for one permission; System Settings opens on its pane. `startOver` first
 * forgets Brigadier Computer Use's own entry for it, one an older build left that no longer matches.
 */
export async function allowComputerAccess(grant: ComputerGrant, startOver = false): Promise<void> {
  set((await request({ method: "allowComputerAccess", grant, startOver })).access);
}
