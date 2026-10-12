import { create } from "zustand";

import { pageCreate, pageRead, pageWrite } from "@/ipc/client";
import type { Page, PageContent, PageWriteOutcome } from "@/ipc/generated";
import { onPageChanged } from "@/state/pageChanges";
import { onFlushPages } from "@/state/pagesFlush";

/**
 * What an open page's editor shows and what still has to reach its file. Edits are saved
 * about 600 ms after typing stops, one save at a time per page, each based on the revision
 * the editor last saw: a page changed elsewhere in the meantime is never overwritten, it shows
 * a conflict instead. Every edit is also copied to local storage at once, so a crash, a forced
 * quit or a failed save loses nothing: the next open restores it.
 */

export type PageEdit = {
  status: "loading" | "ready" | "missing" | "failed";
  content: PageContent;
  /** The revision on disk this content is based on. */
  revision: string;
  /** Edits not saved yet. */
  dirty: boolean;
  /** The page as it is on disk now, when it changed elsewhere while there were unsaved edits. */
  conflict: Page | null;
  /** Why the last save (or the load) failed; the text stays in the editor and the recovery copy. */
  error: string | null;
  /** Bumped when the content is replaced from outside the editor, so it loads it again. */
  version: number;
};

export type PageIpc = {
  read: (id: string) => Promise<Page>;
  write: (id: string, content: PageContent, baseRevision: string) => Promise<PageWriteOutcome>;
  create: () => Promise<Page>;
};

export const SAVE_DELAY_MS = 600;
const RECOVERY = "brigadier.page.";

type Recovery = { base: string; content: PageContent };

export const EMPTY_PAGE: PageContent = { title: "", emoji: null, body: "" };

export function samePage(a: PageContent, b: PageContent): boolean {
  return a.title === b.title && a.emoji === b.emoji && a.body === b.body;
}

export const usePageEdits = create<{ pages: Record<string, PageEdit> }>(() => ({ pages: {} }));

function readRecovery(id: string): Recovery | null {
  try {
    const raw = localStorage.getItem(RECOVERY + id);
    return raw ? (JSON.parse(raw) as Recovery) : null;
  } catch { return null; }
}

