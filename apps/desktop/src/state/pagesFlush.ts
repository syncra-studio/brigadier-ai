import { listen } from "@tauri-apps/api/event";

import { pagesFlushed } from "@/ipc/client";

/** Saves an editor's pending page edits now; resolves once they are written (or failed). */
export type FlushPages = () => Promise<void>;

/** The shell's event asking the window to save its pages before quitting; its payload is the
 * token `pagesFlushed` answers with. */
export const FLUSH_PAGES_EVENT = "flush-pages";

const handlers = new Set<FlushPages>();

/** Registers what saves an editor's pending page edits when the app quits. The quit waits for
 * every handler (about 1.5 s at most) before stopping the daemon. Returns the unregister. */
export function onFlushPages(handler: FlushPages): () => void {
  handlers.add(handler);
  return () => {
    handlers.delete(handler);
  };
}

/** Runs every registered handler at once and resolves when all have settled: one that throws
 * or rejects doesn't stop the others. */
export async function flushPages(): Promise<void> {
  const results = await Promise.allSettled([...handlers].map((handler) => Promise.resolve().then(handler)));
  for (const result of results) {
    if (result.status === "rejected") console.error("saving a page before quitting failed", result.reason);
  }
}

/** Saves every page, then tells the shell, whatever happened, so the quit goes on. */
export async function answerFlush(token: number, acknowledge: (token: number) => Promise<void> = pagesFlushed): Promise<void> {
  try {
    await flushPages();
  } finally {
    await acknowledge(token);
  }
}

/** Answers the shell's `flush-pages` from now on. Started with the bridge, before any editor
 * registers, so a quit with nothing to save is answered at once. */
export function startPagesFlush(): Promise<() => void> {
  return listen<number>(FLUSH_PAGES_EVENT, (event) => {
    answerFlush(event.payload).catch((error: unknown) => {
      console.error("answering the quit's page save failed", error);
    });
  });
}
