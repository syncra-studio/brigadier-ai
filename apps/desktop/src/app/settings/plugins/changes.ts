import type { PluginItemView, PluginScope, PluginsView } from "./types";

/*
 * The page's changes as they go: each item shows what the user asked for at once, while its
 * agents take it in the background; an item's changes run one after another (a newer one waiting
 * replaces the one that waited before it), and items don't wait on each other. A change that
 * fails says so where it was asked for, and only there: leaving, or the next change that works,
 * takes the error away.
 */

/** What an item shows while a change to it runs. */
export type Overlay = { on?: boolean; scope?: PluginScope };

/** Why a change didn't take, and where it was asked for: the list ("list") or an item's page. */
export type Failure = { key: string | null; place: string; message: string };

/** An item's changes: the one waiting to run after the running one, and what's told it's done. */
type Lane = { next: { job: () => Promise<void>; done: () => void } | null };

export class Changes {
  private overlays = new Map<string, Overlay>();
  private lanes = new Map<string, Lane>();
  private failed: Failure | null = null;
  private listeners = new Set<() => void>();
  private version = 0;

  /** Reads the page again, done once it's read. */
  private readonly reload: () => Promise<void>;

  constructor(reload: () => Promise<void>) {
    this.reload = reload;
  }

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => void this.listeners.delete(listener);
  };

  /** Changes each time anything here does: what `useSyncExternalStore` compares. */
  snapshot = () => this.version;

  private changed() {
    this.version += 1;
    for (const listener of this.listeners) listener();
  }

  /** Whether a change to item `key` is still running or waiting. */
  pending(key: string): boolean {
    return this.lanes.has(key);
  }

  /** The failure to show at `place`, if one was asked for there. */
  failure(place: string): string | null {
    return this.failed && this.failed.place === place ? this.failed.message : null;
  }

  /** The user is at `place` now: a failure from anywhere else is over. */
  leave(place: string) {
    if (this.failed && this.failed.place !== place) {
      this.failed = null;
      this.changed();
    }
  }

  /** `view` as the user asked for it: each item with a running change shows that change. */
  apply(view: PluginsView): PluginsView {
    if (this.overlays.size === 0) return view;
    return { ...view, items: view.items.map((item) => this.item(item)) };
  }

  private item(item: PluginItemView): PluginItemView {
    const overlay = this.overlays.get(item.key);
    if (!overlay) return item;
    return { ...item, on: overlay.on ?? item.on, scope: overlay.scope ?? item.scope };
  }

  /**
   * Runs `work` for item `key` (null: no item's, such as trusting a project) asked for at
   * `place`, the item showing `overlay` until it's done and the page is read again. Done once
   * it ran, or was replaced by a newer change to the same item before it started.
   */
  run(key: string | null, place: string, overlay: Overlay, work: () => Promise<unknown>): Promise<void> {
    const lane = key ?? "";
    if (key !== null) this.overlays.set(key, { ...this.overlays.get(key), ...overlay });
    this.changed();
    return new Promise((resolve) => {
      const job = async () => {
        let error: unknown = null;
        try {
          await work();
        } catch (cause) {
          error = cause;
        }
        this.failed = error === null ? null : { key, place, message: error instanceof Error ? error.message : String(error) };
        // What the agents have now, before the item stops showing what was asked.
        await this.reload().catch(() => {});
      };
      const running = this.lanes.get(lane);
      if (running) {
        // The one that waited is replaced: done, as nothing of it ran.
        running.next?.done();
        running.next = { job, done: resolve };
        return;
      }
      const state: Lane = { next: { job, done: resolve } };
      this.lanes.set(lane, state);
      void (async () => {
        while (state.next) {
          const { job: next, done } = state.next;
          state.next = null;
          await next();
          // A newer change waits: the item keeps showing it.
          if (!state.next) {
            this.lanes.delete(lane);
            if (key !== null) this.overlays.delete(key);
          }
          this.changed();
          done();
        }
      })();
    });
  }
}