function writeRecovery(id: string, recovery: Recovery | null): void {
  try {
    if (recovery) localStorage.setItem(RECOVERY + id, JSON.stringify(recovery));
    else localStorage.removeItem(RECOVERY + id);
  } catch (error) { console.error("Could not keep the page's recovery copy", error); }
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function get(id: string): PageEdit | undefined {
  return usePageEdits.getState().pages[id];
}

function set(id: string, change: Partial<PageEdit>): void {
  usePageEdits.setState(({ pages }) => {
    const current = pages[id];
    return current ? { pages: { ...pages, [id]: { ...current, ...change } } } : { pages };
  });
}

function settle(id: string, sent: PageContent, revision: string): void {
  const now = get(id);
  if (!now) return;
  const clean = samePage(now.content, sent);
  set(id, { revision, dirty: !clean, error: null });
  writeRecovery(id, clean ? null : { base: revision, content: now.content });
}

/** Use theirs: drop this editor's unsaved text for the page as it is now. */
function takeTheirs(id: string): void {
  const current = get(id);
  if (!current?.conflict) return;
  const page = current.conflict;
  set(id, { content: page.content, revision: page.revision, conflict: null, dirty: false, error: null, version: current.version + 1 });
  writeRecovery(id, null);
}

/** One controller for every open page in the window. */
export function createPageEditing(ipc: PageIpc, delay = SAVE_DELAY_MS) {
  const timers = new Map<string, ReturnType<typeof setTimeout>>();
  /** Each page's saves run one after another. */
  const chains = new Map<string, Promise<void>>();
  const loads = new Map<string, Promise<void>>();
  const openings = new Map<string, number>();
  const readers = new Map<string, number>();
  /** A change heard while a save was running, checked once it ends. */
  const pendingChange = new Map<string, string | null>();

  function serial(id: string, run: () => Promise<void>): Promise<void> {
    const next = (chains.get(id) ?? Promise.resolve()).then(run, run);
    chains.set(id, next);
    void next.finally(() => { if (chains.get(id) === next) chains.delete(id); });
    return next;
  }

  /** Reads the page; a newer recovery copy wins, or shows as a conflict if the file moved on. */
  function open(id: string, retain = true): Promise<void> {
    openings.set(id, (openings.get(id) ?? 0) + 1);
    // A load retry doesn't attach another editor; a second tab does, even after a load failed.
    if (retain) readers.set(id, (readers.get(id) ?? 0) + 1);
    if (get(id) && get(id)!.status !== "failed") return loads.get(id) ?? Promise.resolve();
    usePageEdits.setState(({ pages }) => ({ pages: { ...pages, [id]: {
      status: "loading", content: EMPTY_PAGE, revision: "", dirty: false, conflict: null, error: null, version: 0,
    } } }));
    const load = ipc.read(id).then((page) => {
      if (!get(id)) return;
      const recovery = readRecovery(id);
      const recovered = recovery && !samePage(recovery.content, page.content) ? recovery : null;
      if (recovery && !recovered) writeRecovery(id, null);
      if (recovered && recovered.base === page.revision) {
        set(id, { status: "ready", content: recovered.content, revision: page.revision, dirty: true, version: get(id)!.version + 1 });
        schedule(id);
      } else if (recovered) {
        set(id, { status: "ready", content: recovered.content, revision: recovered.base, dirty: true, conflict: page, version: get(id)!.version + 1 });
      } else {
        set(id, { status: "ready", content: page.content, revision: page.revision, version: get(id)!.version + 1 });
      }
    }, (error: unknown) => {
      if (!get(id)) return;
      const recovery = readRecovery(id);
      // A page whose file is gone still has its unsaved text here; it can be kept as a new page.
      set(id, recovery
        ? { status: "missing", content: recovery.content, revision: recovery.base, dirty: true, error: message(error), version: get(id)!.version + 1 }
        : { status: "failed", error: message(error) });
    }).finally(() => loads.delete(id));
    loads.set(id, load);
    return load;
  }

  function schedule(id: string): void {
    clearTimeout(timers.get(id));
    timers.set(id, setTimeout(() => { timers.delete(id); void save(id); }, delay));
  }

  /** The editor's text changed. */
  function edit(id: string, content: PageContent): void {
    const current = get(id);
    if (!current || current.status === "loading" || samePage(current.content, content)) return;
    set(id, { content, dirty: true });
    writeRecovery(id, { base: current.revision, content });
    if (!current.conflict && current.status === "ready") schedule(id);
  }

  function save(id: string): Promise<void> {
    return serial(id, async () => {
      const current = get(id);
      if (!current || !current.dirty || current.conflict || current.status !== "ready") return;
      const sent = current.content;
      let outcome: PageWriteOutcome;
      try {
        outcome = await ipc.write(id, sent, current.revision);
      } catch (error) {
        set(id, { error: message(error) });
        return;
      }
      if (outcome.type === "conflict") {
        if (samePage(outcome.current.content, sent)) settle(id, sent, outcome.current.revision);
        else set(id, { conflict: outcome.current, error: null });
        return;
      }
      settle(id, sent, outcome.revision);
    }).then(() => recheck(id));
  }


  /** Saves now whatever is waiting. */
  function flush(id: string): Promise<void> {
    if (timers.has(id)) { clearTimeout(timers.get(id)); timers.delete(id); }
    return save(id);
  }

  function flushAll(): Promise<void> {
    return Promise.all(Object.keys(usePageEdits.getState().pages).map(flush)).then(() => undefined);
  }

  /** Another window, program or agent changed the page. */
  function changed(id: string, revision: string | null): void {
    const current = get(id);
    if (!current || current.status === "loading") return;
    if (chains.has(id)) { pendingChange.set(id, revision); return; }
    if (revision !== null && revision === current.revision) return;
    if (revision === null) {
      if (!current.dirty) set(id, { status: "missing" });
      else set(id, { status: "missing", error: "This page was deleted elsewhere." });
      return;
    }
    void ipc.read(id).then((page) => {
      const now = get(id);
      if (!now || page.revision === now.revision) return;
      if (samePage(page.content, now.content)) {
        set(id, { revision: page.revision, dirty: false, conflict: null });
        writeRecovery(id, null);
      } else if (now.dirty) set(id, { conflict: page });
      else set(id, { content: page.content, revision: page.revision, conflict: null, version: now.version + 1 });
    }, (error: unknown) => console.error("Could not read the changed page", error));
  }

  function recheck(id: string): void {
    if (!pendingChange.has(id)) return;
    const revision = pendingChange.get(id)!;
    pendingChange.delete(id);
    changed(id, revision);
  }

  /** Keep mine: save this editor's text over the page as it is now. */
  function keepMine(id: string): Promise<void> {
    const current = get(id);
    if (!current?.conflict) return Promise.resolve();
    set(id, { revision: current.conflict.revision, conflict: null, dirty: true });
    return flush(id);
  }


  /** Keep both: this editor's text becomes a new page; this one shows the page as it is now. */
  async function keepBoth(id: string): Promise<string> {
    const current = get(id);
    if (!current) throw new Error("The page is not open.");
    const copy = await ipc.create();
    const outcome = await ipc.write(copy.id, { ...current.content, title: current.content.title ? `${current.content.title} (copy)` : "" }, copy.revision);
    if (outcome.type !== "saved") throw new Error("Could not save the copy.");
    // Typing can continue while the copy is being saved. Only discard the exact snapshot
    // that reached the copy; later edits still need their recovery copy and conflict choice.
    const now = get(id);
    if (!now || !samePage(now.content, current.content)) return copy.id;
    if (now.conflict) takeTheirs(id);
    else {
      set(id, { dirty: false, error: null });
      writeRecovery(id, null);
    }
    return copy.id;
  }

  /** The editor closed; what is waiting is saved first. */
  function close(id: string): Promise<void> {
    const opening = openings.get(id);
    readers.set(id, Math.max(0, (readers.get(id) ?? 1) - 1));
    return flush(id).finally(() => {
      const current = get(id);
      if (!readers.get(id) && openings.get(id) === opening && current && current.status !== "loading" && !current.dirty && !current.conflict) {
        usePageEdits.setState(({ pages }) => {
          const { [id]: _gone, ...rest } = pages;
          return { pages: rest };
        });
      }
    });
  }

  /** The page was deleted on purpose: nothing of it is kept. */
  function forget(id: string): void {
    readers.delete(id);
    clearTimeout(timers.get(id));
    timers.delete(id);
    writeRecovery(id, null);
    usePageEdits.setState(({ pages }) => {
      const { [id]: _gone, ...rest } = pages;
      return { pages: rest };
    });
  }

  return { open, edit, save, flush, flushAll, changed, keepMine, takeTheirs, keepBoth, close, forget };
}

export type PageEditing = ReturnType<typeof createPageEditing>;

let shared: PageEditing | null = null;

/** The window's page controller, wired to the daemon, the quit's flush and change events. */
export function pageEditing(): PageEditing {
  if (shared) return shared;
  const editing = createPageEditing({ read: pageRead, write: pageWrite, create: pageCreate });
  shared = editing;
  onFlushPages(() => editing.flushAll());
  onPageChanged(({ id, revision }) => editing.changed(id, revision));
  window.addEventListener("pagehide", () => { void editing.flushAll(); });
  return editing;
}
