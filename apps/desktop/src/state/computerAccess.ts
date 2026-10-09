import { useEffect, useSyncExternalStore } from "react";

import type { ComputerAccess, ComputerGrant } from "@/ipc/generated";
import { request } from "@/ipc/client";

/*
 * Computer use's system permissions, as last read. Settings shows its page only where the
 * system has computer use, so the navigation and the page share this.
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

/** Whether this system has computer use, as last read. */
export function computerUseAvailable(): boolean {
  return access?.available === true;
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

/** How often a shown permission row reads again while one is missing. */
export const WATCH_MS = 1500;

/**
 * Whether the permissions may still change by themselves: one is missing (the user may be
 * turning it on in System Settings right now), or the helper is restarting to use a grant.
 */
export function watchingComputerAccess(read: ComputerAccess | null): boolean {
  return read === null || read.restarting || !read.accessibility || !read.screenRecording;
}

/**
 * The permissions, kept current while shown: read when shown, when the window comes back, and
 * every {@link WATCH_MS} while one is missing, so a grant in System Settings shows without a
 * click here.
 */
export function useLiveComputerAccess(): ComputerAccess | null {
  const current = useComputerAccess();
  const watching = current?.available !== false && watchingComputerAccess(current);
  useEffect(() => {
    readComputerAccess();
    window.addEventListener("focus", readComputerAccess);
    return () => window.removeEventListener("focus", readComputerAccess);
  }, []);
  useEffect(() => {
    if (!watching) return;
    const timer = window.setInterval(readComputerAccess, WATCH_MS);
    return () => window.clearInterval(timer);
  }, [watching]);
  return current;
}

/**
 * Asks the system for one permission; System Settings opens on its pane. `startOver` first
 * forgets Brigadier Computer Use's own entry for it, one an older build left that no longer matches.
 */
export async function allowComputerAccess(grant: ComputerGrant, startOver = false): Promise<void> {
  set((await request({ method: "allowComputerAccess", grant, startOver })).access);
}
