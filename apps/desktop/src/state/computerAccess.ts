import { useSyncExternalStore } from "react";

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

/** Reads the permissions again (the user may have changed them in System Settings). */
export function readComputerAccess(): void {
  request({ method: "getComputerAccess" })
    .then((response) => set(response.access))
    .catch((error: unknown) => console.error("computer access read failed", error));
}

/** Asks the system for one permission; System Settings opens on its pane. */
export async function allowComputerAccess(grant: ComputerGrant): Promise<void> {
  set((await request({ method: "allowComputerAccess", grant })).access);
}
